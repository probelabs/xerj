//! Issue #320 — push a filtered subset of indices to an external
//! ES-compatible target (single-node WAL tap).
//!
//! Every test here fails on the pre-#320 tree for the same reason: nothing
//! pushed data out of the engine at all. `_ccr/*` answered `501`,
//! reindex-from-remote was refused up front, and snapshot/restore was the only
//! export path — scheduled, whole-index, not near-real-time.
//!
//! The target is a stub HTTP server that speaks just enough `_bulk` to record
//! what it was sent, which is also the point of the feature: the wire format
//! is `_bulk` and nothing else, so an Elasticsearch cluster, an OpenSearch
//! cluster or another xerj node are all the same to the tap.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use xerj_common::config::{Config, WalTapConfig};
use xerj_common::types::Schema;
use xerj_engine::Engine;

// ─────────────────────────────────────────────────────────────────────────────
// A stub `_bulk` target
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Received {
    /// Every NDJSON body the target was POSTed, in order.
    bodies: Vec<String>,
    /// `Authorization` header of the last request, if any.
    last_auth: Option<String>,
}

struct StubTarget {
    url: String,
    received: Arc<Mutex<Received>>,
    _task: tokio::task::JoinHandle<()>,
}

impl StubTarget {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Received::default()));
        let sink = received.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let sink = sink.clone();
                tokio::spawn(async move {
                    let _ = serve_one(stream, sink).await;
                });
            }
        });
        Self {
            url,
            received,
            _task: task,
        }
    }

    /// Every `_bulk` action line the target has seen, paired with the source
    /// line that followed it (`None` for a delete).
    fn actions(&self) -> Vec<(Value, Option<Value>)> {
        let bodies = self.received.lock().unwrap().bodies.clone();
        let mut out = Vec::new();
        for body in bodies {
            let mut lines = body.lines();
            while let Some(meta) = lines.next() {
                if meta.trim().is_empty() {
                    continue;
                }
                let meta: Value = serde_json::from_str(meta)
                    .unwrap_or_else(|e| panic!("bad action line {meta:?}: {e}"));
                let is_delete = meta.get("delete").is_some();
                let source = if is_delete {
                    None
                } else {
                    Some(serde_json::from_str(lines.next().expect("source line")).unwrap())
                };
                out.push((meta, source));
            }
        }
        out
    }

    fn indexed_ids(&self, index: &str) -> Vec<String> {
        self.actions()
            .iter()
            .filter_map(|(meta, _)| meta.get("index"))
            .filter(|a| a["_index"] == index)
            .map(|a| a["_id"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn request_count(&self) -> usize {
        self.received.lock().unwrap().bodies.len()
    }
}

/// Read one HTTP request, record the body, answer with an ES-shaped `_bulk`
/// response.
async fn serve_one(mut stream: TcpStream, sink: Arc<Mutex<Received>>) -> std::io::Result<()> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&raw).into_owned();
        let Some(head_end) = text.find("\r\n\r\n") else {
            continue;
        };
        let head = &text[..head_end];
        let len: usize = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse().ok())?
            })
            .unwrap_or(0);
        if raw.len() < head_end + 4 + len {
            continue;
        }
        let body = text[head_end + 4..head_end + 4 + len].to_string();
        let auth = head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("authorization")
                .then(|| v.trim().to_string())
        });
        {
            let mut guard = sink.lock().unwrap();
            guard.bodies.push(body);
            guard.last_auth = auth;
        }
        let payload = br#"{"took":1,"errors":false,"items":[]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        stream.write_all(resp.as_bytes()).await?;
        stream.write_all(payload).await?;
        stream.flush().await?;
        break;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Harness
// ─────────────────────────────────────────────────────────────────────────────

fn engine_with_tap(dir: &TempDir, tap: WalTapConfig) -> Engine {
    let mut config = Config::default();
    config.server.data_dir = dir.path().to_str().unwrap().to_string();
    config.wal_tap = tap;
    Engine::new(config).expect("engine::new")
}

