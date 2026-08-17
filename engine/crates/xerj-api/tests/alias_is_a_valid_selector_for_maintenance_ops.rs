//! An alias is a valid index selector in Elasticsearch for every maintenance
//! endpoint. Here it was not: `resolve_indices_for_op` had no alias branch at
//! all, so an alias was simply "not an index" and the whole request 404'd.
//!
//! Measured before the fix, on a three-member alias `tri`:
//!
//!   POST /tri/_refresh       -> 404      POST /idx-a/_refresh       -> 200
//!   POST /tri/_forcemerge    -> 404      POST /idx-a/_forcemerge    -> 200
//!   POST /tri/_cache/clear   -> 404      POST /idx-a/_cache/clear   -> 200
//!   POST /tri/_terms_enum    -> 404      POST /idx-a/_terms_enum    -> 200
//!
//! The alias expands to EVERY member rather than `aliased.first()`. A
//! maintenance op that silently addressed one member of a multi-index alias is
//! the same defect class as #450 on the destructive side — and a `_refresh`
//! that reaches one of three members makes the next search non-deterministic
//! in a way nothing in the response admits.

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

async fn req(app: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path);
    let body = if body.is_null() {
        Body::empty()
    } else {
        b = b.header("content-type", "application/json");
        Body::from(body.to_string())
    };
    let res = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn three_members(app: &axum::Router) {
    for m in ["idx-a", "idx-b", "idx-c"] {
        req(
            app,
            "PUT",
            &format!("/{m}"),
            json!({"mappings":{"properties":{"n":{"type":"integer"}}}}),
        )
        .await;
        for n in 1..=3 {
            req(app, "POST", &format!("/{m}/_doc/{m}{n}"), json!({"n": n})).await;
        }
        let (st, _) = req(app, "PUT", &format!("/{m}/_alias/tri"), Value::Null).await;
        assert!(st.is_success(), "alias on {m}: {st}");
    }
}

#[tokio::test]
async fn maintenance_endpoints_accept_an_alias_and_reach_every_member() {
    let (app, _d) = app().await;
    three_members(&app).await;

    for (method, path, body) in [
        ("POST", "/tri/_refresh", Value::Null),
        ("POST", "/tri/_forcemerge", Value::Null),
        ("POST", "/tri/_cache/clear", Value::Null),
        ("POST", "/tri/_terms_enum", json!({"field": "n"})),
    ] {
        let (st, b) = req(&app, method, path, body).await;
        assert_eq!(st, StatusCode::OK, "{path} must accept an alias. body: {b}");
    }

    // Not just accepted — every member must be reached. One-member behaviour
    // would still return 200 and would still be wrong.
    let (st, b) = req(&app, "POST", "/tri/_refresh", Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        b["_shards"]["total"], 3,
        "an alias over three indices must refresh all three, not aliased.first(). body: {b}"
    );
    assert_eq!(b["_shards"]["failed"], 0, "body: {b}");
}

/// The alias branch must not weaken the missing-name rule: ES defaults
/// `ignore_unavailable=false`, so a typo in a comma list fails the whole
/// request rather than silently doing less than asked.
#[tokio::test]
async fn an_unknown_name_still_fails_the_whole_request() {
    let (app, _d) = app().await;
    three_members(&app).await;

    let (st, _) = req(&app, "POST", "/no-such-thing/_refresh", Value::Null).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _) = req(&app, "POST", "/idx-a,typo/_refresh", Value::Null).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "a typo beside a real name must 404"
    );

    // And an index of the same name as an alias wins, as in ES.
    let (st, b) = req(&app, "POST", "/idx-a/_refresh", Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        b["_shards"]["total"], 1,
        "concrete name addresses one index. body: {b}"
    );
}
