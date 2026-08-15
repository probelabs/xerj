//! Regression test for #401 — `sort` on an ES meta-field other than
//! `_score` / `_doc` / `_id` resolved to `null` on EVERY hit.
//!
//! `compute_sort_values` special-cased exactly `_score`, `_doc` and `_id`;
//! everything else went to `get_field_value(source, field)`, a lookup INSIDE
//! `_source`.  `_seq_no` / `_version` / `_primary_term` / `_index` are engine
//! metadata and are never literal `_source` keys, so the lookup missed on
//! every document and the whole result set tied on `[null]`.  The visible
//! damage is keyset pagination: `search_after: [null]` is never strictly
//! greater than the next hit's `[null]`, so a client paging on
//! `sort: [{"_seq_no":"asc"}]` — the consistent-full-scan pattern ES
//! documents, and the one this repo's own `wal_tap` shape uses — either
//! re-reads page one forever or stops after it, with no error either way.
//!
//! Lucene cannot express this bug: a sort key is read from the field's DOC
//! VALUES column, not from the stored document.  `SortedNumericSortField`
//! selects the document's representative value out of `SortedNumericDocValues`
//! (`lucene/core/src/java/org/apache/lucene/search/SortedNumericSortField.java:49-59`)
//! and `NumericComparator` compares those column values directly
//! (`lucene/core/src/java/org/apache/lucene/search/comparators/NumericComparator.java:47-63`).
//! xerj has no column for a meta-field, so `Index::meta_sort_value` resolves
//! it through the same version-map accessors that populate the response
//! meta-fields (`lookup_seq_no` / `lookup_version`).
//!
//! Two collectors had to agree for that to be enough, and the second one is
//! the trap: the memtable's bounded sort-candidate path
//! (`sort_candidates_numeric`) cuts the candidate set with a dv column keyed
//! by the sort field.  There is no `_seq_no` column, so it classified every
//! buffered doc as "missing the field" and handed back an arbitrary
//! `materialisation_limit`-sized prefix.  Real sort values over a wrong
//! candidate set is still a silently wrong page — the same defect wearing
//! the other hat — so `is_meta_sort_field` also gates that path.  The same
//! gate is what fixes `_id` at the cap: `_id` already resolved to a real
//! sort value, and `sort: [{"_id":"desc"}]` over a >cap memtable STILL
//! returned the wrong five documents on main.

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

fn sorted_req(field: &str, order: &str, size: usize) -> xerj_query::ast::SearchRequest {
    parse_request(&json!({
        "query": {"match_all": {}},
        "size": size,
        "sort": [{field: order}],
    }))
    .expect("parse_request")
}

