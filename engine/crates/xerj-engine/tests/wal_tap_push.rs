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
    /// `_id` → highest `version` this target currently holds, maintained only
    /// in [`TargetSemantics::ExternalVersioning`].
    versions: HashMap<String, u64>,
}

/// How the stub answers a `_bulk`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetSemantics {
    /// `{"took":1,"errors":false,"items":[]}` — everything is accepted.
    ///
    /// Fine for "did the bytes arrive", useless for the design's central
    /// safety claim, because it makes the whole `errors == true` branch of
    /// `send_bulk` dead code.
    AlwaysAccept,
    /// Elasticsearch's `version_type: external` rule, which is what the tap's
    /// at-least-once story rests on: an action applies only if its `version`
    /// is **strictly greater** than the one the target holds for that `_id`;
    /// otherwise the item comes back `version_conflict_engine_exception` and
    /// the stored document is untouched.
    ///
    /// This is the target the feature is designed against, and until now
    /// nothing in the tree implemented it —
    /// `scripts/verify-wal-tap.sh` runs two real xerj nodes, and xerj's own
    /// `_bulk` ignores per-action `version` / `version_type` (only the
    /// single-doc path honours them, `es_compat.rs:2990`), so even the
    /// end-to-end script exercised precisely the target that ignores the
    /// mechanism.
    ExternalVersioning,
    /// External versioning against a target that already holds a *higher*
    /// version for every `_id` — a second writer got there first. Every action
    /// is a rejected no-op, and the tap must say so rather than report a
    /// healthy zero-lag stream.
    ExternalVersioningAheadBy(u64),
}

struct StubTarget {
    url: String,
    received: Arc<Mutex<Received>>,
    _task: tokio::task::JoinHandle<()>,
}

impl StubTarget {
    async fn start() -> Self {
        Self::start_with(TargetSemantics::AlwaysAccept).await
    }

