//! Regression test for #437 — sorting on a field this engine cannot resolve
//! (any ES meta-field name besides `_score`/`_doc`/`_id` and the four #420
//! made resolvable, or an unmapped/misspelled field) must be REJECTED, not
//! silently answered with `null` on every hit.
//!
//! Before this fix, `compute_sort_values` sent every one of these fields to
//! a plain `get_field_value(source, field)` lookup, which returns `Null`
//! when the field is absent — the whole result set ties on `[null]`, HTTP
//! 200, and `search_after` paging on that field is stranded at page one
//! with no error anywhere.
//!
//! Issue #402 tried to catch this with a request-level denylist of
//! "known-unresolvable" field names and was refuted: xerj deliberately
//! permits a metadata-named key inside `_source` (`{"_seq_no": 1}` is a
//! valid document body, unlike real ES), so whether a name resolves depends
//! on the CORPUS, not the field name — a static list is guaranteed wrong on
//! some corpus. This fix checks the SCHEMA instead, which is the stable
//! thing real ES itself validates (`No mapping found for [x] in order to
//! sort on`).

use serde_json::{json, Value};
use tempfile::TempDir;
use xerj_common::config::Config;
use xerj_common::types::Schema;
use xerj_engine::Engine;
use xerj_query::parse_request;

fn make_engine(dir: &TempDir) -> Engine {
    let mut config = Config::default();
    config.server.data_dir = dir.path().to_str().unwrap().to_string();
    Engine::new(config).expect("engine::new")
}

fn sorted_req(field: &str, size: usize) -> xerj_query::ast::SearchRequest {
    parse_request(&json!({
        "query": {"match_all": {}},
        "size": size,
        "sort": [{field: "asc"}],
    }))
    .expect("parse_request")
}

fn reason_of(err: &xerj_engine::EngineError) -> String {
    match err {
        xerj_engine::EngineError::Common(xerj_common::XerjError::InvalidQuery { reason }) => {
            reason.clone()
        }
        other => panic!("expected EngineError::Common(InvalidQuery), got: {other:?}"),
    }
}

/// #402's own under-inclusion review is the ready-made census: every ES
/// meta-field name besides the ones already handled (`_score`, `_doc`,
/// `_id`, `_seq_no`, `_version`, `_primary_term`, `_index`), plus an
/// arbitrary unmapped/misspelled field.
const UNRESOLVABLE_FIELDS: &[&str] = &[
    "_source",
    "_size",
    "_doc_count",
    "_field_names",
    "_meta",
    "_tier",
    "_nested",
    "_nested_path",
    "_feature",
    "_parent",
    "_matched_queries",
    "not_a_field_at_all",
];

#[tokio::test]
async fn sort_on_an_unresolvable_field_is_rejected_not_stranded() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine
        .create_index("unresolvable", Schema::empty())
        .unwrap();
    let idx = engine.get_index("unresolvable").unwrap();
    for i in 0..6u64 {
        idx.index_document(Some(format!("d{i}")), json!({ "n": i }))
            .await
            .unwrap();
    }

    for field in UNRESOLVABLE_FIELDS {
        let result = idx.search(&sorted_req(field, 2)).await;
        let err = result.err().unwrap_or_else(|| {
            panic!(
                "sort on [{field}] must be rejected — the pre-fix behavior was HTTP 200 \
                 with every hit's sort value silently null, stranding search_after at \
                 page one (#437)"
            )
        });
        let reason = reason_of(&err);
        assert!(
            reason.contains(&format!("No mapping found for [{field}]")),
            "sort on [{field}] was rejected, but the reason doesn't name it: {reason}"
        );
    }
}

/// Control: sorting on a genuinely mapped field, or one of the seven
/// already-handled special cases, must keep working — this fix must not
/// turn every sort into a rejection.
#[tokio::test]
async fn sort_on_a_resolvable_field_still_works() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("resolvable", Schema::empty()).unwrap();
    let idx = engine.get_index("resolvable").unwrap();
    for i in 0..6u64 {
        idx.index_document(Some(format!("d{i}")), json!({ "n": i }))
            .await
            .unwrap();
    }

    // A dynamically-mapped ordinary field.
    let res = idx.search(&sorted_req("n", 10)).await.expect("sort on n");
    assert_eq!(res.hits.len(), 6);
    for hit in &res.hits {
        assert_ne!(hit.sort.first(), Some(&Value::Null), "n must not sort null");
    }

    // The already-handled special cases must still bypass the schema check
    // entirely (they're resolved from the hit/engine metadata, never from
    // `_source`, so they're never "in the schema" to begin with).
    for field in [
        "_score",
        "_doc",
        "_id",
        "_seq_no",
        "_version",
        "_primary_term",
        "_index",
    ] {
        idx.search(&sorted_req(field, 10))
            .await
            .unwrap_or_else(|e| panic!("sort on [{field}] must still work: {e}"));
    }
}

/// A dynamically-mapped `.keyword` multi-field, and a nested/object
/// property, must resolve via the schema too — proving the fix uses
/// `declared_field`'s recursive dotted-path walk, not a flat exact-match
/// lookup that would false-reject both.
#[tokio::test]
async fn sort_on_a_keyword_multi_field_and_a_nested_property_works() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("nested_sort", Schema::empty()).unwrap();
    let idx = engine.get_index("nested_sort").unwrap();
    for i in 0..4u64 {
        idx.index_document(
            Some(format!("d{i}")),
            json!({ "title": format!("t{i}"), "user": { "name": format!("u{i}") } }),
        )
        .await
        .unwrap();
    }

    idx.search(&sorted_req("title.keyword", 10))
        .await
        .expect("sort on a dynamic .keyword multi-field must work");
    idx.search(&sorted_req("user.name", 10))
        .await
        .expect("sort on a nested/object property must work");
}
