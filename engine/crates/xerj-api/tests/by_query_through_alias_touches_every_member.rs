//! Issue #450: `_delete_by_query` (and `_update_by_query`) through a
//! multi-index alias silently operate on only the FIRST member and report a
//! complete success.
//!
//! Both handlers begin with `Engine::get_index(&index)`, which resolves an
//! alias to `aliased.first()` — one member. A destructive by-query against a
//! 3-member alias therefore deletes a third of the corpus and answers
//! `{"deleted": N, "total": N}` with no partial-failure flag: the counts are
//! internally consistent and a caller cannot tell it from a correct run. This
//! is strictly worse than the read-path truncation of #433/#449 — a short read
//! is recoverable, a short delete leaves the caller believing the operation
//! completed.
//!
//! The read paths were taught to fan an alias out to every member in #433; the
//! two by-query WRITE paths were carved out as this separate issue. These
//! tests go through the real HTTP routes, the only place the selector is
//! resolved.
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

const MEMBERS: [&str; 3] = ["idx-a", "idx-b", "idx-c"];
const PER_INDEX: usize = 4;

/// Three indices, `PER_INDEX` docs each, all behind the alias `tri`.
async fn seed(app: &axum::Router) {
    for member in MEMBERS {
        let (st, _) = call(
            app,
            "PUT",
            &format!("/{member}"),
            json!({"mappings": {"properties": {"v": {"type": "long"}, "home": {"type": "keyword"}}}}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "create {member}");
        for i in 0..PER_INDEX {
            let (st, _) = call(
                app,
                "POST",
                &format!("/{member}/_doc/{i}"),
                json!({"v": i, "home": member}),
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "index doc {i} into {member}");
        }
        let (st, _) = call(app, "POST", &format!("/{member}/_refresh"), Value::Null).await;
        assert_eq!(st, StatusCode::OK, "refresh {member}");
    }
    let (st, _) = call(
        app,
        "POST",
        "/_aliases",
        json!({"actions": [
            {"add": {"index": "idx-a", "alias": "tri"}},
            {"add": {"index": "idx-b", "alias": "tri"}},
            {"add": {"index": "idx-c", "alias": "tri"}}
        ]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create alias tri");
}

async fn count(app: &axum::Router, index: &str) -> u64 {
    let (_, body) = call(app, "GET", &format!("/{index}/_count"), Value::Null).await;
    body.get("count").and_then(Value::as_u64).unwrap_or(u64::MAX)
}

/// The #450 bug: a `_delete_by_query` through a 3-member alias must delete
/// every matching document in every member, not just the first member's share.
#[tokio::test]
async fn delete_by_query_through_a_multi_index_alias_deletes_every_member() {
    let (app, _dir) = app().await;
    seed(&app).await;

    let total = (MEMBERS.len() * PER_INDEX) as u64;
    assert_eq!(
        count(&app, "tri").await,
        total,
        "precondition: the alias must see every member's documents"
    );

    let (status, body) = call(
        &app,
        "POST",
        "/tri/_delete_by_query",
        json!({"query": {"match_all": {}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "_delete_by_query status: {body}");

    // The whole corpus behind the alias matched match_all, so a complete run
    // deletes all of it. The bug reports only the first member's share.
    assert_eq!(
        body.get("deleted").and_then(Value::as_u64),
        Some(total),
        "_delete_by_query through a {}-member alias must delete every member's docs, \
         not just the first member's (#450): {body}",
        MEMBERS.len()
    );

    // And it must actually be gone on disk in every member, not merely counted.
    for member in MEMBERS {
        assert_eq!(
            count(&app, member).await,
            0,
            "member {member} still holds documents after _delete_by_query through the alias (#450)"
        );
    }
}
