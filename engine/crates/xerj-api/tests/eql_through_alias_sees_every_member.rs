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

/// An EQL search through a *filtered* alias must honour the filter on EVERY
/// member — a filtered alias is a document boundary (#451 class C, the same
/// leak #559 closed for `_msearch`). The fan-out must not return the members'
/// out-of-slice rows. Regression guard: fanning out unfiltered leaked them.
#[tokio::test]
async fn eql_search_through_a_filtered_alias_honours_the_filter_on_every_member() {
    let (app, _dir) = app().await;

    // idx-a holds an in-slice doc, idx-b holds an out-of-slice doc; both share
    // `host` so the EQL condition matches both and ONLY the alias filter can
    // exclude idx-b's row.
    for (member, status) in [("idx-a", "active"), ("idx-b", "deleted")] {
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/{member}"),
            json!({"mappings": {"properties": {
                "status": {"type": "keyword"}, "host": {"type": "keyword"}
            }}}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "create {member}");
        let (st, _) = call(
            &app,
            "POST",
            &format!("/{member}/_doc/{member}"),
            json!({ "status": status, "host": "h1" }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "index into {member}");
        let (st, _) = call(&app, "POST", &format!("/{member}/_refresh"), Value::Null).await;
        assert_eq!(st, StatusCode::OK, "refresh {member}");
    }

    // Filtered alias `hot` (status==active) over BOTH members.
    for member in ["idx-a", "idx-b"] {
        let (st, body) = call(
            &app,
            "PUT",
            &format!("/{member}/_alias/hot"),
            json!({ "filter": { "term": { "status": "active" } } }),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "filtered alias on {member}: {body}");
    }

    let (status, body) = call(
        &app,
        "POST",
        "/hot/_eql/search",
        json!({ "query": "any where host == \"h1\"" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "_eql/search status: {body}");

    let events = body
        .pointer("/hits/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        events.len(),
        1,
        "_eql/search through a filtered alias leaked an out-of-slice document: the EQL \
         condition matched both members' docs, but the alias filter (status==active) must \
         hide idx-b's status=deleted row. Fanning out unfiltered returns it (#451 _eql arm \
         must AND the alias filter in, like _search/_msearch do): {body}"
    );
    // The one surviving event is the in-slice member's, not the leaked one.
    assert_eq!(
        events[0].get("_index").and_then(Value::as_str),
        Some("idx-a"),
        "the surviving event must be idx-a's in-slice doc, reported under its concrete \
         member name: {body}"
    );
}