    async fn start_with(semantics: TargetSemantics) -> Self {
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
                    let _ = serve_one(stream, sink, semantics).await;
                });
            }
        });
        Self {
            url,
            received,
            _task: task,
        }
    }

    /// The document set the target actually holds, `_id` → version. Only
    /// meaningful for the external-versioning semantics.
    fn stored_versions(&self) -> HashMap<String, u64> {
        self.received.lock().unwrap().versions.clone()
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

/// Apply one `_bulk` NDJSON body under [`TargetSemantics::ExternalVersioning`]
/// and return the ES-shaped response body.
///
/// Deliberately implements the ES rule and not a friendlier one: an `index`
/// action with `version_type: external` applies iff `version > stored`, a
/// `delete` the same, and a rejection is
/// `{"status":409,"error":{"type":"version_conflict_engine_exception", …}}`
/// with the stored document left alone.
fn apply_external_versioning(body: &str, held: &mut HashMap<String, u64>, ahead_by: u64) -> String {
    let mut items = Vec::new();
    let mut errors = false;
    let mut lines = body.lines();
    while let Some(meta_line) = lines.next() {
        if meta_line.trim().is_empty() {
            continue;
        }
        let meta: Value = serde_json::from_str(meta_line).expect("action line");
        let (action, spec) = meta
            .as_object()
            .and_then(|o| o.iter().next())
            .map(|(k, v)| (k.clone(), v.clone()))
            .expect("action object");
        if action != "delete" {
            // Consume the source line.
            let _ = lines.next().expect("source line");
        }
        let id = spec["_id"].as_str().unwrap_or_default().to_string();
        let index = spec["_index"].as_str().unwrap_or_default().to_string();
        let incoming = spec["version"].as_u64().unwrap_or(0);
        assert_eq!(
            spec["version_type"], "external",
            "the tap must send external versioning or none of this applies"
        );

        // `ahead_by > 0` models a second writer that got to this target
        // first: seed a version the tap can never beat.
        let stored = *held.entry(id.clone()).or_insert(incoming + ahead_by);
        if incoming > stored {
            held.insert(id.clone(), incoming);
            let result = if action == "delete" {
                "deleted"
            } else {
                "created"
            };
            items.push(json!({action: {
                "_index": index, "_id": id, "_version": incoming,
                "result": result,
                "status": 201,
            }}));
        } else {
            errors = true;
            items.push(json!({action: {
                "_index": index, "_id": id, "_version": stored, "status": 409,
                "error": {
                    "type": "version_conflict_engine_exception",
                    "reason": format!(
                        "[{id}]: version conflict, current version [{stored}] is higher \
                         or equal to the one provided [{incoming}]"),
                },
            }}));
        }
    }
    json!({"took": 1, "errors": errors, "items": items}).to_string()
}

/// Read one HTTP request, record the body, answer with an ES-shaped `_bulk`
/// response.
async fn serve_one(
    mut stream: TcpStream,
    sink: Arc<Mutex<Received>>,
    semantics: TargetSemantics,
) -> std::io::Result<()> {
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
        let payload = {
            let mut guard = sink.lock().unwrap();
            guard.last_auth = auth;
            let response = match semantics {
                TargetSemantics::AlwaysAccept => {
                    r#"{"took":1,"errors":false,"items":[]}"#.to_string()
                }
                TargetSemantics::ExternalVersioning => {
                    let mut held = std::mem::take(&mut guard.versions);
                    let out = apply_external_versioning(&body, &mut held, 0);
                    guard.versions = held;
                    out
                }
                TargetSemantics::ExternalVersioningAheadBy(n) => {
                    let mut held = std::mem::take(&mut guard.versions);
                    let out = apply_external_versioning(&body, &mut held, n);
                    guard.versions = held;
                    out
                }
            };
            guard.bodies.push(body);
            response
        };
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        stream.write_all(resp.as_bytes()).await?;
        stream.write_all(payload.as_bytes()).await?;
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
///
/// **No `tick()` between the delete and the recreate.** That is the whole
/// point: the default poll interval is 500 ms, so in production a
/// `DELETE` + `PUT` is essentially never observed as an absence, and a test
/// that inserts a poll there proves only that the hygiene path works. The
/// deletion itself has to drop the cursor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recreating_an_index_under_the_same_name_does_not_stall_the_tap() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-logs"]));

    // Enough documents that the surviving cursor is a long way into the file:
    // a one-document cursor could land back inside the recreated stream by
    // luck and hide the bug.
    let ids: Vec<String> = (1..=40).map(|i| format!("old-{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    write_docs(&engine, "edge-logs", &refs).await;
    engine.wal_tap.tick(&engine).await;
    assert_eq!(target.indexed_ids("edge-logs").len(), 40);
    let stale_offset = engine.wal_tap.cursors()["edge-logs"]
        .values()
        .map(|c| c.offset)
        .max()
        .unwrap_or(0);
    assert!(
        stale_offset > 16,
        "the cursor must actually be somewhere inside the WAL for this to mean anything"
    );

    // Delete and recreate WITHOUT a poll in between — one 500 ms interval.
    engine.delete_index("edge-logs").await.unwrap();
    assert!(
        !engine.wal_tap.cursors().contains_key("edge-logs"),
        "the deletion itself must drop the cursor: a poll would not see this"
    );
    write_docs(&engine, "edge-logs", &["new-1", "new-2"]).await;

    engine.wal_tap.tick(&engine).await;

    let shipped = target.indexed_ids("edge-logs");
    assert!(
        shipped.contains(&"new-1".to_string()) && shipped.contains(&"new-2".to_string()),
        "every document of the recreated index must ship — the stale byte offset {stale_offset} \
         either skipped them silently or wedged the shard: {shipped:?}"
    );
    let stats = engine.wal_tap.stats();
    let edge = stats.get("edge-logs").expect("stats for edge-logs");
    assert!(
        edge.last_error.is_none() && edge.healthy(),
        "the recreated index must be healthy, got {:?}",
        edge.last_error
    );
}

/// The storage-level half of the same failure, for a delete this process never
/// saw — the node was down when the index was dropped and recreated, so
/// `forget_index` never ran and the persisted cursor outlives its stream.
///
/// A WAL generation file is append-only, so an offset past its end can only
/// mean "different stream". Before this check the reader clamped the offset to
/// EOF and reported a clean drain: every document skipped, `gaps` still 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cursor_past_the_end_of_its_generation_is_a_gap_not_a_silent_skip() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-logs"]));

    let ids: Vec<String> = (1..=40).map(|i| format!("old-{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    write_docs(&engine, "edge-logs", &refs).await;
    engine.wal_tap.tick(&engine).await;
    assert_eq!(target.indexed_ids("edge-logs").len(), 40);
    let cursors = engine.wal_tap.cursors()["edge-logs"].clone();

    // Recreate the index, then put the OLD cursor back — exactly the state a
    // restart finds after an offline delete/recreate.
    engine.delete_index("edge-logs").await.unwrap();
    write_docs(&engine, "edge-logs", &["new-1", "new-2"]).await;
    engine
        .wal_tap
        .restore_cursors_for_test("edge-logs", cursors);

    engine.wal_tap.tick(&engine).await;

    let shipped = target.indexed_ids("edge-logs");
    assert!(
        shipped.contains(&"new-1".to_string()) && shipped.contains(&"new-2".to_string()),
        "a cursor past EOF must restart the stream, not clamp to EOF and skip: {shipped:?}"
    );
    assert!(
        engine.wal_tap.stats()["edge-logs"].gaps > 0,
        "…and it must be REPORTED as a gap, not absorbed silently"
    );
}

/// The design's central safety claim, which had zero coverage: at-least-once
/// delivery is only safe because `version_type: external` makes a redelivery
/// converge instead of resurrecting an older document.
///
/// The `AlwaysAccept` stub every other test uses answers a hardcoded
/// `{"errors":false,"items":[]}`, which makes the entire `errors == true`
/// branch of `send_bulk` dead code — so this one implements the actual ES
/// external-versioning rule and forces a real redelivery through it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redelivery_converges_at_a_target_that_honours_external_versioning() {
    let target = StubTarget::start_with(TargetSemantics::ExternalVersioning).await;
    let dir = TempDir::new().unwrap();
    let mut tap = tap_config(&target.url, &["edge-logs"]);
    // Two documents per request, so a failure part-way through a poll leaves
    // earlier chunks landed and forces the retry to re-send them.
    tap.max_batch_bytes = 200;
    tap.max_retry_backoff_secs = 1;
    let engine = engine_with_tap(&dir, tap);

    write_docs(&engine, "edge-logs", &["a", "b", "c", "d"]).await;
    engine.wal_tap.tick(&engine).await;
    let after_first = target.stored_versions();
    assert_eq!(
        after_first.len(),
        4,
        "all four documents must reach the target: {after_first:?}"
    );

    // Force the redelivery the at-least-once contract permits: rewind the
    // cursor to the start of the stream and poll again. Every action is
    // re-sent with the same seq_no as its external version.
    engine.wal_tap.rewind_for_test("edge-logs");
    engine.wal_tap.tick(&engine).await;

    let after_redelivery = target.stored_versions();
    assert_eq!(
        after_redelivery, after_first,
        "a redelivery must be a no-op at the target, not a resurrection: {after_redelivery:?}"
    );

    let stats = engine.wal_tap.stats();
    let edge = stats.get("edge-logs").expect("stats");
    assert!(
        edge.version_conflicts >= 4,
        "the redelivered actions must be REPORTED as conflicts, not silently counted as \
         shipped (got {})",
        edge.version_conflicts
    );
    assert_eq!(
        edge.item_failures, 0,
        "a version conflict is not a rejection — it must never be counted as one"
    );
}

/// A target that holds a higher version for every `_id` — a second writer got
/// there first — turns every action into a rejected no-op. `_stats` used to
/// call that healthy with `lag_seq: 0` and a climbing `docs_shipped`, because
/// the counters came from what was RENDERED rather than from what the target
/// said it took.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_target_that_rejects_every_action_is_reported_unhealthy_with_real_lag() {
    let target = StubTarget::start_with(TargetSemantics::ExternalVersioningAheadBy(1_000)).await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-logs"]));

    // Enough polls to cross the "this is not one absorbed redelivery" line.
    for round in 0..5 {
        write_docs(&engine, "edge-logs", &[&format!("d{round}")]).await;
        engine.wal_tap.tick(&engine).await;
    }

    let stats = engine.wal_tap.stats();
    let edge = stats.get("edge-logs").expect("stats for edge-logs");
    assert!(
        target.request_count() >= 5,
        "the tap must actually have tried: {}",
        target.request_count()
    );
    assert_eq!(
        edge.docs_shipped, 0,
        "nothing was accepted, so nothing may be counted as shipped"
    );
    assert_eq!(
        edge.last_shipped_seq, 0,
        "the watermark must come from ACCEPTED items — it is what lag_seq is computed \
         from, and reporting zero lag while replicating nothing is the whole bug"
    );
    assert!(
        edge.version_conflicts >= 5,
        "the conflicts must be counted: {}",
        edge.version_conflicts
    );
    assert!(
        !edge.healthy(),
        "a target rejecting every action must not read as healthy"
    );
    assert!(
        edge.last_error
            .as_deref()
            .unwrap_or_default()
            .contains("version_conflict"),
        "the operator must be told WHY: {:?}",
        edge.last_error
    );

    let head = engine
        .get_index("edge-logs")
        .unwrap()
        .current_seq_no()
        .saturating_sub(1);
    assert!(
        head > edge.last_shipped_seq,
        "lag must be non-zero (head {head}, watermark {})",
        edge.last_shipped_seq
    );
}

/// `PUT /_xerj/wal_tap` advertises "runtime config, no restart". That implied
/// a durability that did not exist: the value lived only in an in-memory
/// `RwLock`, so a restart reverted the tap to the file config — normally
/// `enabled = false` — while cursors froze and WAL pruning carried on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_configuration_survives_a_restart() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();

    // Boot from a file config with the tap OFF, as an operator's node would be.
    {
        let engine = engine_with_tap(&dir, WalTapConfig::default());
        assert!(!engine.wal_tap.config().enabled);
        engine
            .wal_tap
            .set_config(tap_config(&target.url, &["edge-*"]));
        write_docs(&engine, "edge-logs", &["1"]).await;
        engine.wal_tap.tick(&engine).await;
        assert_eq!(target.indexed_ids("edge-logs"), vec!["1"]);
    }

    // Restart with the same (still-off) file config.
    let engine = engine_with_tap(&dir, WalTapConfig::default());
    let config = engine.wal_tap.config();
    assert!(
        config.enabled && config.indices == vec!["edge-*".to_string()],
        "the configuration set at runtime must outlive the process that took it, got {config:?}"
    );
    assert!(engine.wal_tap.has_runtime_config());

    write_docs(&engine, "edge-logs", &["2"]).await;
    engine.wal_tap.tick(&engine).await;
    assert!(
        target.indexed_ids("edge-logs").contains(&"2".to_string()),
        "…and it must still be shipping after the restart: {:?}",
        target.actions()
    );

    // And the overlay is droppable, or one API call would shadow xerj.toml
    // forever.
    engine.wal_tap.clear_runtime_config(WalTapConfig::default());
    assert!(!engine.wal_tap.config().enabled);
    assert!(!engine.wal_tap.has_runtime_config());
}

/// Cursor state used to be serialised and durably rewritten in full on every
/// cursor advance — once per index per poll, from three call sites in
/// `poll_index` plus `forget_deleted`. At N allowlisted indices that is O(N²)
/// bytes and 2N fsyncs per tick, at a 500 ms default, on a node
/// `index_store.rs:806-813` records reaching 9,382 indices. `indices = ["*"]`
/// walked straight into it.
///
/// The cost of a tick must not scale with the allowlist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_state_is_written_once_per_tick_not_once_per_index() {
    let target = StubTarget::start().await;
    let dir = TempDir::new().unwrap();
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-*"]));

    const INDICES: usize = 12;
    for i in 0..INDICES {
        write_docs(&engine, &format!("edge-{i}"), &["a", "b"]).await;
    }

    let before = engine.wal_tap.persists();
    engine.wal_tap.tick(&engine).await;
    let writes = engine.wal_tap.persists() - before;

    assert!(
        target.request_count() >= INDICES,
        "every index must actually have shipped: {}",
        target.request_count()
    );
    assert_eq!(
        writes, 1,
        "one tick over {INDICES} indices must cost ONE durable state write, not one \
         per index (got {writes})"
    );

    // …and the cursors are still durable: a restart resumes rather than
    // re-ships. Deferring the write may only ever cause a redelivery, which
    // external versioning absorbs; it must never lose a position.
    let shipped_before = target.request_count();
    drop(engine);
    let engine = engine_with_tap(&dir, tap_config(&target.url, &["edge-*"]));
    engine.wal_tap.tick(&engine).await;
    assert_eq!(
        target.request_count(),
        shipped_before,
        "a deferred flush must still be a flush — nothing may be re-shipped after a restart"
    );

    // An idle tick writes nothing at all.
    let idle_before = engine.wal_tap.persists();
    engine.wal_tap.tick(&engine).await;
    assert_eq!(
        engine.wal_tap.persists(),
        idle_before,
        "an idle poll must not rewrite the state file"
    );
}

