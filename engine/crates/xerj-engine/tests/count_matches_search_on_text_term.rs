//! Regression tests for issue #362: `_count` must answer the same question
//! `_search` answers.
//!
//! `POST /{index}/_count` is `search({query, size: 0}).total`, so a `size: 0`
//! total that disagrees with the `size > 0` hit set is the HTTP-visible bug:
//! a count describing documents nobody can retrieve.
//!
//! Pre-fix, `try_shortcut_count`'s Term arm fell back to the segment FTS term
//! dictionary whenever the field had no doc-values column — which is exactly
//! what an analyzed `text` field is. The dictionary holds ANALYZED tokens
//! (standard analyzer: tokenised, lowercased), while `_search` resolves a
//! `term` on such a field against the whole `_source` value. Two documents,
//! two spellings, both counted and neither retrievable.

use serde_json::{json, Value};
use tempfile::TempDir;
use xerj_common::config::Config;
use xerj_common::types::{FieldConfig, FieldType, Schema};
use xerj_engine::{Engine, Index};
use xerj_query::parse_request;

fn make_engine(dir: &TempDir) -> Engine {
    let mut config = Config::default();
    config.server.data_dir = dir.path().to_str().unwrap().to_string();
    Engine::new(config).expect("engine::new")
}

/// What `POST /_count` runs: `size: 0`, total only (`es_compat::count_docs`).
async fn count_total(idx: &Index, q: &Value) -> u64 {
    let req = parse_request(&json!({ "query": q, "size": 0, "from": 0 })).expect("parse_request");
    idx.search(&req).await.expect("count search").total.value
}

/// What `POST /_search` runs: a real page of hits.
async fn search_hits(idx: &Index, q: &Value) -> (u64, usize) {
    let req = parse_request(&json!({ "query": q, "size": 50 })).expect("parse_request");
    let r = idx.search(&req).await.expect("search");
    (r.total.value, r.hits.len())
}

async fn seed() -> (TempDir, std::sync::Arc<Index>) {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let mut schema = Schema::empty();
    // `"type": "text"` from an ES mapping lands here with `doc_values: false`
    // (`es_compat::es_properties_to_fields`), which is precisely what sends a
    // term count for this field into the FTS term-dictionary fallback.
    let mut title = FieldConfig::new("title", FieldType::Text);
    title.options.doc_values = false;
    schema.fields.push(title);
    schema
        .fields
        .push(FieldConfig::new("path", FieldType::Keyword));
    // A keyword field with `doc_values: false` reaches the same
    // no-column branch as `title`, so the abandon must not cost it its
    // exact count — it is the shape that would turn an overcount into an
    // undercount if the branch were narrowed the other way.
    let mut code = FieldConfig::new("code", FieldType::Keyword);
    code.options.doc_values = false;
    schema.fields.push(code);
    engine.create_index("count-parity", schema).unwrap();
    let idx = engine.get_index("count-parity").unwrap();

    idx.index_document(
        Some("java".into()),
        json!({
            "title": "TestSegmentReader.java",
            "path": "lucene/TestSegmentReader.java",
            "code": "AB-1234"
        }),
    )
    .await
    .unwrap();
    idx.index_document(
        Some("fox".into()),
        json!({"title": "the quick brown fox", "path": "docs/fox.txt", "code": "ab-1234"}),
    )
    .await
    .unwrap();
    // The count shortcut only runs against flushed segments; the memtable
    // arm is a different code path and already agrees with the scan.
    idx.flush().await.unwrap();
    (dir, idx)
}

/// The `_count` / `_search` disagreement, one query per row.
async fn assert_count_agrees_with_search(idx: &Index, q: Value, label: &str) {
    let count = count_total(idx, &q).await;
    let (total, hits) = search_hits(idx, &q).await;
    assert_eq!(
        count, total,
        "{label}: _count total {count} != _search total {total} for {q}"
    );
    assert_eq!(
        count, hits as u64,
        "{label}: _count says {count} but _search can materialise only {hits} document(s) for {q}"
    );
}

/// Issue #362's own repro: a term whose spelling differs from `_source`
/// only in case. The dictionary retry made `_count` find it; `_search`
/// compares against the raw `_source` value and cannot.
#[tokio::test]
async fn count_does_not_overreport_lowercased_term_on_text_field() {
    let (_dir, idx) = seed().await;
    assert_count_agrees_with_search(
        &idx,
        json!({"term": {"title": "testsegmentreader.java"}}),
        "lowercased spelling",
    )
    .await;
}

/// The half with no case component at all: an ANALYZED token of a
/// multi-word text value. The dictionary hit here is genuine, which is why
/// deleting the lowercase retry alone would not have fixed it.
#[tokio::test]
async fn count_does_not_overreport_analyzed_token_on_text_field() {
    let (_dir, idx) = seed().await;
    assert_count_agrees_with_search(
        &idx,
        json!({"term": {"title": "quick"}}),
        "analyzed token of a multi-word value",
    )
    .await;
}

/// The exact spelling still counts, and still returns its document — the
/// fix must not trade the overcount for an undercount.
#[tokio::test]
async fn count_still_finds_the_exact_text_term() {
    let (_dir, idx) = seed().await;
    let q = json!({"term": {"title": "TestSegmentReader.java"}});
    assert_count_agrees_with_search(&idx, q.clone(), "exact spelling").await;
    assert_eq!(
        count_total(&idx, &q).await,
        1,
        "exact-spelling term must still count its document"
    );
}

/// A `keyword` field keeps its exact, byte-comparing count: that field has
/// a doc-values column and never took the FTS fallback.
#[tokio::test]
async fn keyword_term_count_is_unchanged() {
    let (_dir, idx) = seed().await;
    let q = json!({"term": {"path": "lucene/TestSegmentReader.java"}});
    assert_count_agrees_with_search(&idx, q.clone(), "keyword field").await;
    assert_eq!(count_total(&idx, &q).await, 1, "keyword term must count 1");
    assert_eq!(
        count_total(
            &idx,
            &json!({"term": {"path": "LUCENE/TestSegmentReader.java"}})
        )
        .await,
        0,
        "keyword term must stay case-sensitive"
    );
}

/// The abandon is not a licence to undercount: a `keyword` field with
/// `doc_values: false` takes the very same no-column branch and must still
/// count exactly one document per spelling, case-sensitively.
#[tokio::test]
async fn keyword_without_doc_values_still_counts_exactly() {
    let (_dir, idx) = seed().await;
    for (spelling, expected) in [("AB-1234", 1), ("ab-1234", 1), ("zz-9", 0)] {
        let q = json!({"term": {"code": spelling}});
        assert_count_agrees_with_search(&idx, q.clone(), "keyword, doc_values:false").await;
        assert_eq!(
            count_total(&idx, &q).await,
            expected,
            "keyword without doc-values: wrong count for {spelling}"
        );
    }
}
