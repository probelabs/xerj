//! Thin blocking ES-compat client (reqwest). Retries with exponential
//! backoff on 429/5xx/transport errors; parses per-item bulk errors.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

#[derive(Clone)]
pub struct Es {
    base: String,
    http: reqwest::blocking::Client,
    api_key: Option<String>,
    bulk_timeout: Duration,
    retry_initial_delay: Duration,
    retry_max_delay: Duration,
}

#[derive(Debug)]
pub struct BulkOutcome {
    pub item_errors: u64,
    pub first_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BulkOperation {
    Index,
    Create,
    Update,
    Delete,
}

impl BulkOperation {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "index" => Some(Self::Index),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    fn has_source_line(self) -> bool {
        !matches!(self, Self::Delete)
    }

    fn accepts_status(self, status: u64) -> bool {
        (200..300).contains(&status) || self == Self::Delete && status == 404
    }
}

fn bulk_operations(body: &[u8]) -> Result<Vec<BulkOperation>> {
    anyhow::ensure!(
        body.last() == Some(&b'\n'),
        "generated bulk request is not newline terminated"
    );
    let lines: Vec<&[u8]> = body
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    let mut operations = Vec::new();
    let mut line = 0usize;
    while line < lines.len() {
        let action: Value = serde_json::from_slice(lines[line])
            .with_context(|| format!("parse generated bulk action line {}", line + 1))?;
        let object = action.as_object().ok_or_else(|| {
            anyhow!(
                "generated bulk action line {} is not a JSON object",
                line + 1
            )
        })?;
        anyhow::ensure!(
            object.len() == 1,
            "generated bulk action line {} must contain exactly one operation",
            line + 1
        );
        let name = object.keys().next().expect("one operation");
        let operation = BulkOperation::parse(name).ok_or_else(|| {
            anyhow!(
                "generated bulk action line {} uses unsupported operation {name}",
                line + 1
            )
        })?;
        operations.push(operation);
        line += 1;
        if operation.has_source_line() {
            anyhow::ensure!(
                line < lines.len(),
                "generated bulk {} action {} is missing its source line",
                operation.name(),
                operations.len()
            );
            line += 1;
        }
    }
    Ok(operations)
}

fn parse_bulk_response(
    mut response: reqwest::blocking::Response,
    expected: &[BulkOperation],
) -> Result<BulkOutcome> {
    let status = response.status();
    if !status.is_success() {
        let body = bounded_response_excerpt(&mut response);
        let detail = if body.is_empty() {
            String::new()
        } else {
            format!(": {body}")
        };
        return Err(anyhow!("bulk HTTP {status}{detail}"));
    }
    let value: Value = response.json().context("parse bulk response")?;
    let errors = value
        .get("errors")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("bulk response is missing boolean `errors`"))?;
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("bulk response is missing array `items`"))?;
    anyhow::ensure!(
        items.len() == expected.len(),
        "bulk response item count mismatch: sent {} action(s), received {} item(s)",
        expected.len(),
        items.len()
    );

    let mut item_errors = 0u64;
    let mut first_error = None;
    for (ordinal, (item, expected_operation)) in items.iter().zip(expected).enumerate() {
        let object = item
            .as_object()
            .ok_or_else(|| anyhow!("bulk response item {} is not an object", ordinal + 1))?;
        anyhow::ensure!(
            object.len() == 1,
            "bulk response item {} must contain exactly one operation",
            ordinal + 1
        );
        let (name, result) = object.iter().next().expect("one operation");
        anyhow::ensure!(
            name == expected_operation.name(),
            "bulk response item {} operation mismatch: sent {}, received {}",
            ordinal + 1,
            expected_operation.name(),
            name
        );
        let result = result.as_object().ok_or_else(|| {
            anyhow!(
                "bulk response item {} operation result is not an object",
                ordinal + 1
            )
        })?;
        let item_status = result
            .get("status")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("bulk response item {} is missing status", ordinal + 1))?;
        let error = result.get("error").filter(|error| !error.is_null());
        if !expected_operation.accepts_status(item_status) || error.is_some() {
            item_errors += 1;
            let detail = format!(
                "item {} {} returned status {}{}",
                ordinal + 1,
                expected_operation.name(),
                item_status,
                error
                    .map(|value| format!(
                        ": {}",
                        value.to_string().chars().take(300).collect::<String>()
                    ))
                    .unwrap_or_default()
            );
            if first_error.is_none() {
                first_error = Some(detail);
            }
        }
    }
    anyhow::ensure!(
        errors == (item_errors > 0),
        "bulk response `errors` flag ({errors}) disagrees with {} rejected item(s)",
        item_errors
    );
    Ok(BulkOutcome {
        item_errors,
        first_error,
    })
}