fn tap_config(url: &str, indices: &[&str]) -> WalTapConfig {
    WalTapConfig {
        enabled: true,
        target_url: url.to_string(),
        indices: indices.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

async fn write_docs(engine: &Engine, index: &str, ids: &[&str]) {
    if engine.get_index(index).is_err() {
        engine.create_index(index, Schema::empty()).unwrap();
    }
    let idx = engine.get_index(index).unwrap();
    for id in ids {
        idx.index_document(
            Some((*id).to_string()),
            json!({"msg": format!("hello {id}")}),
        )
        .await
        .unwrap();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// The headline. Before #320 there was no push path of any kind: this asserts
/// that documents written to an allowlisted index land at an external
/// `_bulk` target, and that documents written to an index outside the
/// allowlist do not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_allowlisted_index_is_pushed_to_an_external_bulk_target() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-*"]));

    write_docs(&engine, "edge-logs", &["1", "2", "3"]).await;
    write_docs(&engine, "orders", &["o1"]).await;

    engine.wal_tap.tick(&engine).await;

    let mut shipped = target.indexed_ids("edge-logs");
    shipped.sort();
    assert_eq!(
        shipped,
        vec!["1", "2", "3"],
        "every document of an allowlisted index must reach the target"
    );
    assert!(
        target.indexed_ids("orders").is_empty(),
        "the allowlist is a filter, not a suggestion: {:?}",
        target.actions()
    );

    let stats = engine.wal_tap.stats();
    let edge = stats.get("edge-logs").expect("stats for edge-logs");
    assert_eq!(edge.docs_shipped, 3);
    assert_eq!(edge.bulk_failures, 0);
    assert_eq!(edge.gaps, 0);
    assert!(edge.last_error.is_none());
}

/// At-least-once delivery is only safe because every action carries the WAL
/// `seq_no` as an external version — the same "highest seq wins" rule the
/// engine's own version map uses. Without it a redelivered or reordered batch
/// could resurrect an older document at the target.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_action_carries_external_versioning_and_the_document_id() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-logs"]));

    write_docs(&engine, "edge-logs", &["a"]).await;
    engine.wal_tap.tick(&engine).await;

    let actions = target.actions();
    let (meta, source) = actions.first().expect("one action");
    let action = &meta["index"];
    assert_eq!(action["_index"], "edge-logs");
    assert_eq!(action["_id"], "a");
    assert_eq!(action["version_type"], "external");
    assert!(
        action["version"].as_u64().unwrap_or(0) > 0,
        "the WAL seq_no must be the external version, got {action}"
    );
    assert_eq!(source.as_ref().unwrap()["msg"], "hello a");
}

/// A delete has to travel too, or the target diverges permanently on every
/// removed document.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletes_are_pushed_as_bulk_delete_actions() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-logs"]));

    write_docs(&engine, "edge-logs", &["a", "b"]).await;
    engine
        .get_index("edge-logs")
        .unwrap()
        .delete_document("a")
        .await
        .unwrap();

    engine.wal_tap.tick(&engine).await;

    let deletes: Vec<String> = target
        .actions()
        .iter()
        .filter_map(|(meta, _)| meta.get("delete").cloned())
        .map(|a| a["_id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(deletes, vec!["a"], "the delete must reach the target");
    assert_eq!(engine.wal_tap.stats()["edge-logs"].deletes_shipped, 1);
}

/// The one rule the allowlist cannot override. `.xerj-*` holds API keys,
/// sessions and console state; `indices = ["*"]` must not ship it anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn system_indices_are_never_pushed_even_with_a_star_allowlist() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["*"]));

    write_docs(&engine, "edge-logs", &["1"]).await;
    write_docs(&engine, ".xerj-secrets", &["k1"]).await;

    engine.wal_tap.tick(&engine).await;

    assert_eq!(target.indexed_ids("edge-logs"), vec!["1"]);
    let shipped_indices: Vec<String> = target
        .actions()
        .iter()
        .filter_map(|(meta, _)| meta.as_object()?.values().next()?.get("_index").cloned())
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !shipped_indices.iter().any(|i| i.starts_with(".xerj")),
        "a system index escaped to an external cluster: {shipped_indices:?}"
    );
}

/// Backpressure and durability: an unreachable target must not lose data and
/// must not grow anything locally. The cursor stays put, the failure is
/// visible, and the documents arrive once the target comes back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_target_holds_the_cursor_and_reports_why() {
    let dir = TempDir::new().unwrap();
    // Bind and immediately drop a listener to get a port nothing is serving.
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        format!("http://{addr}")
    };
    let engine = engine_with_tap(&dir, tap_config(&dead, &["edge-logs"]));

    write_docs(&engine, "edge-logs", &["1", "2"]).await;
    engine.wal_tap.tick(&engine).await;

    let stats = engine.wal_tap.stats();
    let edge = stats.get("edge-logs").expect("stats for edge-logs");
    assert!(edge.bulk_failures >= 1, "the failure must be counted");
    assert_eq!(edge.docs_shipped, 0, "nothing was actually delivered");
    assert!(
        edge.last_error.is_some(),
        "an operator must be able to see why the tap stopped advancing"
    );
    assert!(
        engine
            .wal_tap
            .cursors()
            .get("edge-logs")
            .map(|shards| shards.is_empty())
            .unwrap_or(true),
        "the cursor must not advance past entries the target never received"
    );

    // Point it at a live target: the held-back documents go now.
    let target = StubTarget::start().await;
    let mut config = engine.wal_tap.config();
    config.target_url = target.url.clone();
    config.max_retry_backoff_secs = 1;
    engine.wal_tap.set_config(config);
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    engine.wal_tap.tick(&engine).await;

    let mut shipped = target.indexed_ids("edge-logs");
    shipped.sort();
    assert_eq!(
        shipped,
        vec!["1", "2"],
        "documents held during the outage must ship once the target returns"
    );
}

