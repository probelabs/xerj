//! A `knn` (or `semantic`) clause naming a field the index cannot answer must
//! fail loudly with a `400`, not return a silent, successful empty result —
//! issue #498.
//!
//! `refsym`, XERJ's own reference-coding corpus, has no `dense_vector` field, so
//! `knn: {field: "embedding", ...}` had nothing to run against and came back
//! `200` with `hits.total = 0` and `error: null`. An agent cannot tell "nothing
//! matched" from "this index does not do vector search," so it concludes the
//! code does not exist — the exact silent-false-negative the vector/semantic
//! badges are supposed to prevent.
//!
//! Elasticsearch answers a `knn` against a missing or wrongly-typed field with
//! `400 illegal_argument_exception` naming the field. XERJ must do the same.
//!
//! The companion `knn_on_declared_but_empty_vector_field_is_empty_200` locks the
//! distinction that sank the first attempt (#529): a field that IS a
//! `dense_vector` but currently holds no vectors is a real empty result, not an
//! error — the 400 must key on "the field cannot answer a vector query," not on
//! "the field has no data yet."
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

/// The #498 bug: `knn` on a field that is absent from a non-vector index must be
/// a `400`, not a silent empty `200`.
#[tokio::test]
async fn knn_on_absent_field_is_400_not_silent_empty() {
    let (app, _dir) = app().await;

    // An index whose mapping has NO dense_vector field (mirrors refsym).
    let (status, body) = json_req(
        &app,
        "PUT",
        "/codes",
        json!({ "mappings": { "properties": {
            "name": { "type": "text" },
            "code": { "type": "keyword" }
        } } }),
    )
    .await;
    assert!(status.is_success(), "create index: {status} {body}");

    let (status, body) = json_req(
        &app,
        "POST",
        "/codes/_doc/1",
        json!({ "name": "sniff content type", "code": "fn sniff() {}" }),
    )
    .await;
    assert!(status.is_success(), "index doc: {status} {body}");
    let (_s, _b) = json_req(&app, "POST", "/codes/_refresh", json!({})).await;

    // knn against a field that does not exist in the mapping.
    let (status, body) = json_req(
        &app,
        "POST",
        "/codes/_search",
        json!({ "knn": {
            "field": "embedding",
            "query_vector": [0.1, 0.2, 0.3],
            "k": 5,
            "num_candidates": 50
        } }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "knn on an absent field must be 400, not a silent empty 200 (#498): got {status} {body}"
    );
    let reason = body
        .pointer("/error/reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        reason.contains("embedding"),
        "the 400 must name the field: {body}"
    );
}

/// The invariant #529 broke: a field that IS a `dense_vector` but currently
/// holds no vectors returns an empty `200`, NOT a 400. "Cannot answer a vector
/// query" is a mapping/execution fact, not "has no data yet."
#[tokio::test]
async fn knn_on_declared_but_empty_vector_field_is_empty_200() {
    let (app, _dir) = app().await;

    let (status, body) = json_req(
        &app,
        "PUT",
        "/vecs",
        json!({ "mappings": { "properties": {
            "v": { "type": "dense_vector", "dims": 3 }
        } } }),
    )
    .await;
    assert!(status.is_success(), "create vector index: {status} {body}");
    // Deliberately index NO documents — the field exists and is vector-typed,
    // it simply has nothing to match.

    let (status, body) = json_req(
        &app,
        "POST",
        "/vecs/_search",
        json!({ "knn": {
            "field": "v",
            "query_vector": [0.1, 0.2, 0.3],
            "k": 5,
            "num_candidates": 50
        } }),
    )
    .await;

    assert!(
        status.is_success(),
        "knn on a declared-but-empty dense_vector field is a real empty result, not an error (#498/#529 guard): {status} {body}"
    );
    let total = body
        .pointer("/hits/total/value")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    assert_eq!(total, 0, "expected an empty result set: {body}");
}

/// #542 (follow-up to #498): the MULTI-knn array form (`knn: [...]`) must also
/// 400 a clause naming an unanswerable field — #498 only wired the check into
/// the single top-level knn arm, so the array form still returns a silent
/// empty 200.
#[tokio::test]
async fn multi_knn_on_absent_field_is_400_not_silent_empty() {
    let (app, _dir) = app().await;
    let (status, body) = json_req(
        &app,
        "PUT",
        "/codes",
        json!({ "mappings": { "properties": {
            "name": { "type": "text" },
            "code": { "type": "keyword" }
        } } }),
    )
    .await;
    assert!(status.is_success(), "create index: {status} {body}");
    let (_s, _b) = json_req(
        &app,
        "POST",
        "/codes/_doc/1",
        json!({ "name": "sniff", "code": "fn sniff() {}" }),
    )
    .await;
    let (_s, _b) = json_req(&app, "POST", "/codes/_refresh", json!({})).await;

    // The ES multi-knn array form with 2+ clauses (a single-element array
    // collapses to the single-knn path, already fixed by #498). A clause naming
    // a field absent from the mapping must fail loudly, not return a silent
    // empty 200.
    let (status, body) = json_req(
        &app,
        "POST",
        "/codes/_search",
        json!({ "knn": [
            { "field": "embedding", "query_vector": [0.1, 0.2, 0.3], "k": 5, "num_candidates": 50 },
            { "field": "embedding2", "query_vector": [0.4, 0.5, 0.6], "k": 5, "num_candidates": 50 }
        ] }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "multi-knn on an absent field must be 400, not a silent empty 200 (#542): got {status} {body}"
    );
    assert!(
        body.pointer("/error/reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("embedding"),
        "the 400 must name the field: {body}"
    );
}

/// #542 (nested-knn arm): a `nested` knn naming a vector field absent from the
/// nested mapping must 400, not return a silent empty 200. The nested arm is a
/// separate dispatch from the single/multi-knn arms fixed in #498/#543.
#[tokio::test]
async fn nested_knn_on_absent_field_is_400_not_silent_empty() {
    let (app, _dir) = app().await;
    let (status, body) = json_req(
        &app,
        "PUT",
        "/docs",
        json!({ "mappings": { "properties": {
            "items": { "type": "nested", "properties": { "name": { "type": "text" } } }
        } } }),
    )
    .await;
    assert!(status.is_success(), "create index: {status} {body}");
    let (_s, _b) = json_req(
        &app,
        "POST",
        "/docs/_doc/1",
        json!({ "items": [ { "name": "sniff" }, { "name": "detect" } ] }),
    )
    .await;
    let (_s, _b) = json_req(&app, "POST", "/docs/_refresh", json!({})).await;

    // Nested knn on `items.vector`, which the nested mapping does not have.
    let (status, body) = json_req(
        &app,
        "POST",
        "/docs/_search",
        json!({ "query": { "nested": { "path": "items", "query": { "knn": {
            "field": "items.vector", "query_vector": [0.1, 0.2, 0.3], "k": 5, "num_candidates": 50
        } } } } }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "nested knn on an absent field must be 400, not a silent empty 200 (#542): got {status} {body}"
    );
    assert!(
        body.pointer("/error/reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("items.vector"),
        "the 400 must name the field: {body}"
    );
}

/// #547: the #529-class invariant for MULTI-knn — a 2-clause array on
/// DECLARED-but-empty dense_vector fields returns an empty 200, not a 400.
#[tokio::test]
async fn multi_knn_on_declared_but_empty_vector_fields_is_empty_200() {
    let (app, _dir) = app().await;
    let (status, body) = json_req(
        &app,
        "PUT",
        "/vecs",
        json!({ "mappings": { "properties": {
            "a": { "type": "dense_vector", "dims": 3 },
            "b": { "type": "dense_vector", "dims": 3 }
        } } }),
    )
    .await;
    assert!(status.is_success(), "create index: {status} {body}");
    // No documents indexed — both fields are declared vectors with nothing to match.
    let (status, body) = json_req(
        &app,
        "POST",
        "/vecs/_search",
        json!({ "knn": [
            { "field": "a", "query_vector": [0.1, 0.2, 0.3], "k": 5, "num_candidates": 50 },
            { "field": "b", "query_vector": [0.4, 0.5, 0.6], "k": 5, "num_candidates": 50 }
        ] }),
    )
    .await;
    assert!(
        status.is_success(),
        "multi-knn on declared-but-empty vector fields is a real empty result, not 400 (#547/#529): {status} {body}"
    );
    assert_eq!(
        body.pointer("/hits/total/value").and_then(Value::as_u64),
        Some(0),
        "expected empty: {body}"
    );
}

/// #547: the #529-class invariant for NESTED knn — a nested knn on a
/// DECLARED-but-empty nested dense_vector returns an empty 200, not a 400.
#[tokio::test]
async fn nested_knn_on_declared_but_empty_vector_field_is_empty_200() {
    let (app, _dir) = app().await;
    let (status, body) = json_req(
        &app,
        "PUT",
        "/docs",
        json!({ "mappings": { "properties": {
            "items": { "type": "nested", "properties": {
                "vector": { "type": "dense_vector", "dims": 3 }
            } }
        } } }),
    )
    .await;
    assert!(status.is_success(), "create index: {status} {body}");
    // No documents indexed — the nested vector field is declared, just empty.
    let (status, body) = json_req(
        &app,
        "POST",
        "/docs/_search",
        json!({ "query": { "nested": { "path": "items", "query": { "knn": {
            "field": "items.vector", "query_vector": [0.1, 0.2, 0.3], "k": 5, "num_candidates": 50
        } } } } }),
    )
    .await;
    assert!(
        status.is_success(),
        "nested knn on a declared-but-empty nested dense_vector is a real empty result, not 400 (#547/#529): {status} {body}"
    );
    assert_eq!(
        body.pointer("/hits/total/value").and_then(Value::as_u64),
        Some(0),
        "expected empty: {body}"
    );
}
