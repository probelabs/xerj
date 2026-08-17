//! Regression tests for issue #332: a multi-valued keyword field was
//! flattened into ONE FTS token.
//!
//! `{"tags": ["red", "blue"]}` on a `keyword` field used to be joined into the
//! single string `"red blue"` before the FTS layer saw it
//! (`memtable::extract_text_value` / `index::extract_field_text`, both doing
//! `arr.join(" ")`).  The keyword analyzer emits its whole input as one token,
//! so the segment carried the term `"red blue"` and neither `"red"` nor
//! `"blue"` existed as a posting.  Every clause that projects to a whole-value
//! `FtsQuery::Term` therefore missed the document once it was flushed.
//!
//! Measured on `main` at d8be09cf, one doc `{"tags": ["red","blue"]}`, `tags`
//! mapped `keyword` — the exact inverse of Elasticsearch, and the hit set
//! CHANGES at `_flush`:
//!
//! ```text
//! BEFORE flush | term tags=red   (first value)              -> 1 hit(s)
//! BEFORE flush | term tags=blue  (non-first value)          -> 0 hit(s)
//! BEFORE flush | term tags='red blue' (joined artefact)     -> 0 hit(s)
//! AFTER  flush | term tags=red   (first value)              -> 0 hit(s)
//! AFTER  flush | term tags=blue  (non-first value)          -> 0 hit(s)
//! AFTER  flush | term tags='red blue' (joined artefact)     -> 1 hit(s)
//! ```
//!
//! The pre-flush half had a second, independent cause: the memtable's
//! single-valued doc-values columns keep only an array's FIRST element, and
//! the fused columnar walk that serves a bare `term` never bailed out on such
//! a field the way its three sibling paths did.
//!
//! Nearly every case below asserts the hit set is the same BEFORE and AFTER
//! `flush()`: this class of defect is flush-dependent, and a post-flush-only
//! assertion would pass on a build that had simply stopped indexing the field.
//! The one deliberate exception is documented at its assertion.

use std::collections::BTreeSet;

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

fn req(q: Value) -> xerj_query::ast::SearchRequest {
    parse_request(&json!({ "query": q, "size": 50 })).expect("parse_request")
}

async fn ids(idx: &Index, q: &Value) -> BTreeSet<String> {
    idx.search(&req(q.clone()))
        .await
        .unwrap()
        .hits
        .iter()
        .map(|h| h.id.clone())
        .collect()
}

fn expect(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

/// Two docs, one index:
/// * `1` — multi-valued keyword `tags: ["red","blue"]`, multi-valued text
///   `notes: ["alpha bravo","charlie delta"]`
/// * `2` — the single-valued control, `tags: "red"`, `notes: "alpha bravo"`
async fn seed(engine: &Engine, name: &str) -> std::sync::Arc<Index> {
    let mut schema = Schema::empty();
    schema
        .fields
        .push(FieldConfig::new("body", FieldType::Text));
    schema
        .fields
        .push(FieldConfig::new("notes", FieldType::Text));
    schema
        .fields
        .push(FieldConfig::new("tags", FieldType::Keyword));
    engine.create_index(name, schema).unwrap();
    let idx = engine.get_index(name).unwrap();
    idx.index_document(
        Some("1".into()),
        json!({
            "body": "hello",
            "tags": ["red", "blue"],
            "notes": ["alpha bravo", "charlie delta"]
        }),
    )
    .await
    .unwrap();
    idx.index_document(
        Some("2".into()),
        json!({"body": "hello", "tags": "red", "notes": "alpha bravo"}),
    )
    .await
    .unwrap();
    idx
}

/// Run every case pre-flush and post-flush and require the same hit set.
async fn assert_flush_parity(idx: &std::sync::Arc<Index>, cases: &[(Value, &[&str], &str)]) {
    for (q, exp, label) in cases {
        assert_eq!(
            ids(idx, q).await,
            expect(exp),
            "{label}: PRE-flush (memtable) hit set wrong for {q}"
        );
    }
    idx.flush().await.unwrap();
    for (q, exp, label) in cases {
        assert_eq!(
            ids(idx, q).await,
            expect(exp),
            "{label}: POST-flush (segment) hit set wrong for {q}"
        );
    }
}

#[tokio::test]
async fn term_match_multi_match_see_every_keyword_array_element() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mvk_terms").await;

    assert_flush_parity(
        &idx,
        &[
            // The headline: an exact `term` for one element of the array.
            (json!({"term": {"tags": "red"}}), &["1", "2"], "term red"),
            (json!({"term": {"tags": "blue"}}), &["1"], "term blue"),
            // `match` on a keyword field lowers to the same whole-value term.
            (json!({"match": {"tags": "red"}}), &["1", "2"], "match red"),
            (json!({"match": {"tags": "blue"}}), &["1"], "match blue"),
            // `multi_match` — the clause that visibly missed doc 1 on `main`.
            (
                json!({"multi_match": {"query": "red", "fields": ["tags", "body"]}}),
                &["1", "2"],
                "multi_match red",
            ),
            (
                json!({"multi_match": {"query": "blue", "fields": ["tags", "body"]}}),
                &["1"],
                "multi_match blue",
            ),
            // `terms` takes the stored-scan route and always agreed with ES;
            // keep it as the in-binary control that the two routes now match.
            (
                json!({"terms": {"tags": ["blue"]}}),
                &["1"],
                "terms control",
            ),
            (
                json!({"bool": {"must": [{"match": {"body": "hello"}}],
                                "filter": [{"terms": {"tags": ["red"]}}]}}),
                &["1", "2"],
                "bool must+filter control",
            ),
        ],
    )
    .await;
}