/// A cursor that has already shipped an entry must not ship it again on the
/// next poll, and a restart must resume from the persisted position rather
/// than replaying the whole retained WAL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursors_are_durable_so_a_restart_does_not_re_ship_everything() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let tap = tap_config(&target.url, &["edge-logs"]);

    {
        let engine = engine_with_tap(&dir, tap.clone());
        write_docs(&engine, "edge-logs", &["1", "2"]).await;
        engine.wal_tap.tick(&engine).await;
        assert_eq!(target.indexed_ids("edge-logs").len(), 2);

        // An idle poll ships nothing.
        engine.wal_tap.tick(&engine).await;
        assert_eq!(
            target.request_count(),
            1,
            "an idle poll must not re-send an already-shipped batch"
        );
    }

    // Restart against the same data directory.
    let engine = engine_with_tap(&dir, tap);
    write_docs(&engine, "edge-logs", &["3"]).await;
    engine.wal_tap.tick(&engine).await;

    let shipped = target.indexed_ids("edge-logs");
    assert_eq!(
        shipped,
        vec!["1", "2", "3"],
        "a restart must resume, not replay: {shipped:?}"
    );
}

/// The credential travels to the target and nowhere else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_configured_credential_is_sent_to_the_target() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let mut tap = tap_config(&target.url, &["edge-logs"]);
    tap.target_auth = "ApiKey s3cr3t".to_string();
    let engine = engine_with_tap(&dir, tap);

    write_docs(&engine, "edge-logs", &["1"]).await;
    engine.wal_tap.tick(&engine).await;

    assert_eq!(
        target.received.lock().unwrap().last_auth.as_deref(),
        Some("ApiKey s3cr3t")
    );
}

/// The allowlist is runtime-adjustable without a restart — that is half the
/// point of the feature for an edge node whose curation changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_allowlist_can_be_changed_at_runtime() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-logs"]));

    write_docs(&engine, "edge-logs", &["1"]).await;
    write_docs(&engine, "metrics", &["m1"]).await;
    engine.wal_tap.tick(&engine).await;
    assert!(target.indexed_ids("metrics").is_empty());

    let mut config = engine.wal_tap.config();
    config.indices.push("metrics".to_string());
    engine.wal_tap.set_config(config);

    write_docs(&engine, "metrics", &["m2"]).await;
    engine.wal_tap.tick(&engine).await;

    assert!(
        target.indexed_ids("metrics").contains(&"m2".to_string()),
        "an index added to the allowlist must start shipping: {:?}",
        target.actions()
    );
}

