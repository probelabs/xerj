//! Issue #451 (failure class C): `msearch` (and `search_with_scroll`,
//! `msearch_template`, `_cat/count`) resolve an alias to its members and search
//! them directly, but never read `index_alias_metadata`, so an alias's `filter`
//! is dropped. A filtered alias is the standard poor-man's document-level
//! boundary, so these paths return document bodies the alias exists to hide —
//! while `_search` through the same alias honours the filter.
//!
//! This test pins the divergence on `_msearch`: a filtered alias that shows one
//! document via `_search` must show the same one document via `_msearch`, not
//! leak the out-of-slice document.
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

async fn ndjson(app: &axum::Router, path: &str, body: String) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/x-ndjson")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A filtered alias must apply its filter on `_msearch`, not just `_search`.
#[tokio::test]
async fn msearch_through_a_filtered_alias_honours_the_filter() {
    let (app, _dir) = app().await;
    let (st, _) = call(
        &app,
        "PUT",
        "/idx-a",
        json!({"mappings": {"properties": {"status": {"type": "keyword"}}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create index");
    for (id, status) in [("active", "active"), ("archived", "archived")] {
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/idx-a/_doc/{id}"),
            json!({ "status": status }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "index {id}");
    }
    let (_s, _b) = call(&app, "POST", "/idx-a/_refresh", Value::Null).await;

    // Filtered alias: only status=active is in-slice.
    let (st, body) = call(
        &app,
        "PUT",
        "/idx-a/_alias/hot",
        json!({ "filter": { "term": { "status": "active" } } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create filtered alias: {body}");

    // Baseline: `_search` through the alias honours the filter -> 1 hit.
    let (_s, search) = call(
        &app,
        "POST",
        "/hot/_search",
        json!({ "query": { "match_all": {} }, "track_total_hits": true }),
    )
    .await;
    let search_total = search
        .pointer("/hits/total/value")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    assert_eq!(
        search_total, 1,
        "precondition: _search through the filtered alias should see only the in-slice doc: {search}"
    );

    // `_msearch` through the same alias must ALSO honour the filter.
    let nd = format!(
        "{}\n{}\n",
        json!({ "index": "hot" }),
        json!({ "query": { "match_all": {} }, "track_total_hits": true })
    );
    let (status, body) = ndjson(&app, "/_msearch", nd).await;
    assert_eq!(status, StatusCode::OK, "_msearch status: {body}");
    let msearch_total = body
        .pointer("/responses/0/hits/total/value")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    assert_eq!(
        msearch_total, search_total,
        "_msearch through a filtered alias leaked the out-of-slice document — it returned \
         {msearch_total} hits vs {search_total} for _search; the alias filter (a document \
         boundary) was dropped on the msearch path (#451 class C): {body}"
    );
}

/// The scroll path (`_search?scroll=`) must also honour a filtered alias.
#[tokio::test]
async fn scroll_through_a_filtered_alias_honours_the_filter() {
    let (app, _dir) = app().await;
    let (st, _) = call(
        &app,
        "PUT",
        "/idx-a",
        json!({"mappings": {"properties": {"status": {"type": "keyword"}}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create index");
    for (id, status) in [("active", "active"), ("archived", "archived")] {
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/idx-a/_doc/{id}"),
            json!({ "status": status }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "index {id}");
    }
    let (_s, _b) = call(&app, "POST", "/idx-a/_refresh", Value::Null).await;
    let (st, _) = call(
        &app,
        "PUT",
        "/idx-a/_alias/hot",
        json!({ "filter": { "term": { "status": "active" } } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create filtered alias");

    // Open a scroll through the filtered alias.
    let (status, body) = call(
        &app,
        "POST",
        "/hot/_search?scroll=1m",
        json!({ "query": { "match_all": {} }, "track_total_hits": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "scroll open status: {body}");
    let total = body
        .pointer("/hits/total/value")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    let returned = body
        .pointer("/hits/hits")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(usize::MAX);
    assert_eq!(
        total, 1,
        "scroll through a filtered alias must see only the in-slice doc, not leak the \
         out-of-slice document (#451 class C): {body}"
    );
    assert_eq!(
        returned, 1,
        "scroll first page must return only the in-slice doc: {body}"
    );
}
