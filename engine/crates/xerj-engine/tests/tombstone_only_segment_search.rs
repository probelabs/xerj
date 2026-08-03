//! Search regressions for delete-only (`doc_count == 0`) segments.
//!
//! A persisted delete has no stored documents. Its segment intentionally
//! contains only the tombstone section, so query paths must not try to hydrate
//! a `Stored` section from it.

use serde_json::json;
use tempfile::TempDir;
use xerj_common::config::Config;
use xerj_common::types::Schema;
use xerj_engine::{Engine, Index};
use xerj_query::parse_request;

fn make_engine(dir: &TempDir) -> Engine {
    let mut config = Config::default();
    config.server.data_dir = dir.path().to_str().unwrap().to_string();
    Engine::new(config).expect("engine::new")
}

async fn seed_delete_only_segment(engine: &Engine, name: &str) {
    engine.create_index(name, Schema::empty()).unwrap();
    let index = engine.get_index(name).unwrap();

    index
        .index_document(Some("live".into()), json!({"group": "kept", "amount": 10}))
        .await
        .unwrap();
    index
        .index_document(
            Some("deleted".into()),
            json!({"group": "kept", "amount": 999}),
        )
        .await
        .unwrap();
    index.flush().await.unwrap();

    assert!(index.delete_document("deleted").await.unwrap());
    index.flush().await.unwrap();

    assert!(
        index
            .store_snapshot()
            .segments
            .iter()
            .any(|segment| segment.doc_count == 0 && segment.has_tombstones),
        "test setup must publish a tombstone-only segment"
    );
}

async fn assert_filtered_aggregation_works(index: &Index) {
    let unfiltered = parse_request(&json!({
        "query": {"match_all": {}},
        "track_total_hits": true
    }))
    .unwrap();
    let result = index
        .search(&unfiltered)
        .await
        .expect("unfiltered search must succeed");
    assert_eq!(result.total.value, 1);
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].id, "live");

    // A filtered aggregation enters the JSON full-corpus fallback before the
    // later doc-values fallback can answer. Before the fix that corpus walk
    // attempted to hydrate the tombstone-only segment and returned
    // `segment ... unreadable during corpus assembly`.
    let request = parse_request(&json!({
        "size": 0,
        "track_total_hits": true,
        "query": {"terms": {"group": ["kept"]}},
        "aggs": {
            "amount_min": {"min": {"field": "amount"}},
            "amount_max": {"max": {"field": "amount"}}
        }
    }))
    .unwrap();

    let result = index.search(&request).await.expect("search must succeed");
    assert_eq!(result.total.value, 1);
    let aggs = result.aggs.expect("aggregations");
    assert_eq!(aggs["amount_min"]["value"], json!(10.0));
    assert_eq!(aggs["amount_max"]["value"], json!(10.0));
}

#[tokio::test]
async fn filtered_aggregation_skips_live_tombstone_only_segment() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    seed_delete_only_segment(&engine, "live-delete").await;

    let index = engine.get_index("live-delete").unwrap();
    assert_filtered_aggregation_works(&index).await;
}

#[tokio::test]
async fn filtered_aggregation_skips_reopened_tombstone_only_segment() {
    let dir = TempDir::new().unwrap();
    {
        let engine = make_engine(&dir);
        seed_delete_only_segment(&engine, "reopened-delete").await;
    }

    let reopened = make_engine(&dir);
    let index = reopened.get_index("reopened-delete").unwrap();
    assert!(
        index
            .store_snapshot()
            .segments
            .iter()
            .any(|segment| segment.doc_count == 0 && segment.has_tombstones),
        "restart must retain the tombstone-only segment"
    );
    assert_filtered_aggregation_works(&index).await;
}