/// Off by default, and inert when on but unconfigured. A feature that pushes
/// documents to another cluster must never do so because someone upgraded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tap_is_off_by_default_and_inert_without_a_target() {
    assert!(!WalTapConfig::default().enabled);
    assert!(WalTapConfig::default().indices.is_empty());

    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    // Enabled, allowlisted — but no target URL.
    let engine = engine_with_tap(
        &dir,
        WalTapConfig {
            enabled: true,
            target_url: String::new(),
            indices: vec!["*".into()],
            ..Default::default()
        },
    );
    write_docs(&engine, "edge-logs", &["1"]).await;
    engine.wal_tap.tick(&engine).await;
    assert_eq!(target.request_count(), 0);
    assert!(engine.wal_tap.stats().is_empty());
}

/// `max_batch_docs` and `max_batch_bytes` bound each request rather than
/// silently truncating the stream: everything still arrives, in more requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_limits_split_the_stream_instead_of_dropping_it() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let mut tap = tap_config(&target.url, &["edge-logs"]);
    tap.max_batch_docs = 3;
    tap.max_batch_bytes = 200;
    let engine = engine_with_tap(&dir, tap);

    let ids: Vec<String> = (1..=9).map(|i| i.to_string()).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    write_docs(&engine, "edge-logs", &refs).await;

    engine.wal_tap.tick(&engine).await;

    let mut shipped = target.indexed_ids("edge-logs");
    shipped.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(shipped, expected, "a bounded batch must not drop documents");
    assert!(
        target.request_count() > 1,
        "the byte/doc caps must actually split the stream"
    );

    // And every document arrived exactly once — the caps must not double-send.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for id in target.indexed_ids("edge-logs") {
        *counts.entry(id).or_default() += 1;
    }
    assert!(
        counts.values().all(|&c| c == 1),
        "documents were delivered more than once: {counts:?}"
    );
}

/// An index deleted and recreated under the same name gets a brand-new WAL
/// starting at generation 0, which the old cursor points far past. Left alone
/// that stalls the shard silently and forever — the worst failure mode a WAL
/// consumer can have, because `_stats` would show a healthy tap shipping
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recreating_an_index_under_the_same_name_does_not_stall_the_tap() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-logs"]));

    write_docs(&engine, "edge-logs", &["1"]).await;
    engine.wal_tap.tick(&engine).await;
    assert_eq!(target.indexed_ids("edge-logs"), vec!["1"]);

    engine.delete_index("edge-logs").await.unwrap();
    // The tick that observes the deletion forgets the stale cursor.
    engine.wal_tap.tick(&engine).await;
    assert!(
        !engine.wal_tap.cursors().contains_key("edge-logs"),
        "the cursor of a deleted index must not outlive it"
    );

    write_docs(&engine, "edge-logs", &["2"]).await;
    engine.wal_tap.tick(&engine).await;
    assert!(
        target.indexed_ids("edge-logs").contains(&"2".to_string()),
        "a recreated index must keep shipping: {:?}",
        target.actions()
    );
}

/// `_stats` is the honesty surface, so `lag_seq` must not lie in the one case
/// where the naive arithmetic does: a tap that positions at the END of an
/// established index is caught up by definition — it deliberately skipped the
/// history it cannot backfill. Reporting that history as a backlog would send
/// an operator hunting a problem the tap does not have.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tap_that_started_at_the_end_does_not_report_history_as_lag() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    // Enabled but allowlisting nothing, so the index accumulates history the
    // tap never saw.
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["nothing-*"]));
    write_docs(&engine, "edge-logs", &["1", "2", "3"]).await;

    // `flush()` force-rotates every WAL shard and prunes the generations whose
    // entries are now durable in a segment, which is exactly the state of an
    // established index: generation 0 is gone, so the "never pruned, read it
    // all" path does not apply and the tap must position at the end.
    let index = engine.get_index("edge-logs").unwrap();
    index.flush().await.unwrap();

    let mut config = engine.wal_tap.config();
    config.indices = vec!["edge-logs".into()];
    engine.wal_tap.set_config(config);
    engine.wal_tap.tick(&engine).await;

    let head = index.current_seq_no().saturating_sub(1);
    assert!(
        head > 0,
        "the index must actually have history for this to mean anything"
    );
    let shipped = engine
        .wal_tap
        .stats()
        .get("edge-logs")
        .map(|s| s.last_shipped_seq)
        .unwrap_or(0);
    assert!(
        shipped >= head,
        "a tap positioned at the end must report zero lag, not the whole \
         history it never intended to ship (head {head}, watermark {shipped})"
    );
}
