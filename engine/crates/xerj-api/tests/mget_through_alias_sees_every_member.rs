//! Issue #451 (failure class B): `_mget` through a multi-index alias resolves
//! the selector with `Engine::get_index`, which collapses an alias to
//! `aliased.first()` — one member. A document that lives in any other member of
//! the alias is reported `found: false`, silently invisible.
//!
//! This is the read-side twin of #450 (the by-query write path) and #433 (the
//! `_search`/`_count` read paths, already fixed). `_explain`, `_segments`,
//! `_cat/segments`, `_eql/search`, `_async_search` and `_flush` share the same
//! `get_index` cause; `_mget` is the clearest to pin because "the id exists but
//! the alias says it doesn't" is an unambiguous wrong answer.
//!
//! The proper fix (#451) is a single selector resolver returning
//! `(concrete_indices, alias_filters)` that every one of these endpoints calls,
//! not a per-site alias branch. This test is what that resolver must satisfy on
//! the `_mget` path.
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

/// A doc in each member; an `_mget` through the alias must find both, not just
/// the first member's.
#[tokio::test]
async fn mget_through_a_multi_index_alias_finds_docs_in_every_member() {
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

    let (status, body) = call(
        &app,
        "POST",
        "/tri/_mget",
        json!({"docs": [{"_id": "1"}, {"_id": "2"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "_mget status: {body}");

    let docs = body
        .get("docs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(docs.len(), 2, "expected two doc responses: {body}");

    // Both ids exist — one per member. Collapsing the alias to its first member
    // makes the other member's doc report found:false (#451 class B).
    for (i, want_id) in ["1", "2"].iter().enumerate() {
        let found = docs[i]
            .get("found")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert!(
            found,
            "_mget through the alias must find doc {want_id} (it lives in a real member), \
             not report found:false because the alias collapsed to its first member (#451): {body}"
        );
    }
}
