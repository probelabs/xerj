//! Issue #405 (split out of #370): the scroll snapshot cap
//! ([`SCROLL_SNAPSHOT_MAX_HITS`]) was enforced against the SUMMED total on
//! `POST /{index}/_search?scroll=` but only PER INDEX on the alias route
//! `POST /{index}/_search_scroll?scroll=`.
//!
//! `search_with_scroll` (the alias route's handler) fetches up to
//! `SCROLL_SNAPSHOT_MAX_HITS` hits from EACH resolved index separately, so an
//! N-index request could pin `N * SCROLL_SNAPSHOT_MAX_HITS` hits into one
//! scroll context — the exact silently-truncated-export failure mode issue
//! #198 exists to refuse — without ever tripping the old per-index check,
//! since no single index alone exceeded the cap.
//!
//! Mirrors the fixture style of `scroll_snapshot_cap_is_documented.rs`
//! (issue #370/#198): an under-cap control so the test cannot pass vacuously,
//! then the real over-cap repro from #405 — two indices, each under the cap
//! individually, whose SUM exceeds it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use xerj_api::es_compat::SCROLL_SNAPSHOT_MAX_HITS;

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

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(req).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn seed(app: &axum::Router, index: &str, docs: usize) {
    let (st, body) = send(
        app,
        Request::builder()
            .method("PUT")
            .uri(format!("/{index}"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"mappings": {"properties": {"n": {"type": "long"}}}}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create {index}: {body}");

    let mut ndjson = String::with_capacity(docs * 32);
    for n in 0..docs {
        ndjson.push_str(&format!(
            "{{\"index\":{{\"_id\":\"{n}\"}}}}\n{{\"n\":{n}}}\n"
        ));
    }
    let (st, body) = send(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/{index}/_bulk"))
            .header("content-type", "application/x-ndjson")
            .body(Body::from(ndjson))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "bulk {index}: {body}");
    assert_eq!(
        body["errors"],
        json!(false),
        "bulk seed reported item errors for {index}: {body}"
    );

    let (st, body) = send(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/{index}/_refresh"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "refresh {index}: {body}");
}

async fn open_alias_scroll(
    app: &axum::Router,
    index_spec: &str,
    size: usize,
) -> (StatusCode, Value) {
    send(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/{index_spec}/_search_scroll?scroll=1m"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"size": size, "query": {"match_all": {}}}).to_string(),
            ))
            .unwrap(),
    )
    .await
}

/// Two indices, each individually UNDER the cap, whose combined total
/// exceeds it — the exact shape #405 reports (2 x 6,000 against a 10,000
/// cap). The old per-index check never fired here: neither `idx.search()`
/// call alone hit `SCROLL_SNAPSHOT_MAX_HITS`, so `scroll_truncated` stayed
/// false while the context silently pinned all 12,000 hits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alias_route_refuses_a_summed_total_over_the_cap() {
    let (app, _dir) = app().await;
    let per_index = SCROLL_SNAPSHOT_MAX_HITS / 2 + 1; // 5,001 each -> 10,002 summed
    seed(&app, "sa", per_index).await;
    seed(&app, "sb", per_index).await;

    let (status, body) = open_alias_scroll(&app, "sa,sb", 100).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a {}-document combined scroll (2 x {per_index}, each under the {SCROLL_SNAPSHOT_MAX_HITS} \
         cap individually) must be refused on the alias route exactly as it already is on \
         `_search?scroll=`, not silently pinned into an under-reporting context: {body}",
        per_index * 2
    );
    let reason = body["error"]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("400 has no error.reason: {body}"));
    assert!(
        reason.contains(&(per_index * 2).to_string()),
        "the refusal must name the true summed total ({}), not a per-index one: {reason}",
        per_index * 2
    );
}

/// Control: the same two-index request, safely under the cap on each side
/// AND in aggregate, must still succeed — without this, a tree that 400s
/// every multi-index alias scroll would pass the assertion above vacuously.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alias_route_multi_index_under_cap_still_scrolls() {
    let (app, _dir) = app().await;
    seed(&app, "sa", 128).await;
    seed(&app, "sb", 128).await;

    let (status, body) = open_alias_scroll(&app, "sa,sb", 50).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a 256-document combined scroll is well under the {SCROLL_SNAPSHOT_MAX_HITS} cap \
         and must succeed: {body}"
    );
    assert!(
        body["_scroll_id"].is_string(),
        "under-cap alias scroll returned no _scroll_id: {body}"
    );
    assert_eq!(
        body["hits"]["total"]["value"].as_u64(),
        Some(256),
        "under-cap alias scroll reported the wrong total: {body}"
    );
}

/// A single index alone over the cap must still be refused on the alias
/// route (the case the OLD per-index check already caught correctly) — this
/// fix must not regress it while closing the summed-total gap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alias_route_still_refuses_a_single_index_over_the_cap() {
    let (app, _dir) = app().await;
    let over = SCROLL_SNAPSHOT_MAX_HITS + 1;
    seed(&app, "solo", over).await;

    let (status, body) = open_alias_scroll(&app, "solo", 100).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a single {over}-document index alone over the cap must still be refused: {body}"
    );
}
