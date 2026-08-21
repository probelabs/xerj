//! A `semantic` query naming a field that is not a `semantic_text` (a plain
//! text/keyword field, or absent) must fail with a `400`, not silently degrade
//! — issue #530, the symmetric sibling of the `knn` case (#498).
//!
//! With no active embedding backend, the built-in lexical embedder only
//! auto-embeds `semantic_text` fields; a `semantic` query against anything else
//! has no comparable stored vector to match, so it must be rejected loudly.
//!
//! Elasticsearch is referenced for wire semantics only; no code from it is
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

async fn json_req(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A `semantic` query on a plain text field (not `semantic_text`) must 400.
#[tokio::test]
async fn semantic_on_text_field_is_400() {
    let (app, _dir) = app().await;
    let (status, body) = json_req(
        &app,
        "PUT",
        "/notes",
        json!({ "mappings": { "properties": {
            "title": { "type": "text" },
            "tag": { "type": "keyword" }
        } } }),
    )
    .await;
    assert!(status.is_success(), "create index: {status} {body}");
    let (_s, _b) = json_req(
        &app,
        "POST",
        "/notes/_doc/1",
        json!({ "title": "hello world", "tag": "a" }),
    )
    .await;
    let (_s, _b) = json_req(&app, "POST", "/notes/_refresh", json!({})).await;

    let (status, body) = json_req(
        &app,
        "POST",
        "/notes/_search",
        json!({ "query": { "semantic": { "field": "title", "query": "greetings" } } }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "semantic on a non-semantic_text field must be 400 (#530): got {status} {body}"
    );
    assert!(
        body.pointer("/error/reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("title"),
        "the 400 must name the field (#530): {body}"
    );
}

/// A `semantic` query on a field absent from the mapping must 400.
#[tokio::test]
async fn semantic_on_absent_field_is_400() {
    let (app, _dir) = app().await;
    let (status, body) = json_req(
        &app,
        "PUT",
        "/notes",
        json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
    )
    .await;
    assert!(status.is_success(), "create index: {status} {body}");

    let (status, body) = json_req(
        &app,
        "POST",
        "/notes/_search",
        json!({ "query": { "semantic": { "field": "nonexistent_vec", "query": "greetings" } } }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "semantic on an absent field must be 400 (#530): got {status} {body}"
    );
    assert!(
        body.pointer("/error/reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("nonexistent_vec"),
        "the 400 must name the field (#530): {body}"
    );
}
