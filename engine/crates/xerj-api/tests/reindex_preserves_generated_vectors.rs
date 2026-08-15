//! Regression coverage for document-copying paths.
//!
//! The default `_source` projection now omits engine-generated embedding
//! companions. `_reindex` must opt into the complete source explicitly, or it
//! silently loses the vectors while reporting a successful copy.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use xerj_common::types::{FieldConfig, FieldType, Schema};

async fn seeded_app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.path().to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);

    let mut schema = Schema::empty();
    let mut body = FieldConfig::new("body", FieldType::Text);
    body.options.dimensions = Some(32);
    body.options.similarity = Some("cosine".into());
    body.embedding = Some(xerj_common::types::EmbeddingConfig {
        endpoint: None,
        model: None,
        target_field: Some("body_vector".into()),
    });
    schema.add_field(body).expect("body field");
    let mut companion = FieldConfig::new("body_vector", FieldType::Vector);
    companion.options.dimensions = Some(32);
    companion.options.similarity = Some("cosine".into());
    schema.add_field(companion).expect("body_vector field");

    state
        .engine
        .create_index("src", schema)
        .expect("create source index");
    let idx = state.engine.get_index("src").expect("get source index");
    let body_text =
        "XERJ preserves generated embedding companions when documents move through copy paths. "
            .repeat(24);
    idx.index_document(Some("1".into()), json!({ "body": body_text }))
        .await
        .expect("index source document");
    idx.refresh().await.expect("refresh source");

    (xerj_api::router::build_es_compat_router(state), dir)
}

async fn post(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post(path)
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
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn reindex_preserves_generated_embedding_companions() {
    let (app, _dir) = seeded_app().await;

    let (status, source_search) = post(
        &app,
        "/src/_search",
        json!({
            "query": { "match_all": {} },
            "size": 1,
            "_source": true,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "source search with explicit full _source failed: {source_search}"
    );
    let source = &source_search["hits"]["hits"][0]["_source"];
    let source_vector = source["body_vector"].clone();
    assert!(
        source_vector
            .as_array()
            .is_some_and(|values| !values.is_empty()),
        "the source fixture must contain a generated body_vector: {source}"
    );
    let source_chunks = source
        .get("body_vector_chunks")
        .filter(|chunks| chunks.is_array())
        .cloned();

    let (status, reindex_response) = post(
        &app,
        "/_reindex",
        json!({
            "source": { "index": "src" },
            "dest": { "index": "dst" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reindex failed: {reindex_response}");
    assert_eq!(
        reindex_response["total"], 1,
        "reindex response: {reindex_response}"
    );

    let (status, refresh_response) = post(&app, "/dst/_refresh", json!({})).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "destination refresh failed: {refresh_response}"
    );

    let (status, destination_search) = post(
        &app,
        "/dst/_search",
        json!({
            "query": { "match_all": {} },
            "size": 1,
            "_source": true,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "destination search with explicit full _source failed: {destination_search}"
    );
    let destination = &destination_search["hits"]["hits"][0]["_source"];
    assert_eq!(
        destination["body_vector"], source_vector,
        "reindex must request `_source: true` explicitly or generated body_vector is silently lost"
    );
    if let Some(chunks) = source_chunks {
        assert_eq!(
            destination["body_vector_chunks"], chunks,
            "reindex must preserve generated body_vector_chunks when the source document has them"
        );
    }
}

/// `_clone` / `_shrink` / `_split` all funnel through `clone_index_to`, which
/// is a copy path with exactly the same requirement as `_reindex`: read the
/// COMPLETE `_source`, companions included.
///
/// This pins the reconciliation of two changes that landed independently on
/// the same function. One replaced the single `size: 10000` search with a
/// keyset-paged loop that propagates write failures (issue #204 — a clone that
/// silently copied the first 10,000 documents, or copied nothing at all, still
/// answered `{"acknowledged": true}`). The other added the explicit
/// `"_source": true` to the search body, because the default projection hides
/// engine-generated embedding companions. Keeping the loop while dropping the
/// projection would have traded a silently truncated copy for a silently lossy
/// one — the same defect wearing a different hat — so the per-page body
/// carries it, and this test fails if a future edit drops it again.
#[tokio::test]
async fn clone_preserves_generated_embedding_companions() {
    let (app, _dir) = seeded_app().await;

    let (status, source_search) = post(
        &app,
        "/src/_search",
        json!({
            "query": { "match_all": {} },
            "size": 1,
            "_source": true,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "source search with explicit full _source failed: {source_search}"
    );
    let source = &source_search["hits"]["hits"][0]["_source"];
    let source_vector = source["body_vector"].clone();
    assert!(
        source_vector
            .as_array()
            .is_some_and(|values| !values.is_empty()),
        "the source fixture must contain a generated body_vector: {source}"
    );

    // A second document whose companion was supplied by the CALLER and does
    // not match what the embedder would produce from `body`. Without the
    // explicit projection the clone reads the default `_source`, which hides
    // companions, and the destination silently regenerates one from `body` —
    // so this is the document that can tell a verbatim copy from a lossy one.
    // (Document 1 cannot: the destination inherits the source schema, so its
    // regenerated companion is byte-identical to the source's.)
    let supplied: Vec<f64> = (0..32).map(|i| (i as f64) / 32.0).collect();
    let (status, indexed) = post(
        &app,
        "/src/_doc/2?refresh=true",
        json!({ "body": "a second document", "body_vector": supplied }),
    )
    .await;
    assert!(
        status.is_success(),
        "seeding the caller-supplied companion failed: {indexed}"
    );
    let (status, supplied_search) = post(
        &app,
        "/src/_search",
        json!({
            "query": { "ids": { "values": ["2"] } },
            "_source": true,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "source search failed: {supplied_search}"
    );
    let supplied_vector = supplied_search["hits"]["hits"][0]["_source"]["body_vector"].clone();

    let (status, clone_response) = post(&app, "/src/_clone/cloned", json!({})).await;
    assert_eq!(status, StatusCode::OK, "clone failed: {clone_response}");

    let (status, refresh_response) = post(&app, "/cloned/_refresh", json!({})).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "clone refresh failed: {refresh_response}"
    );

    let (status, cloned_search) = post(
        &app,
        "/cloned/_search",
        json!({
            "query": { "match_all": {} },
            "size": 10,
            "_source": true,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "clone search with explicit full _source failed: {cloned_search}"
    );
    assert_eq!(
        cloned_search["hits"]["total"]["value"], 2,
        "the clone must contain every source document: {cloned_search}"
    );
    let by_id = |id: &str| -> Value {
        cloned_search["hits"]["hits"]
            .as_array()
            .expect("hits array")
            .iter()
            .find(|hit| hit["_id"] == id)
            .unwrap_or_else(|| panic!("document [{id}] missing from the clone: {cloned_search}"))
            ["_source"]
            .clone()
    };
    assert_eq!(
        by_id("1")["body_vector"],
        source_vector,
        "clone lost the generated body_vector"
    );
    assert_eq!(
        by_id("2")["body_vector"],
        supplied_vector,
        "clone must request `_source: true` on every page: without it the copy \
         reads the default projection, the companion never crosses, and the \
         destination regenerates a DIFFERENT vector from `body` while still \
         answering {{\"acknowledged\": true}}"
    );
}
