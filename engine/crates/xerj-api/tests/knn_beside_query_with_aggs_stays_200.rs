//! Issue #458 (rework guard): the ES 8.x "faceted hybrid" shape is a top-level
//! `knn` beside a `query` AND an `aggs` block. Folding EVERY knn-beside-query
//! request to `hybrid` regressed this from 200 to 400, because the hybrid/fusion
//! executor rejects aggregations. The rework folds to `hybrid` only when the
//! request is hybrid-safe (no aggs/sort/collapse/search_after/min_score);
//! otherwise it keeps the lexical `bool.should`, which still computes the
//! aggregations and returns 200. This guards that regression.
//!
//! Elasticsearch is referenced for wire semantics only; no ES code is here.

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

/// `knn` + `query` + `aggs` (faceted hybrid) must return 200 with its facets —
/// not the 400 an unconditional hybrid fold produced.
#[tokio::test]
async fn knn_beside_query_with_aggs_stays_200_with_facets() {
    let (app, _dir) = app().await;

    let (st, b) = json_req(
        &app,
        "PUT",
        "/hy",
        json!({ "mappings": { "properties": {
            "body": { "type": "text" },
            "v": { "type": "dense_vector", "dims": 3 },
            "cat": { "type": "keyword" }
        } } }),
    )
    .await;
    assert!(st.is_success(), "create index: {st} {b}");

    for (id, body, vec, cat) in [
        ("1", "alpha beta", [0.1_f32, 0.2, 0.3], "x"),
        ("2", "alpha gamma", [0.2_f32, 0.1, 0.4], "y"),
    ] {
        let (st, b) = json_req(
            &app,
            "POST",
            &format!("/hy/_doc/{id}"),
            json!({ "body": body, "v": vec, "cat": cat }),
        )
        .await;
        assert!(st.is_success(), "index {id}: {st} {b}");
    }
    let (_s, _b) = json_req(&app, "POST", "/hy/_refresh", json!({})).await;

    // The ES 8.x faceted-hybrid shape: top-level knn + query + aggs.
    let (status, body) = json_req(
        &app,
        "POST",
        "/hy/_search",
        json!({
            "query": { "match": { "body": "alpha" } },
            "knn": { "field": "v", "query_vector": [0.1, 0.2, 0.3], "k": 3, "num_candidates": 10 },
            "aggs": { "by_cat": { "terms": { "field": "cat" } } }
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "#458: knn + query + aggs (faceted hybrid) must stay 200, not regress to 400 — an \
         unconditional hybrid fold hits the executor's 'aggregations are not supported with \
         hybrid/fusion queries' path: {status} {body}"
    );
    let buckets = body
        .pointer("/aggregations/by_cat/buckets")
        .and_then(Value::as_array);
    assert!(
        buckets.is_some_and(|b| !b.is_empty()),
        "the aggregation must be computed and returned, not dropped: {body}"
    );
    assert!(
        body.pointer("/hits/hits")
            .and_then(Value::as_array)
            .is_some_and(|h| !h.is_empty()),
        "hits must still be returned alongside the aggregation: {body}"
    );
}

/// The core fix: a hybrid-safe `knn` beside a `query` must return the UNION of
/// both halves — a document reachable only via the vector side must appear. The
/// old fold to a two-clause `bool.should` never dispatched the kNN half, so that
/// document was silently dropped.
#[tokio::test]
async fn knn_beside_query_returns_the_vector_only_document() {
    let (app, _dir) = app().await;

    let (st, b) = json_req(
        &app,
        "PUT",
        "/u",
        json!({ "mappings": { "properties": {
            "body": { "type": "text" },
            "v": { "type": "dense_vector", "dims": 3 }
        } } }),
    )
    .await;
    assert!(st.is_success(), "create index: {st} {b}");

    // doc "lex" matches the lexical query; doc "vec" does NOT (its body shares no
    // term) but its vector is the query vector, so only the kNN half reaches it.
    for (id, body, vec) in [
        ("lex", "alpha beta", [1.0_f32, 0.0, 0.0]),
        ("vec", "zzz qqq", [0.1_f32, 0.2, 0.3]),
    ] {
        let (st, b) = json_req(
            &app,
            "POST",
            &format!("/u/_doc/{id}"),
            json!({ "body": body, "v": vec }),
        )
        .await;
        assert!(st.is_success(), "index {id}: {st} {b}");
    }
    let (_s, _b) = json_req(&app, "POST", "/u/_refresh", json!({})).await;

    let (status, body) = json_req(
        &app,
        "POST",
        "/u/_search",
        json!({
            "query": { "match": { "body": "alpha" } },
            "knn": { "field": "v", "query_vector": [0.1, 0.2, 0.3], "k": 2, "num_candidates": 10 }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "_search status: {status} {body}");

    let ids: Vec<String> = body
        .pointer("/hits/hits")
        .and_then(Value::as_array)
        .map(|hits| {
            hits.iter()
                .filter_map(|h| h.get("_id").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        ids.iter().any(|id| id == "vec"),
        "#458: knn beside query dropped the vector-only document — the kNN half was \
         never dispatched. hits={ids:?} body={body}"
    );
    assert!(
        ids.iter().any(|id| id == "lex"),
        "the lexical match must still be present: hits={ids:?} body={body}"
    );
}
