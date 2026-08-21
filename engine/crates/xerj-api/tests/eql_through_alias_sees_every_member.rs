//! Issue #451 (failure class B, `_eql/search` arm): `eql_search` resolves the
//! path index with `Engine::get_index`, which collapses an alias to
//! `aliased.first()` — one member. So an EQL search through a multi-index alias
//! silently returns only the first member's events, the same silent
//! wrong-answer as `_mget`/`_explain`/`_cat/segments`/`_segments` (fixed in
//! #555). `_eql/search` was left for last because a correct fan-out has to
//! merge the members' event lists.
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

/// An EQL search through a 2-member alias must return matching events from every
/// member, not just the first member's.
#[tokio::test]
async fn eql_search_through_a_multi_index_alias_sees_every_member() {
    let (app, _dir) = app().await;

    for member in ["idx-a", "idx-b"] {
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/{member}"),
            json!({"mappings": {"properties": {"status": {"type": "keyword"}}}}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "create {member}");
        // One matching event per member.
        let (st, _) = call(
            &app,
            "POST",
            &format!("/{member}/_doc/{member}"),
            json!({ "status": "active" }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "index into {member}");
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

    let (status, body) = call(
        &app,
        "POST",
        "/tri/_eql/search",
        json!({ "query": "any where status == \"active\"" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "_eql/search status: {body}");

    let events = body
        .pointer("/hits/events")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(usize::MAX);
    assert_eq!(
        events, 2,
        "_eql/search through a 2-member alias must return the matching event from BOTH \
         members, not just the first member's — the alias collapsed to aliased.first() \
         (#451 _eql arm): {body}"
    );
}