/// Every hit's `sort` value for a meta-field must be the document's REAL
/// metadata, not `null` — and `_seq_no` must agree with `lookup_seq_no`,
/// the accessor the `_seq_no` response field is built from.
#[tokio::test]
async fn meta_field_sort_values_are_real_not_null() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("meta_sort", Schema::empty()).unwrap();
    let idx = engine.get_index("meta_sort").unwrap();

    for i in 0..8u64 {
        idx.index_document(Some(format!("d{i}")), json!({ "n": i }))
            .await
            .unwrap();
    }
    // A second write on one doc so `_version` is not constant across the
    // corpus: a fix that resolved `_version` to a hard-coded 1 would pass a
    // "not null" assertion but still not be reading per-doc metadata.
    idx.index_document(Some("d3".to_string()), json!({ "n": 3, "touched": true }))
        .await
        .unwrap();

    let res = idx.search(&sorted_req("_seq_no", "asc", 50)).await.unwrap();
    assert_eq!(res.hits.len(), 8, "corpus size");
    for hit in &res.hits {
        let expected = idx
            .lookup_seq_no(&hit.id)
            .unwrap_or_else(|| panic!("{} has no seq_no", hit.id));
        assert_eq!(
            hit.sort.first(),
            Some(&Value::from(expected)),
            "hit {} sort value must be its real _seq_no, got {:?}",
            hit.id,
            hit.sort
        );
    }
    // …and the page must actually be ordered by it.
    let seqs: Vec<u64> = res
        .hits
        .iter()
        .map(|h| h.sort[0].as_u64().expect("numeric _seq_no sort value"))
        .collect();
    let mut ascending = seqs.clone();
    ascending.sort_unstable();
    assert_eq!(seqs, ascending, "_seq_no asc page must be sorted by seq_no");
    assert_eq!(
        seqs.iter().collect::<std::collections::HashSet<_>>().len(),
        seqs.len(),
        "_seq_no is unique per live doc, so no two hits may tie"
    );

    // `_version`: real per-doc write counter (d3 was written twice).
    let res = idx
        .search(&sorted_req("_version", "desc", 50))
        .await
        .unwrap();
    assert_eq!(res.hits.len(), 8, "corpus size under _version sort");
    for hit in &res.hits {
        let expected = idx.lookup_version(&hit.id).expect("version");
        assert_eq!(
            hit.sort.first(),
            Some(&Value::from(expected)),
            "hit {} sort value must be its real _version",
            hit.id
        );
    }
    assert_eq!(
        res.hits[0].sort.first().and_then(Value::as_u64),
        Some(2),
        "the twice-written doc's _version sort value must be 2, not a constant"
    );
    assert_eq!(
        res.hits[0].id,
        "d3",
        "_version desc must put the twice-written doc first, got {:?}",
        res.hits.iter().map(|h| &h.id).collect::<Vec<_>>()
    );

    // `_primary_term` / `_index` are constant in a single-shard, single-index
    // engine, but they must be the CONSTANT the response meta-fields emit,
    // never `null` — a null key silently ties the whole result set.
    for (field, expected) in [
        ("_primary_term", Value::from(1u64)),
        ("_index", Value::from("meta_sort")),
    ] {
        let res = idx.search(&sorted_req(field, "asc", 50)).await.unwrap();
        // Non-vacuity: an empty page would satisfy the loop below.
        assert_eq!(res.hits.len(), 8, "sort on {field} must return the corpus");
        for hit in &res.hits {
            assert_eq!(
                hit.sort.first(),
                Some(&expected),
                "sort on {field} must resolve, got {:?} for {}",
                hit.sort,
                hit.id
            );
        }
    }
}

/// The reported impact: keyset pagination on `_seq_no`.  With `[null]` sort
/// values the cursor never advances, so this loop either re-reads page one
/// forever (caught by the iteration budget) or returns fewer than the whole
/// corpus.
#[tokio::test]
async fn search_after_on_seq_no_pages_the_whole_index() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("sa_seq", Schema::empty()).unwrap();
    let idx = engine.get_index("sa_seq").unwrap();

    const N: usize = 25;
    for i in 0..N {
        idx.index_document(Some(format!("d{i:03}")), json!({ "n": i }))
            .await
            .unwrap();
    }
    // Flush these to a segment, then buffer five more, so the paged walk
    // spans both the segment and the memtable source.
    idx.flush().await.unwrap();
    for i in N..(N + 5) {
        idx.index_document(Some(format!("d{i:03}")), json!({ "n": i }))
            .await
            .unwrap();
    }
    let total = N + 5;

    let mut collected: Vec<String> = Vec::new();
    let mut after: Option<Vec<Value>> = None;
    // Budget: enough pages to drain the corpus, few enough that a stuck
    // cursor trips it instead of hanging the suite.
    for _ in 0..(total + 5) {
        let mut body = json!({
            "query": {"match_all": {}},
            "size": 4,
            "sort": [{"_seq_no": "asc"}],
        });
        if let Some(ref a) = after {
            body["search_after"] = Value::Array(a.clone());
        }
        let res = idx.search(&parse_request(&body).unwrap()).await.unwrap();
        if res.hits.is_empty() {
            break;
        }
        after = res.hits.last().map(|h| h.sort.clone());
        collected.extend(res.hits.iter().map(|h| h.id.clone()));
        assert!(
            collected.len() <= total,
            "cursor is not advancing: {} ids collected from a {}-doc index \
             (first repeat page starts at {:?})",
            collected.len(),
            total,
            &collected[total.min(collected.len().saturating_sub(1))]
        );
    }

    let mut unique = collected.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        total,
        "_seq_no keyset pagination must visit every doc exactly once; \
         collected {} ids, {} unique",
        collected.len(),
        unique.len()
    );
    assert_eq!(collected.len(), total, "no doc may be returned twice");
}

