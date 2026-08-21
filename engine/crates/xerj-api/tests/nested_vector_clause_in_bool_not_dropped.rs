//! Issue #395: a `semantic`/`knn` clause nested inside a `bool` is silently
//! dropped — the engine only *peels the top* of the query tree, so a vector
//! clause that is not at the top (or is one of several `bool` clauses) never
//! reaches the vector path. The request answers `200 OK` with the purely lexical
//! result set, no error and no `_shards.failed`.
//!
//! Case B from the report: a `bool` whose only clause is `semantic` returns zero
//! hits, while the same clause at the top level returns all three documents.
//! Whatever the resolution (dispatch the nested clause, or reject it loudly),
//! `bool{must:[semantic]}` must not silently answer `200`/zero.
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

async fn total(app: &axum::Router, query: Value) -> (StatusCode, u64) {
    let (status, body) = call(
        app,
        "POST",
        "/docs/_search",
        json!({ "query": query, "size": 10, "track_total_hits": true }),
    )
    .await;
    let t = body
        .pointer("/hits/total/value")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    (status, t)
}

/// A `bool` whose only clause is `semantic` must return the same documents as
/// the bare `semantic` clause — not silently drop the vector half to zero (#395).
#[tokio::test]
async fn semantic_clause_nested_in_bool_is_not_silently_dropped() {
    let (app, _dir) = app().await;
    let (st, body) = call(
        &app,
        "PUT",
        "/docs",
        json!({"mappings": {"properties": {
            "ctx": { "type": "semantic_text", "dimensions": 32 }
        }}}),
    )
    .await;
    assert!(st.is_success(), "create semantic_text index: {st} {body}");

    for (id, text) in [
        ("0", "graph edges connect nodes in a network"),
        ("1", "weather forecast sunshine and rain today"),
        ("2", "edges and vertices form a graph structure"),
    ] {
        let (st, _) = call(
            &app,
            "POST",
            &format!("/docs/_doc/{id}"),
            json!({ "ctx": text }),
        )
        .await;
        assert!(st.is_success(), "index doc {id}: {st}");
    }
    let (_s, _b) = call(&app, "POST", "/docs/_refresh", Value::Null).await;

    let q = "graph edges";
    let sem = json!({ "semantic": { "field": "ctx", "query": q, "k": 10 } });

    // A: bare semantic — the baseline the nested form must match.
    let (sa, bare_total) = total(&app, sem.clone()).await;
    assert_eq!(sa, StatusCode::OK, "bare semantic status");
    assert!(
        bare_total > 0,
        "precondition: bare semantic must find the semantic_text docs (got {bare_total})"
    );

    // B: the SAME clause nested in a single-clause bool.must.
    let (sb, nested_total) = total(&app, json!({ "bool": { "must": [ sem ] } })).await;

    // If the executor cannot dispatch the nested clause it must say so (400),
    // not answer 200 with a silently lexical/empty result. Either way it must
    // NOT return a smaller, misleading hit set than the bare form.
    if sb == StatusCode::OK {
        assert_eq!(
            nested_total, bare_total,
            "a bool wrapping only a `semantic` clause returned {nested_total} hits vs {bare_total} \
             for the bare clause — the vector half was silently dropped (#395)"
        );
    } else {
        assert_eq!(
            sb,
            StatusCode::BAD_REQUEST,
            "a nested vector clause that cannot be dispatched must 400 (naming `hybrid`), \
             not answer a silent lexical 200 (#395): got {sb}"
        );
    }
}