fn bounded_response_excerpt(response: &mut reqwest::blocking::Response) -> String {
    const LIMIT: u64 = 4 * 1024;
    let mut bytes = Vec::with_capacity(LIMIT as usize);
    let _ = response.take(LIMIT).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ensure an autoindex index-create body pins the index to a single WAL shard.
///
/// autoindex creates one index per inferred dataset — hundreds for a large
/// repo. Each index otherwise opens one WAL file per ingest shard (a count that
/// scales with the server's CPU cores), so hundreds of indices exhaust the
/// process file-descriptor limit — fatal on macOS, whose default soft limit is
/// 256. `index.xerj_ingest_shards = 1` keeps each index at a single WAL fd.
/// Non-destructive: only fills the key when absent.
fn with_single_wal_shard(body: &Value) -> Value {
    let mut body = body.clone();
    if let Some(obj) = body.as_object_mut() {
        let settings = obj
            .entry("settings")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(sobj) = settings.as_object_mut() {
            let index = sobj.entry("index").or_insert_with(|| serde_json::json!({}));
            if let Some(iobj) = index.as_object_mut() {
                iobj.entry("xerj_ingest_shards")
                    .or_insert_with(|| serde_json::json!(1));
            }
        }
    }
    body
}

impl Es {
    pub fn new(url: &str, api_key: Option<String>) -> Result<Self> {
        Self::with_bulk_timeout(url, api_key, 300)
    }

    pub fn with_bulk_timeout(
        url: &str,
        api_key: Option<String>,
        bulk_timeout_secs: u64,
    ) -> Result<Self> {
        Self::with_bulk_policy(
            url,
            api_key,
            Duration::from_secs(bulk_timeout_secs),
            Duration::from_millis(250),
            Duration::from_secs(8),
        )
    }

    fn with_bulk_policy(
        url: &str,
        api_key: Option<String>,
        bulk_timeout: Duration,
        retry_initial_delay: Duration,
        retry_max_delay: Duration,
    ) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(300))
            .danger_accept_invalid_certs(true)
            .build()
            .context("build http client")?;
        Ok(Es {
            base: url.trim_end_matches('/').to_string(),
            http,
            api_key,
            bulk_timeout,
            retry_initial_delay,
            retry_max_delay,
        })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::blocking::RequestBuilder {
        let mut r = self.http.request(method, format!("{}{}", self.base, path));
        if let Some(k) = &self.api_key {
            r = r.header("Authorization", format!("ApiKey {k}"));
        }
        r
    }

    pub fn ping(&self) -> Result<Value> {
        let resp = self
            .req(reqwest::Method::GET, "/")
            .send()
            .with_context(|| format!("endpoint unreachable: {}", self.base))?;
        Ok(resp.json().unwrap_or(Value::Null))
    }

    /// GET an arbitrary path and return only the HTTP status code.
    ///
    /// One attempt, no retry/backoff: `xerj brain` uses this to probe
    /// whether a server is listening (`/health/ready`, auth-exempt) and
    /// whether its credentials are accepted (`/`), and a liveness probe
    /// that silently retried for ~16s would misreport "already running"
    /// as "boot took 16s". A transport error means nothing answered.
    pub fn get_status(&self, path: &str) -> Result<u16> {
        let resp = self
            .req(reqwest::Method::GET, path)
            .send()
            .with_context(|| format!("no response from {}{}", self.base, path))?;
        Ok(resp.status().as_u16())
    }

    /// Retry wrapper: 429/5xx/transport → backoff 250ms..8s, 6 attempts.
    fn with_retry<T>(
        &self,
        what: &str,
        mut f: impl FnMut() -> Result<reqwest::blocking::Response>,
        parse: impl Fn(reqwest::blocking::Response) -> Result<T>,
    ) -> Result<T> {
        const MAX_ATTEMPTS: usize = 6;
        let mut delay = self.retry_initial_delay;
        let mut last_err = None;
        for attempt in 0..MAX_ATTEMPTS {
            match f() {
                Ok(resp) => {
                    let status = resp.status();
                    if status.as_u16() == 429 || status.is_server_error() {
                        let mut resp = resp;
                        let body = bounded_response_excerpt(&mut resp);
                        let detail = if body.is_empty() {
                            String::new()
                        } else {
                            format!(": {body}")
                        };
                        last_err = Some(anyhow!("{what}: HTTP {status}{detail}"));
                    } else {
                        return parse(resp);
                    }
                }
                Err(e) => last_err = Some(e),
            }
            if attempt + 1 < MAX_ATTEMPTS {
                std::thread::sleep(delay);
                delay = (delay * 2).min(self.retry_max_delay);
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("{what}: retries exhausted")))
    }

    /// PUT index with explicit mapping; tolerates already-exists.
    pub fn ensure_index(&self, index: &str, body: &Value) -> Result<()> {
        let body = with_single_wal_shard(body);
        let resp = self
            .req(reqwest::Method::PUT, &format!("/{index}"))
            .json(&body)
            .send()
            .context("PUT index")?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().unwrap_or_default();
        if text.contains("resource_already_exists")
            || status.as_u16() == 400 && text.contains("exists")
        {
            return Ok(());
        }
        Err(anyhow!("PUT /{index} failed: {status} {text}"))
    }

    /// Additive mapping update for fields introduced after an index was created.
    pub fn update_mapping(&self, index: &str, body: &Value) -> Result<()> {
        let resp = self
            .req(reqwest::Method::PUT, &format!("/{index}/_mapping"))
            .json(body)
            .send()
            .context("PUT mapping")?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().unwrap_or_default();
        Err(anyhow!("PUT /{index}/_mapping failed: {status} {text}"))
    }

    pub fn bulk(&self, body: Vec<u8>) -> Result<BulkOutcome> {
        let outcome = self.bulk_allow_item_rejections(body)?;
        anyhow::ensure!(
            outcome.item_errors == 0,
            "bulk rejected {} item(s): {}",
            outcome.item_errors,
            outcome
                .first_error
                .as_deref()
                .unwrap_or("unknown bulk item rejection")
        );
        Ok(outcome)
    }

    /// Execute a bulk while returning item rejections to the caller.
    ///
    /// Only the create-if-absent metadata path uses this escape hatch because
    /// it can prove that a 409 race established the desired document. Every
    /// ordinary publication must use `bulk`, whose default is fail-closed.
    pub(crate) fn bulk_allow_item_rejections(&self, body: Vec<u8>) -> Result<BulkOutcome> {
        let expected = bulk_operations(&body)?;
        self.with_retry(
            "_bulk",
            || self.send_bulk(body.clone()),
            |response| parse_bulk_response(response, &expected),
        )
    }

    fn send_bulk(&self, body: Vec<u8>) -> Result<reqwest::blocking::Response> {
        self.req(reqwest::Method::POST, "/_bulk")
            .timeout(self.bulk_timeout)
            .header("Content-Type", "application/x-ndjson")
            .header("X-Turbo", "1")
            .body(body)
            .send()
            .with_context(|| format!("bulk send (request timeout {:?})", self.bulk_timeout))
    }

    /// Remove every document matching `query` before a file-level replacement.
    /// Refresh is requested so a retry cannot observe or retain an older
    /// locator set alongside the replacement. The server executes ONE bounded
    /// search-and-delete pass per call (size-capped at 10k docs), so a single
    /// response is not complete removal: repeat until a pass deletes nothing.
    pub fn delete_by_query(&self, index: &str, query: &Value) -> Result<()> {
        const MAX_PASSES: usize = 1_000;
        for _ in 0..MAX_PASSES {
            if self.delete_by_query_pass(index, query)? == 0 {
                return Ok(());
            }
        }
        Err(anyhow!(
            "POST /{index}/_delete_by_query still reported deletions after {MAX_PASSES} passes; \
             refusing to treat the previous generation as fully removed"
        ))
    }

    /// One server-side delete pass; returns the reported `deleted` count.
    fn delete_by_query_pass(&self, index: &str, query: &Value) -> Result<u64> {
        self.with_retry(
            "delete_by_query",
            || {
                self.req(
                    reqwest::Method::POST,
                    &format!("/{index}/_delete_by_query?refresh=true"),
                )
                .json(&serde_json::json!({"query": query}))
                .send()
                .map_err(|e| anyhow!("delete_by_query: {e}"))
            },
            |resp| {
                let status = resp.status();
                let body: Value = resp.json().unwrap_or(Value::Null);
                if status.is_success()
                    && body
                        .get("failures")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty)
                {
                    Ok(body.get("deleted").and_then(Value::as_u64).unwrap_or(0))
                } else {
                    Err(anyhow!(
                        "POST /{index}/_delete_by_query failed: HTTP {status}: {body}"
                    ))
                }
            },
        )
    }

    pub fn refresh(&self, pattern: &str) -> Result<()> {
        self.with_retry(
            "refresh",
            || {
                self.req(reqwest::Method::POST, &format!("/{pattern}/_refresh"))
                    .send()
                    .map_err(|e| anyhow!("refresh: {e}"))
            },
            |resp| {
                if resp.status().is_success() {
                    Ok(())
                } else {
                    Err(anyhow!("refresh HTTP {}", resp.status()))
                }
            },
        )
    }

    pub fn search(&self, index: &str, body: &Value) -> Result<Value> {
        self.with_retry(
            "search",
            || {
                self.req(reqwest::Method::POST, &format!("/{index}/_search"))
                    .json(body)
                    .send()
                    .map_err(|e| anyhow!("search: {e}"))
            },
            |resp| {
                let status = resp.status();
                let v: Value = resp.json().unwrap_or(Value::Null);
                if !status.is_success() {
                    return Err(anyhow!("search /{index} HTTP {status}: {v}"));
                }
                Ok(v)
            },
        )
    }

    pub fn count(&self, index: &str) -> Result<u64> {
        // _count may not exist on all builds — use size:0 search with totals.
        let v = self.search(
            index,
            &serde_json::json!({"size": 0, "track_total_hits": true}),
        )?;
        v.pointer("/hits/total/value")
            .and_then(|t| t.as_u64())
            .or_else(|| v.pointer("/hits/total").and_then(|t| t.as_u64()))
            .ok_or_else(|| anyhow!("no total in search response"))
    }

    /// `_cat/indices` is plain text, no header (?format=json is IGNORED —
    /// verified). Returns (index, docs_count) with `.xerj_*` system indices
    /// filtered out.
    pub fn cat_indices(&self) -> Result<Vec<(String, u64)>> {
        let resp = self
            .req(reqwest::Method::GET, "/_cat/indices")
            .send()
            .context("_cat/indices")?;
        let text = resp.text().unwrap_or_default();
        let mut out = Vec::new();
        for line in text.lines() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 3 {
                continue;
            }
            // format: health status index uuid pri rep docs.count deleted size…
            let name = cols[2].to_string();
            if name.starts_with(".xerj") || name.starts_with('.') {
                continue;
            }
            let docs = cols
                .get(6)
                .and_then(|c| c.parse::<u64>().ok())
                // fallback for column-order variants: first integer after the
                // uuid that is not the 1-digit pri/rep pair
                .or_else(|| cols.iter().skip(7).find_map(|c| c.parse::<u64>().ok()))
                .unwrap_or(0);
            out.push((name, docs));
        }
        Ok(out)
    }

    pub fn get_doc(&self, index: &str, id: &str) -> Result<Option<Value>> {
        let resp = self
            .req(reqwest::Method::GET, &format!("/{index}/_doc/{id}"))
            .send()
            .context("GET doc")?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        let v: Value = resp.json().unwrap_or(Value::Null);
        if v.get("found").and_then(|f| f.as_bool()).unwrap_or(false) {
            Ok(v.get("_source").cloned())
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Es;
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn bulk_request_uses_custom_timeout_and_drops_timed_out_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (closed_tx, closed_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..read]).contains("POST /_bulk"));
            std::thread::sleep(Duration::from_millis(1300));
            let closed = stream.read(&mut request).map(|n| n == 0).unwrap_or(true);
            closed_tx.send(closed).unwrap();
        });
        let es = Es::with_bulk_policy(
            &format!("http://{address}"),
            None,
            Duration::from_secs(1),
            Duration::from_millis(10),
            Duration::from_millis(20),
        )
        .unwrap();
        let started = Instant::now();
        let err = es.send_bulk(b"{}\n".to_vec()).unwrap_err();
        let elapsed = started.elapsed();
        assert!(format!("{err:#}").contains("timed out"), "{err:#}");
        assert!(elapsed >= Duration::from_millis(800), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
        assert!(closed_rx.recv_timeout(Duration::from_secs(4)).unwrap());
        server.join().unwrap();
    }

    fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            request.extend_from_slice(&buffer[..count]);
            if let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + length {
                    break;
                }
            }
        }
        request
    }

    fn success(stream: &mut std::net::TcpStream) {
        respond_json(
            stream,
            br#"{"errors":false,"items":[{"index":{"status":201}}]}"#,
        );
    }

    fn respond_json(stream: &mut std::net::TcpStream, body: &[u8]) {
        respond(stream, "200 OK", body);
    }

    fn respond(stream: &mut std::net::TcpStream, status: &str, body: &[u8]) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    }

    fn bulk_against_one_response(
        body: Vec<u8>,
        response: &'static [u8],
    ) -> anyhow::Result<super::BulkOutcome> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            respond_json(&mut stream, response);
        });
        let es = Es::with_bulk_policy(
            &format!("http://{address}"),
            None,
            Duration::from_millis(100),
            Duration::from_millis(1),
            Duration::from_millis(2),
        )
        .unwrap();
        let result = es.bulk(body);
        server.join().unwrap();
        result
    }

    #[test]
    fn bulk_response_requires_exact_well_formed_item_accounting() {
        let request = b"{\"index\":{}}\n{}\n".to_vec();
        let cases: &[(&[u8], &str)] = &[
            (b"not-json", "parse bulk response"),
            (
                br#"{"items":[{"index":{"status":201}}]}"#,
                "boolean `errors`",
            ),
            (br#"{"errors":false}"#, "array `items`"),
            (br#"{"errors":false,"items":[]}"#, "item count mismatch"),
            (
                br#"{"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}}]}"#,
                "item count mismatch",
            ),
            (
                br#"{"errors":false,"items":["malformed"]}"#,
                "item 1 is not an object",
            ),
            (
                br#"{"errors":false,"items":[{"delete":{"status":200}}]}"#,
                "operation mismatch",
            ),
            (
                br#"{"errors":false,"items":[{"index":{"result":"created"}}]}"#,
                "missing status",
            ),
            (
                br#"{"errors":true,"items":[{"index":{"status":201}}]}"#,
                "disagrees with 0 rejected",
            ),
            (
                br#"{"errors":false,"items":[{"index":{"status":400,"error":{"type":"bad"}}}]}"#,
                "disagrees with 1 rejected",
            ),
        ];
        for (response, expected) in cases {
            let error = bulk_against_one_response(request.clone(), response).unwrap_err();
            assert!(
                format!("{error:#}").contains(expected),
                "{expected}: {error:#}"
            );
        }
    }

    #[test]
    fn bulk_response_matches_mixed_operations_and_accepts_absent_delete() {
        let request =
            b"{\"index\":{\"_id\":\"a\"}}\n{\"v\":1}\n{\"delete\":{\"_id\":\"b\"}}\n".to_vec();
        let outcome = bulk_against_one_response(
            request,
            br#"{"errors":false,"items":[{"index":{"status":201}},{"delete":{"status":404}}]}"#,
        )
        .unwrap();
        assert_eq!(outcome.item_errors, 0);
    }

    #[test]
    fn terminal_retry_error_keeps_a_bounded_backend_reason() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                let body = format!("admission queue full {}", "x".repeat(8 * 1024));
                respond(&mut stream, "503 Service Unavailable", body.as_bytes());
            }
        });
        let es = Es::with_bulk_policy(
            &format!("http://{address}"),
            None,
            Duration::from_millis(100),
            Duration::from_millis(1),
            Duration::from_millis(2),
        )
        .unwrap();
        let error = es
            .bulk(b"{\"index\":{}}\n{}\n".to_vec())
            .expect_err("six retryable responses must fail");
        let message = format!("{error:#}");
        assert!(message.contains("HTTP 503"), "{message}");
        assert!(message.contains("admission queue full"), "{message}");
        assert!(message.len() < 4_300, "backend excerpt was not bounded");
        server.join().unwrap();
    }

    #[test]
    fn retry_path_recovers_after_one_bulk_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            read_request(&mut first);
            let first = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(120));
                drop(first);
            });
            let (mut second, _) = listener.accept().unwrap();
            read_request(&mut second);
            success(&mut second);
            first.join().unwrap();
        });
        let es = Es::with_bulk_policy(
            &format!("http://{address}"),
            None,
            Duration::from_millis(50),
            Duration::from_millis(10),
            Duration::from_millis(20),
        )
        .unwrap();
        let outcome = es.bulk(b"{\"index\":{}}\n{}\n".to_vec()).unwrap();
        assert_eq!(outcome.item_errors, 0);
        server.join().unwrap();
    }

    #[test]
    fn all_timeout_retry_path_is_bounded_to_six_attempts_without_final_sleep() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let accepted = Arc::new(Mutex::new(0usize));
        let accepted_server = accepted.clone();
        let server = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                *accepted_server.lock().unwrap() += 1;
                handlers.push(std::thread::spawn(move || {
                    read_request(&mut stream);
                    std::thread::sleep(Duration::from_millis(100));
                }));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        let es = Es::with_bulk_policy(
            &format!("http://{address}"),
            None,
            Duration::from_millis(25),
            Duration::from_millis(10),
            Duration::from_millis(20),
        )
        .unwrap();
        let started = Instant::now();
        let error = match es.bulk(b"{\"index\":{}}\n{}\n".to_vec()) {
            Ok(_) => panic!("all six delayed responses unexpectedly succeeded"),
            Err(error) => error,
        };
        let elapsed = started.elapsed();
        assert!(format!("{error:#}").contains("timed out"), "{error:#}");
        assert_eq!(*accepted.lock().unwrap(), 6);
        // Six 25ms attempts plus 10+20+20+20+20ms backoffs. A sixth
        // post-failure sleep would push this past the deliberately tight cap.
        assert!(elapsed < Duration::from_millis(260), "{elapsed:?}");
        server.join().unwrap();
    }

    #[test]
    fn delete_by_query_repeats_until_a_pass_deletes_nothing() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_server = requests.clone();
        let server = std::thread::spawn(move || {
            // A previous generation larger than one server pass: the first
            // pass removes its 10k cap and only the second reports done.
            for body in [
                br#"{"deleted":10000,"failures":[]}"#.as_slice(),
                br#"{"deleted":0,"failures":[]}"#.as_slice(),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                requests_server
                    .lock()
                    .unwrap()
                    .push(read_request(&mut stream));
                respond_json(&mut stream, body);
            }
        });
        let es = Es::with_bulk_policy(
            &format!("http://{address}"),
            None,
            Duration::from_millis(100),
            Duration::from_millis(10),
            Duration::from_millis(20),
        )
        .unwrap();
        es.delete_by_query("data", &serde_json::json!({"term": {"ax_file": "key"}}))
            .unwrap();
        server.join().unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            let text = String::from_utf8_lossy(request);
            assert!(text.contains("POST /data/_delete_by_query"), "{text}");
        }
    }

    #[test]
    fn delete_by_query_that_never_reaches_zero_fails_at_the_pass_cap() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut served = 0usize;
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                if request.starts_with(b"STOP") {
                    break;
                }
                respond_json(&mut stream, br#"{"deleted":10000,"failures":[]}"#);
                served += 1;
            }
            served
        });
        let es = Es::with_bulk_policy(
            &format!("http://{address}"),
            None,
            Duration::from_millis(100),
            Duration::from_millis(10),
            Duration::from_millis(20),
        )
        .unwrap();
        let error = es
            .delete_by_query("data", &serde_json::json!({"term": {"ax_file": "key"}}))
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("still reported deletions"),
            "{error:#}"
        );
        let mut stop = std::net::TcpStream::connect(address).unwrap();
        stop.write_all(b"STOP\r\n\r\n").unwrap();
        drop(stop);
        assert_eq!(server.join().unwrap(), 1_000);
    }

    #[test]
    fn ambiguous_dropped_response_retries_byte_identical_bulk_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_server = requests.clone();
        let server = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            requests_server
                .lock()
                .unwrap()
                .push(read_request(&mut first));
            drop(first); // backend may have committed, but response is ambiguous
            let (mut second, _) = listener.accept().unwrap();
            requests_server
                .lock()
                .unwrap()
                .push(read_request(&mut second));
            success(&mut second);
        });
        let body = b"{\"index\":{\"_index\":\"i\",\"_id\":\"stable-id\"}}\n{\"v\":1}\n".to_vec();
        let es = Es::with_bulk_policy(
            &format!("http://{address}"),
            None,
            Duration::from_millis(100),
            Duration::from_millis(10),
            Duration::from_millis(20),
        )
        .unwrap();
        es.bulk(body.clone()).unwrap();
        server.join().unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            let offset = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            assert_eq!(&request[offset..], body.as_slice());
        }
    }
}