/// The joined string must NOT survive as a term. Pre-fix `"red blue"` was
/// the ONLY term the segment held for doc 1, so this query matched it —
/// exactly backwards from Elasticsearch, where no keyword value equals
/// `"red blue"`.
#[tokio::test]
async fn joined_array_is_not_a_term() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mvk_joined").await;

    assert_flush_parity(
        &idx,
        &[(json!({"term": {"tags": "red blue"}}), &[], "term joined")],
    )
    .await;

    // `multi_match` for the joined string is asserted POST-flush only, and
    // deliberately so.  The segment is the side this fix owns, and it is now
    // right: no keyword value equals `"red blue"`, so nothing matches.  The
    // memtable answers `{"1","2"}` instead — but NOT because of arrays.  The
    // stored-source scan that serves a memtable-resident `multi_match` /
    // `match` whitespace-splits the query and ORs the tokens without ever
    // consulting the mapping, so it treats a `keyword` field as analyzed
    // `text`.  Doc 2 carries the SCALAR `tags: "red"` and diverges the same
    // way with no array anywhere in the index, which is what makes this a
    // separate defect rather than an unfinished corner of #332 — filed as
    // #354, whose fix wants its own ES-YAML run.
    idx.flush().await.unwrap();
    assert_eq!(
        ids(
            &idx,
            &json!({"multi_match": {"query": "red blue", "fields": ["tags"]}})
        )
        .await,
        expect(&[]),
        "POST-flush: the joined array must not be reachable as a multi_match term"
    );
}

/// A phrase must not span two elements of an array — Lucene separates them by
/// `position_increment_gap` (100), and so does the segment writer now.
/// Pre-fix BOTH sides said 1 hit for `"bravo charlie"` (measured on d8be09cf):
/// the joined string put `bravo` and `charlie` at adjacent positions in the
/// segment, and the stored-source scan matched the same joined text.
#[tokio::test]
async fn phrase_does_not_span_array_elements() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mvk_phrase").await;

    assert_flush_parity(
        &idx,
        &[
            (
                json!({"match_phrase": {"notes": "bravo charlie"}}),
                &[],
                "match_phrase across the boundary",
            ),
            (
                json!({"multi_match": {"query": "bravo charlie",
                                       "fields": ["notes"], "type": "phrase"}}),
                &[],
                "multi_match phrase across the boundary",
            ),
            // ... but a phrase WITHIN one element still matches, in both docs
            // for the first element and only doc 1 for the second.
            (
                json!({"match_phrase": {"notes": "alpha bravo"}}),
                &["1", "2"],
                "match_phrase inside element 0",
            ),
            (
                json!({"multi_match": {"query": "charlie delta",
                                       "fields": ["notes"], "type": "phrase"}}),
                &["1"],
                "multi_match phrase inside element 1",
            ),
        ],
    )
    .await;
}

