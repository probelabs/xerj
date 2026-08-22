//! Issue #464: `max_query_memory_mb` does not cover aggregation memory.
//!
//! The per-query memory guard (`governor::check_query_alloc`) is called only for
//! the search result window (`from + size` hits). An aggregation request runs
//! with `size: 0`, so that estimate is 0 and the guard passes — yet the brute
//! full-corpus agg path deep-clones EVERY memtable (and segment) source into an
//! owned `Value` to feed `run_aggs_with_all`. Under a bulk writer the memtable
//! holds 10^4-10^5 docs, so a single agg allocated multiples of the configured
//! limit with the guard none the wiser.
//!
//! This path is reached by ANY agg that bails the columnar fast path: a
//! meta-observing `top_hits` (which needs `_id`/source per bucket), and equally
//! the no-meta families with no columnar path at all — `percentiles`,
//! `date_histogram`, `cardinality`. The fix estimates the corpus materialisation
//! (memtable + flushed-segment docs) up front and rejects before the clone.
//!
//! This test installs a low `max_query_memory_mb`, fills the memtable with more
//! documents than that budget can materialise, and asserts BOTH a `top_hits`
//! agg (meta-observing) and a `percentiles` agg (no meta fields) are REJECTED —
//! the single corpus guard must bind the whole family, not just meta aggs.
//! Fail-before: without the guard either request returns 200 — the point of #464.
//!
//! Own test binary on purpose: `Engine::new` installs `max_query_memory_mb` in
//! the process-wide governor static, so no other suite may observe this budget.

use serde_json::json;
use tempfile::TempDir;
use xerj_common::config::Config;
use xerj_common::types::Schema;
use xerj_engine::Engine;
use xerj_query::parse_request;

/// 1 MiB budget. The corpus estimate is `corpus_docs * per-doc bytes`; well
/// under a thousand buffered docs already exceeds this.
const BUDGET_MB: u64 = 1;
/// Enough buffered docs that the corpus materialisation estimate clears the
/// 1 MiB budget, but few enough to stay in the memtable (no flush) and below
/// `FAST_AGG_MIN_DOCS` — so every agg here takes the brute full-corpus path the
/// guard protects.
const DOCS: usize = 900;

fn low_budget_engine(dir: &TempDir) -> Engine {
    let mut config = Config::default();
    config.server.data_dir = dir.path().to_str().unwrap().to_string();
    config.limits.max_query_memory_mb = BUDGET_MB;
    Engine::new(config).expect("engine::new installs the governor from config")
}

#[tokio::test]
async fn aggregation_corpus_is_bound_by_max_query_memory_mb() {
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

    // A `size:0` `top_hits` agg (meta-observing) bails the fast path to the
    // brute full-corpus assembly, which deep-clones all DOCS memtable sources —
    // an estimate that exceeds the 1 MiB budget.
    let top_hits = parse_request(&json!({
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
    assert!(
        idx.search(&top_hits).await.is_err(),
        "a top_hits agg over {DOCS} buffered docs must be rejected by \
         max_query_memory_mb={BUDGET_MB}MB — the full-corpus deep-clone allocates \
         far beyond the budget (#464). Got Ok, so the agg path bypassed the guard."
    );

    // A `percentiles` agg has NO columnar fast path and mentions NO meta field —
    // it bails to the SAME brute full-corpus assembly. It proves the guard binds
    // the whole agg family, not just meta-observing aggs (#464).
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
