//! Issue #451 (failure class B): `_explain` resolves the path index with
//! `Engine::get_index`, which collapses an alias to `aliased.first()`. A
//! document that lives in any other member of the alias is reported
//! `matched: false` / "document not found" — the same `get_index` cause as
//! `_mget`, on the explain diagnostic.
//!
//! Migrated onto the shared `resolve_selector_with_filters` resolver (#451):
//! `_explain` now finds the member that actually holds the document and
//! explains against it, reporting the concrete member in `_index`.
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

/// A doc that lives in the alias's SECOND member must still be explained
/// (matched), not reported "document not found" because the alias collapsed to
/// its first member.
#[tokio::test]
async fn explain_through_a_multi_index_alias_finds_a_non_first_member_doc() {
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

    // Explain doc "2" (lives in idx-b, the non-first member) against match_all.
    let (status, body) = call(
        &app,
        "POST",
        "/tri/_explain/2",
        json!({"query": {"match_all": {}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "_explain status: {body}");
    assert_eq!(
        body.get("matched").and_then(Value::as_bool),
        Some(true),
        "_explain through the alias must find doc 2 in idx-b and match it, not report \
         'document not found' because the alias collapsed to its first member (#451): {body}"
    );
    // And it reports the concrete member the doc lives in, not the alias.
    assert_eq!(
        body.get("_index").and_then(Value::as_str),
        Some("idx-b"),
        "_explain must report the concrete member in _index (#451): {body}"
    );
}