/// A `terms` aggregation over a multi-valued keyword field buckets EVERY
/// value, and the answer does not change at flush.
#[tokio::test]
async fn terms_agg_buckets_every_array_element() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mvk_agg").await;

    let body = json!({
        "query": {"match_all": {}},
        "size": 0,
        "aggs": {"t": {"terms": {"field": "tags"}}}
    });
    let expected = json!([
        {"key": "red", "doc_count": 2},
        {"key": "blue", "doc_count": 1}
    ]);

    for state in ["PRE-flush", "POST-flush"] {
        if state == "POST-flush" {
            idx.flush().await.unwrap();
        }
        let res = idx.search(&parse_request(&body).unwrap()).await.unwrap();
        let buckets = res
            .aggs
            .as_ref()
            .and_then(|a| a["t"]["buckets"].as_array())
            .cloned()
            .unwrap_or_default();
        let trimmed: Vec<Value> = buckets
            .iter()
            .map(|b| json!({"key": b["key"], "doc_count": b["doc_count"]}))
            .collect();
        assert_eq!(
            Value::Array(trimmed),
            expected,
            "{state}: terms agg over a multi-valued keyword field"
        );
    }
}

/// `_source` is stored verbatim and must come back as the array it went in
/// as — the fix touches the inverted index, never the stored document.
#[tokio::test]
async fn source_round_trips_unchanged() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mvk_source").await;

    for state in ["PRE-flush", "POST-flush"] {
        if state == "POST-flush" {
            idx.flush().await.unwrap();
        }
        let res = idx
            .search(&req(json!({"term": {"tags": "blue"}})))
            .await
            .unwrap();
        assert_eq!(res.hits.len(), 1, "{state}: expected exactly doc 1");
        assert_eq!(
            res.hits[0].source["tags"],
            json!(["red", "blue"]),
            "{state}: _source must keep the original array"
        );
    }
}

/// The MERGE path must index arrays the same way the flush path does.
///
/// It used to carry its own copy of the extraction walk
/// (`index::extract_field_text`, joining with `" "`), so a merged segment
/// reproduced the bug even once a freshly flushed one had stopped having it —
/// a defect that only appears after a second flush plus a merge, which no
/// flush-only test can see. `extract_fts_fields_excluding` now delegates to
/// `memtable::extract_field_values_excluding`, the flush path's own walker.
#[tokio::test]
async fn merged_segments_keep_every_array_element() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mvk_merge").await;

    // Segment 1: docs 1 and 2.
    idx.flush().await.unwrap();
    // Segment 2: a third doc, so the merge has something to merge.
    idx.index_document(
        Some("3".into()),
        json!({"body": "hello", "tags": ["green", "blue"], "notes": "alpha bravo"}),
    )
    .await
    .unwrap();
    idx.flush().await.unwrap();

    let merged = idx.force_merge(1).await.unwrap();
    assert!(merged >= 1, "force_merge should have merged something");

    assert_eq!(
        ids(&idx, &json!({"term": {"tags": "blue"}})).await,
        expect(&["1", "3"]),
        "MERGED: a non-first array element must still be its own term"
    );
    assert_eq!(
        ids(&idx, &json!({"term": {"tags": "red"}})).await,
        expect(&["1", "2"]),
        "MERGED: the first array element and the scalar control"
    );
    assert_eq!(
        ids(&idx, &json!({"term": {"tags": "red blue"}})).await,
        expect(&[]),
        "MERGED: the joined string must not survive the merge as a term"
    );
    assert_eq!(
        ids(&idx, &json!({"match_phrase": {"notes": "bravo charlie"}})).await,
        expect(&[]),
        "MERGED: the position gap between array elements must survive the merge"
    );
}
