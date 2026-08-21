//! Issue #451 (failure class B): `GET /_cat/segments/{index}` listed only
//! `[index]` and resolved it with `Engine::get_index`, which collapses an alias
//! to `aliased.first()`. So `_cat/segments` over a multi-index alias reported a
//! single row (the alias name, the first member's stats) instead of one row per
//! member — and a glob selector matched nothing.
//!
//! Migrated onto the shared `resolve_selector_with_filters` resolver (#451),
//! which expands `_all` / `*` / globs / aliases / comma-lists uniformly.
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

/// `_cat/segments` over a 2-member alias must return a row per member, naming
/// the concrete members — not one row under the alias name.
#[tokio::test]
async fn cat_segments_through_a_multi_index_alias_lists_every_member() {
    let (app, _dir) = app().await;

    for (member, id) in [("idx-a", "1"), ("idx-b", "2")] {
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/{member}"),
            json!({"mappings": {"properties": {"v": {"type": "keyword"}}}}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "create {member}");
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/{member}/_doc/{id}"),
            json!({"v": member}),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "index doc {id} into {member}");
        let (st, _) = call(&app, "POST", &format!("/{member}/_refresh"), Value::Null).await;
        assert_eq!(st, StatusCode::OK, "refresh {member}");
    }
    let (st, _) = call(
        &app,
        "POST",
        "/_aliases",
        json!({"actions": [
            {"add": {"index": "idx-a", "alias": "tri"}},
            {"add": {"index": "idx-b", "alias": "tri"}}
        ]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create alias tri");

    let (status, body) = call(&app, "GET", "/_cat/segments/tri?format=json", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "_cat/segments status: {body}");

    let rows = body.as_array().cloned().unwrap_or_default();
    let mut indices: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("index").and_then(Value::as_str).map(String::from))
        .collect();
    indices.sort();
    assert_eq!(
        indices,
        vec!["idx-a".to_string(), "idx-b".to_string()],
        "_cat/segments through the alias must list a row per concrete member, not one \
         row under the alias name (#451): {body}"
    );
}
