//! Issue #506(1): collapse `inner_hits` fabricated `_seq_no` as the array index
//! (`i as u64`) and re-read `_version` live at render time, instead of carrying
//! the doc's snapshotted sequence number / version. On a collapsed scroll that
//! hands an inner hit a positional `_seq_no` (0 for each group's first member)
//! and a live `_version` beside a snapshot `_source` — the torn-read class #499
//! fixed for top-level hits, on the inner_hits path.
//!
//! This test pins the real seq_no: with `inner_hits` sorted so the array order
//! is the reverse of the insertion (seq_no) order, the first inner hit — the
//! last-indexed document — must carry a HIGHER `_seq_no` than the last inner
//! hit, which a positional index (0, 1, 2) inverts.
//!
//! Elasticsearch is referenced for wire semantics only; no ES code is
//! reproduced here.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.path().to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);
    (xerj_api::router::build_es_compat_router(state), dir)
}

async fn call(app: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    let body = if body.is_null() {
        Body::empty()
    } else {
        req = req.header("content-type", "application/json");
        Body::from(body.to_string())
    };
    let response = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Collapse `inner_hits` must carry each member's real snapshot `_seq_no`, not
/// the array index (#506).
#[tokio::test]
async fn collapse_inner_hits_seq_no_is_the_real_snapshot_value() {
    let (app, _dir) = app().await;
    let (st, _) = call(
        &app,
        "PUT",
        "/docs",
        json!({"mappings": {"properties": {
            "grp": { "type": "keyword" },
            "v": { "type": "long" }
        }}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create index");

    // One collapse group ("A"), three members indexed in ascending v — so
    // insertion order (== seq_no order) is v=10, v=20, v=30.
    for (id, v) in [("a", 10), ("b", 20), ("c", 30)] {
        let (st, _) = call(
            &app,
            "POST",
            &format!("/docs/_doc/{id}"),
            json!({ "grp": "A", "v": v }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "index {id}");
    }
    let (_s, _b) = call(&app, "POST", "/docs/_refresh", Value::Null).await;

    // Collapse by grp; inner_hits sorted v DESC -> array order [v=30, v=20, v=10]
    // = reverse of insertion (seq_no) order. Request seq_no on the inner hits.
    let (status, body) = call(
        &app,
        "POST",
        "/docs/_search",
        json!({
            "query": { "match_all": {} },
            "collapse": { "field": "grp", "inner_hits": {
                "name": "members", "size": 10,
                "sort": [{ "v": "desc" }],
                "seq_no_primary_term": true
            } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "collapse search: {body}");

    let inner = body
        .pointer("/hits/hits/0/inner_hits/members/hits/hits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(inner.len(), 3, "expected 3 inner hits: {body}");

    let seq_nos: Vec<i64> = inner
        .iter()
        .map(|h| h.get("_seq_no").and_then(Value::as_i64).unwrap_or(-999))
        .collect();

    // The first inner hit (v=30, indexed LAST) must have a higher seq_no than
    // the last inner hit (v=10, indexed FIRST). A positional index emits [0,1,2],
    // inverting this.
    assert!(
        seq_nos[0] > seq_nos[2],
        "collapse inner_hits _seq_no is positional (array index), not the real snapshot \
         sequence number: the last-indexed member must have the highest seq_no (#506). \
         got _seq_no sequence {seq_nos:?}: {body}"
    );
    // And it must not be the tell-tale positional [0, 1, 2].
    assert_ne!(
        seq_nos,
        vec![0, 1, 2],
        "collapse inner_hits _seq_no is the array index [0,1,2], not the doc's seq_no (#506): {body}"
    );
}