/// The credential must not be readable back out of the config surface, and
/// `https://user:pass@host` is the spelling that got round the `target_auth`
/// omission — `reqwest` turns URL userinfo into a `Basic` header.
#[test]
fn a_target_url_carrying_credentials_is_refused_and_redacted() {
    assert!(WalTapConfig::check_target_url("https://central:9200").is_ok());
    assert!(WalTapConfig::check_target_url("").is_ok());
    assert!(WalTapConfig::check_target_url("central:9200").is_err());

    let reason = WalTapConfig::check_target_url("https://user:hunter2@central:9200")
        .expect_err("userinfo must be refused");
    assert!(
        reason.contains("target_auth"),
        "the error must point at the field that exists for this: {reason}"
    );
    // A path containing `@` is not userinfo and must still be accepted.
    assert!(WalTapConfig::check_target_url("https://central:9200/a@b").is_ok());

    let config = WalTapConfig {
        target_url: "https://user:hunter2@central:9200".into(),
        ..Default::default()
    };
    let shown = config.redacted_target_url();
    assert!(
        !shown.contains("hunter2") && shown.contains("central:9200"),
        "a URL that reached the process another way must still not be readable back: {shown}"
    );

    // A whole-node config carrying one refuses to start.
    let mut whole = Config::default();
    whole.wal_tap.target_url = "http://user:pw@central:9200".into();
    assert!(
        whole.validate().is_err(),
        "a credential-bearing target_url must fail startup, not reach the boot log"
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
