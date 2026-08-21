//! Issue #451 (failure class B): `GET /{index}/_segments` resolved the path
//! index with `Engine::get_index`, which collapses an alias to
//! `aliased.first()`. So `_segments` over a multi-index alias returned a single
//! `indices` entry (keyed by the alias name, the first member's segments)
//! instead of one entry per concrete member.
//!
//! Migrated onto the shared `resolve_selector_with_filters` resolver (#451).
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

/// `_segments` over a 2-member alias must return one `indices` entry per member,
/// keyed by the concrete member names — not one entry under the alias name.
#[tokio::test]
async fn index_segments_through_a_multi_index_alias_lists_every_member() {
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

    let (status, body) = call(&app, "GET", "/tri/_segments", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "_segments status: {body}");

    let indices = body
        .pointer("/indices")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut keys: Vec<String> = indices.keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["idx-a".to_string(), "idx-b".to_string()],
        "_segments through the alias must list an entry per concrete member, not one \
         entry under the alias name (#451): {body}"
    );
}
