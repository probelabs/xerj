//! Issue #524: an alias's `filter` must survive a restart.
//!
//! The alias→members topology is persisted to `aliases.json`, but each alias's
//! `filter` lived only in the in-memory `index_alias_metadata` map and was
//! never written to disk. So a filtered alias came back after a restart with
//! its members but no filter: `_search`/`_count` through it silently returned
//! the whole backing corpus instead of the slice the filter names, and once
//! the by-query paths honour the filter (#453) a `_delete_by_query` through
//! such an alias would empty the whole backing index. The fix persists the
//! metadata to a `alias_metadata.json` sidecar and reloads it on boot.
//!
//! This test creates a filtered alias, drops the engine, opens a fresh one on
//! the same data dir, and asserts the filter is still there. Before the fix it
//! comes back empty.
//!
//! Elasticsearch is referenced for wire semantics only; no ES code is
//! reproduced here.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

fn config_for(dir: &std::path::Path) -> xerj_common::config::Config {
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    config
}

/// Build a fresh app over `dir`. Calling this after the previous app has been
/// dropped is a restart: nothing is handed over in memory, the second boot
/// sees only what reached the disk.
fn app_over(dir: &std::path::Path) -> axum::Router {
    let config = config_for(dir);
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);
    xerj_api::router::build_es_compat_router(state)
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(req).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn put_json(path: &str, body: Value) -> Request<Body> {
    Request::put(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn get(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).expect("request")
}

#[tokio::test]
async fn an_alias_filter_survives_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let the_filter = json!({ "term": { "tier": "gold" } });

    // ── Session 1: create an index and a FILTERED alias over it. ──
    {
        let app = app_over(dir.path());

        let (st, body) = send(&app, put_json("/orders", json!({}))).await;
        assert_eq!(st, StatusCode::OK, "create index: {body}");

        let (st, body) = send(
            &app,
            put_json("/orders/_alias/gold", json!({ "filter": the_filter })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "create filtered alias: {body}");

        // Sanity BEFORE the restart: the filter is visible, so a failure after
        // the restart is unambiguously a persistence failure, not a write that
        // never landed.
        let (st, body) = send(&app, get("/_alias/gold")).await;
        assert_eq!(st, StatusCode::OK, "read alias before restart: {body}");
        assert_eq!(
            body.pointer("/orders/aliases/gold/filter"),
            Some(&the_filter),
            "the filter must be present before the restart: {body}"
        );
    } // app — and with it the engine and its node lock — dropped here.

    // ── Session 2: a fresh engine on the same dir sees only what reached disk. ──
    let app = app_over(dir.path());
    let (st, body) = send(&app, get("/_alias/gold")).await;
    assert_eq!(st, StatusCode::OK, "read alias after restart: {body}");
    assert_eq!(
        body.pointer("/orders/aliases/gold/filter"),
        Some(&the_filter),
        "the alias filter must survive a restart, not come back empty: {body}"
    );
}
