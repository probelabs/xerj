//! Issue #464: `max_query_memory_mb` does not cover aggregation memory.
//!
//! The per-query memory guard (`governor::check_query_alloc`) is called only for
//! the search result window (`from + size` hits). An aggregation request runs
//! with `size: 0`, so that estimate is 0 and the guard passes — yet a
//! meta-observing agg (`top_hits`, or one targeting `_id`/`_index`/`_seq_no`)
//! deep-clones EVERY buffered memtable document to build its `MemDocs::Owned`
//! view (`fast_aggs.rs`). Under a bulk writer the memtable holds 10^4-10^5 docs,
//! so a single agg allocated 5.2x the configured limit with the guard none the
//! wiser.
//!
//! This test installs a low `max_query_memory_mb`, fills the memtable with more
//! documents than that budget can materialise, and asserts a `size: 0`
//! `top_hits` agg is REJECTED (the guard now estimates the owned-source
//! materialisation and trips before the allocation). Fail-before: without the
//! agg-path guard the same request returns 200 — the whole point of #464.
//!
//! Own test binary on purpose: `Engine::new` installs `max_query_memory_mb` in
//! the process-wide governor static, so no other suite may observe this budget.

use serde_json::json;
use tempfile::TempDir;
use xerj_common::config::Config;
use xerj_common::types::Schema;
use xerj_engine::Engine;
use xerj_query::parse_request;

/// 1 MiB budget. The owned-source estimate is `memtable_docs * per-doc bytes`;
/// well under a thousand buffered docs already exceeds this.
const BUDGET_MB: u64 = 1;
/// Enough buffered docs that the owned-source materialisation estimate clears
/// the 1 MiB budget, but few enough to stay in the memtable (no flush).
const DOCS: usize = 900;

fn low_budget_engine(dir: &TempDir) -> Engine {
    let mut config = Config::default();
    config.server.data_dir = dir.path().to_str().unwrap().to_string();
    config.limits.max_query_memory_mb = BUDGET_MB;
    Engine::new(config).expect("engine::new installs the governor from config")
}

#[tokio::test]
async fn top_hits_aggregation_is_bound_by_max_query_memory_mb() {
    let dir = TempDir::new().unwrap();
    let engine = low_budget_engine(&dir);
    engine.create_index("aggmem", Schema::empty()).unwrap();
    let idx = engine.get_index("aggmem").unwrap();

    for i in 0..DOCS {
        idx.index_document(
            Some(format!("d{i:05}")),
            json!({
                "cat": if i % 2 == 0 { "A" } else { "B" },
                "title": format!("doc number {i}"),
                "n": i as i64,
            }),
        )
        .await
        .unwrap();
    }

    // A control search WITHOUT aggs (size:0) must still pass — its window
    // estimate is 0, so this isolates the failure to the aggregation path.
    let control = parse_request(&json!({ "query": { "match_all": {} }, "size": 0 })).unwrap();
    assert!(
        idx.search(&control).await.is_ok(),
        "a plain size:0 search must not be blocked by the query-memory budget (#464)"
    );

    // A `size:0` `top_hits` agg forces `needs_owned_mem` → the memtable
    // deep-clone of all DOCS docs, whose estimate exceeds the 1 MiB budget.
    let agg = parse_request(&json!({
        "query": { "match_all": {} },
        "size": 0,
        "aggs": {
            "by_cat": {
                "terms": { "field": "cat", "size": 10 },
                "aggs": { "top": { "top_hits": { "size": 2, "_source": ["title"] } } }
            }
        }
    }))
    .unwrap();

    let result = idx.search(&agg).await;
    assert!(
        result.is_err(),
        "a top_hits agg over {DOCS} buffered docs must be rejected by \
         max_query_memory_mb={BUDGET_MB}MB — the owned-source deep-clone allocates \
         far beyond the budget (#464). Got Ok, so the agg path bypassed the guard."
    );

    // A `percentiles` agg has NO columnar fast path — it bails to the brute
    // full-corpus assembly, which deep-clones every memtable + segment source
    // into owned `Value`s. It does NOT mention meta fields, so the meta-keyed
    // guard alone missed this whole family; the corpus guard must bind it too.
    let percentiles = parse_request(&json!({
        "query": { "match_all": {} },
        "size": 0,
        "aggs": { "p": { "percentiles": { "field": "n" } } }
    }))
    .unwrap();
    assert!(
        idx.search(&percentiles).await.is_err(),
        "a percentiles agg (brute full-corpus deep-clone, no meta fields) must \
         ALSO be bound by max_query_memory_mb — not just meta-observing aggs (#464)"
    );
}