/// The candidate-cut half.  `materialisation_limit` is
/// `max(from + size + 100, 256)`, so a memtable with more docs than that
/// exercises the bounded candidate path — which has no dv column to cut on
/// for a meta-field (nor for `_id`) and must therefore not be used.
#[tokio::test]
async fn meta_field_sort_is_exact_past_the_memtable_candidate_cap() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("cap_sort", Schema::empty()).unwrap();
    let idx = engine.get_index("cap_sort").unwrap();

    // > materialisation_limit (256) buffered docs, none flushed.
    const N: u64 = 2000;
    for i in 0..N {
        idx.index_document(Some(format!("d{i:04}")), json!({ "n": i }))
            .await
            .unwrap();
    }

    // Ids are lexicographically ordered with insertion order, so the
    // expected page is the same for `_seq_no` and `_id`.
    let newest: Vec<String> = (N - 5..N).rev().map(|i| format!("d{i:04}")).collect();
    let oldest: Vec<String> = (0..5).map(|i| format!("d{i:04}")).collect();

    for field in ["_seq_no", "_id"] {
        let res = idx.search(&sorted_req(field, "desc", 5)).await.unwrap();
        let ids: Vec<String> = res.hits.iter().map(|h| h.id.clone()).collect();
        assert_eq!(
            ids, newest,
            "sort {field} desc over a {N}-doc memtable must return the true \
             top 5, not an arbitrary prefix of the candidate cut"
        );

        let res = idx.search(&sorted_req(field, "asc", 5)).await.unwrap();
        let ids: Vec<String> = res.hits.iter().map(|h| h.id.clone()).collect();
        assert_eq!(ids, oldest, "sort {field} asc over a {N}-doc memtable");
    }

    // Control: a real `_source` field DOES have a memtable dv column, so the
    // bounded candidate path stays enabled for it and must stay correct.
    let res = idx.search(&sorted_req("n", "desc", 5)).await.unwrap();
    let ids: Vec<String> = res.hits.iter().map(|h| h.id.clone()).collect();
    assert_eq!(ids, newest, "control: plain numeric field sort");
}

/// Same guarantee once the docs live in a segment rather than the memtable —
/// the segment side resolves `_seq_no` through the version map too.
#[tokio::test]
async fn seq_no_sort_is_exact_after_flush() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine
        .create_index("flushed_sort", Schema::empty())
        .unwrap();
    let idx = engine.get_index("flushed_sort").unwrap();

    const N: u64 = 600;
    for i in 0..N {
        idx.index_document(Some(format!("d{i:04}")), json!({ "n": i }))
            .await
            .unwrap();
    }
    idx.flush().await.unwrap();

    let res = idx.search(&sorted_req("_seq_no", "desc", 5)).await.unwrap();
    let ids: Vec<String> = res.hits.iter().map(|h| h.id.clone()).collect();
    let newest: Vec<String> = (N - 5..N).rev().map(|i| format!("d{i:04}")).collect();
    assert_eq!(ids, newest, "flushed _seq_no desc page");
    for hit in &res.hits {
        assert_eq!(
            hit.sort.first().and_then(Value::as_u64),
            idx.lookup_seq_no(&hit.id),
            "flushed hit {} must carry its real _seq_no",
            hit.id
        );
    }
}
