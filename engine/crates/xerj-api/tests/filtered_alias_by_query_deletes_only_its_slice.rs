//! Issue #553 (fast-follow to #450): the #450 fix ANDs an alias's `filter` into
//! the by-query so a `_delete_by_query` through a FILTERED alias removes only
//! the slice the filter names — never the whole backing index (the #524
//! over-delete hazard). The 3-skeptic verification of #552 confirmed this is
//! correct by code inspection but flagged that no test EXERCISES it. This is
//! that test: it fails loudly if the alias filter is ever dropped and a filtered
//! by-query starts draining out-of-slice documents.
//!
//! Two members, each holding one in-slice (`status:active`) and one out-of-slice
//! (`status:archived`) document, behind a filtered alias. A `match_all`
//! `_delete_by_query` through the alias must delete exactly the two active docs
//! and leave the two archived docs untouched.
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

async fn count(app: &axum::Router, index: &str) -> u64 {
    let (_, body) = call(app, "GET", &format!("/{index}/_count"), Value::Null).await;
    body.get("count")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX)
}

/// A `_delete_by_query` through a FILTERED multi-index alias deletes only the
/// in-slice documents, never the whole backing index (#553 / #524).
#[tokio::test]
async fn delete_by_query_through_a_filtered_alias_deletes_only_the_slice() {
    let (app, _dir) = app().await;

    // Two members; each has one active (in-slice) and one archived (out-of-slice) doc.
    for member in ["idx-a", "idx-b"] {
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/{member}"),
            json!({"mappings": {"properties": {"status": {"type": "keyword"}}}}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "create {member}");
        for (id, status) in [("active", "active"), ("archived", "archived")] {
            let (st, _) = call(
                &app,
                "PUT",
                &format!("/{member}/_doc/{id}"),
                json!({"status": status}),
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "index {id} into {member}");
        }
        let (st, _) = call(&app, "POST", &format!("/{member}/_refresh"), Value::Null).await;
        assert_eq!(st, StatusCode::OK, "refresh {member}");
    }

    // Filtered alias 'hot' over both members: only status=active is in-slice.
    let the_filter = json!({ "term": { "status": "active" } });
    for member in ["idx-a", "idx-b"] {
        let (st, body) = call(
            &app,
            "PUT",
            &format!("/{member}/_alias/hot"),
            json!({ "filter": the_filter }),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::OK,
            "create filtered alias on {member}: {body}"
        );
    }

    // A match_all delete through the filtered alias must touch ONLY the slice.
    let (status, body) = call(
        &app,
        "POST",
        "/hot/_delete_by_query",
        json!({"query": {"match_all": {}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "_delete_by_query status: {body}");
    assert_eq!(
        body.get("deleted").and_then(Value::as_u64),
        Some(2),
        "a match_all delete through a filtered alias must delete only the two in-slice \
         (active) docs, not the whole backing index (#553/#524): {body}"
    );

    // The archived (out-of-slice) doc in each member must SURVIVE — the alias
    // filter is what stands between "delete my slice" and "empty my index".
    for member in ["idx-a", "idx-b"] {
        assert_eq!(
            count(&app, member).await,
            1,
            "member {member} must keep its out-of-slice (archived) doc after a filtered-alias \
             delete — over-deleting past the filter is the #524 data-loss hazard: {member}"
        );
    }
}
