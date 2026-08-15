//! Issue #362, from the outside: `_count` must never report documents that
//! `_search` cannot return.
//!
//! `POST /{index}/_count` is implemented as `search({query, size: 0}).total`
//! (`es_compat::count_docs`), so the two endpoints are supposed to answer the
//! same question. On a `text` field they did not. A `text` mapping carries
//! `doc_values: false`, so the engine's term-count shortcut had no column to
//! read and fell back to the segment's FTS term dictionary — which holds
//! ANALYZED tokens (tokenised and lowercased by the standard analyzer), while
//! `_search` resolves a `term` on that field against the whole `_source`
//! value. Both spellings below counted a document that no `_search`, at any
//! `size`, could produce:
//!
//! ```text
//! {"term":{"title":"testsegmentreader.java"}}   count 1, hits 0
//! {"term":{"title":"quick"}}                    count 1, hits 0
//! ```
//!
//! The second row has no case component at all, which is why it is here: it is
//! the case that proves the disagreement is not about casing.

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

/// Two documents, one `text` field, flushed to a segment — the shape the
/// issue was filed against (a source-file corpus with CamelCase names).
async fn seeded() -> (axum::Router, tempfile::TempDir) {
    let (app, dir) = app().await;
    let (status, body) = json_req(
        &app,
        "PUT",
        "/files",
        json!({
            "mappings": { "properties": {
                "title": { "type": "text" },
                "path":  { "type": "keyword" }
            }}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create index failed: {body}");

    for (id, doc) in [
        (
            "java",
            json!({"title": "TestSegmentReader.java", "path": "lucene/TestSegmentReader.java"}),
        ),
        (
            "fox",
            json!({"title": "the quick brown fox", "path": "docs/fox.txt"}),
        ),
    ] {
        let (status, body) = json_req(&app, "PUT", &format!("/files/_doc/{id}"), doc).await;
        assert!(status.is_success(), "index {id} failed: {status} {body}");
    }
    // The count shortcut under test only runs over flushed segments.
    let (status, body) = json_req(&app, "POST", "/files/_flush", json!({})).await;
    assert!(status.is_success(), "flush failed: {status} {body}");
    (app, dir)
}

/// `_count` and `_search` must agree on both the total AND the number of
/// documents actually retrievable.
async fn assert_agrees(app: &axum::Router, query: Value, label: &str) {
    let (status, count_body) =
        json_req(app, "POST", "/files/_count", json!({ "query": query })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{label}: _count failed {count_body}"
    );
    let count = count_body["count"].as_u64().expect("count is a number");

    let (status, search_body) = json_req(
        app,
        "POST",
        "/files/_search",
        json!({ "query": query, "size": 50 }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{label}: _search failed {search_body}"
    );
    let total = search_body["hits"]["total"]["value"]
        .as_u64()
        .expect("total is a number");
    let hits = search_body["hits"]["hits"]
        .as_array()
        .expect("hits is an array")
        .len() as u64;

    assert_eq!(
        count, total,
        "{label}: _count said {count}, _search total said {total} for {query}"
    );
    assert_eq!(
        count, hits,
        "{label}: _count said {count} but _search returned {hits} document(s) for {query}"
    );
}

#[tokio::test]
async fn count_does_not_overreport_a_lowercased_term_on_a_text_field() {
    let (app, _dir) = seeded().await;
    assert_agrees(
        &app,
        json!({"term": {"title": "testsegmentreader.java"}}),
        "lowercased spelling",
    )
    .await;
}

#[tokio::test]
async fn count_does_not_overreport_an_analyzed_token_on_a_text_field() {
    let (app, _dir) = seeded().await;
    assert_agrees(&app, json!({"term": {"title": "quick"}}), "analyzed token").await;
}

/// The spelling that does match keeps working: no undercount in exchange.
#[tokio::test]
async fn count_still_agrees_on_the_exact_text_term() {
    let (app, _dir) = seeded().await;
    let query = json!({"term": {"title": "TestSegmentReader.java"}});
    assert_agrees(&app, query.clone(), "exact spelling").await;
    let (_, body) = json_req(&app, "POST", "/files/_count", json!({ "query": query })).await;
    assert_eq!(
        body["count"], 1,
        "exact-spelling term must still count its document: {body}"
    );
}

/// A `keyword` field has doc-values and never took the FTS fallback; it must
/// keep its exact, case-sensitive count.
#[tokio::test]
async fn keyword_term_count_is_unchanged() {
    let (app, _dir) = seeded().await;
    let query = json!({"term": {"path": "lucene/TestSegmentReader.java"}});
    assert_agrees(&app, query.clone(), "keyword field").await;
    let (_, body) = json_req(&app, "POST", "/files/_count", json!({ "query": query })).await;
    assert_eq!(body["count"], 1, "keyword term must count 1: {body}");

    let wrong_case = json!({"term": {"path": "LUCENE/TestSegmentReader.java"}});
    assert_agrees(&app, wrong_case.clone(), "keyword field, wrong case").await;
    let (_, body) = json_req(
        &app,
        "POST",
        "/files/_count",
        json!({ "query": wrong_case }),
    )
    .await;
    assert_eq!(
        body["count"], 0,
        "keyword term must stay case-sensitive: {body}"
    );
}
