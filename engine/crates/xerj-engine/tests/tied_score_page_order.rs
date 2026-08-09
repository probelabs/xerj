//! Regression test for #191 — tied `_score`s must resolve to ONE total order,
//! and every bounded page must be a prefix of the full materialisation.
//!
//! ES/Lucene make the top-k comparator total: `HitQueue.lessThan` breaks an
//! equal-score tie by the smaller `doc` id, and leaves are visited in
//! ascending global doc-id order, so the same document wins the tie no matter
//! how many hits were collected
//! (`lucene/core/src/java/org/apache/lucene/search/HitQueue.java:76-82` and
//! `TopScoreDocCollector.java:122-124`).
//!
//! XERJ ties by `seq_no` (arrival order — its `_doc` analogue) but does NOT
//! walk documents in `seq_no` order: `IndexSnapshot::segments` is in
//! flush/merge COMPLETION order (a flush drains each memtable shard into its
//! own segment; a merge appends its output at the end), and an FTS segment
//! returns its hits in stored-layout order.  Both bounded collectors therefore
//! used to resolve ties by *where the walk happened to reach the cap*:
//!
//!   * the scored FTS collector rejected a later segment whose best score only
//!     TIED the worst kept hit (`>` instead of a full-key comparison), and
//!   * the count-authoritative stored scan simply `break`ed once the collector
//!     was full, keeping the first `cap` matches in segment-list order.
//!
//! Measured pre-fix on the corpus below (12 byte-identical documents, one
//! exact score): `size:1000` returns doc00…doc23 in arrival order, while
//! `size:1` returned `doc02` on one release run and `doc01` on the next —
//! the answer is not merely wrong, it is not even STABLE, because the losing
//! tie-break is the racy segment-list order.  The partial-tie fixture below
//! returned `[top0, top1, tie03]` at `size:3` against `[top0, top1, tie00]`
//! at `size:500`.
//!
//! The properties asserted here are the ones a paginating client relies on:
//!   1. every `size:k` page equals the first `k` of the full page,
//!   2. walking the corpus one hit at a time with `from` reproduces it, and
//!   3. the answer does not depend on how many hits were requested.

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

fn req(from: usize, size: usize, q: &Value) -> xerj_query::ast::SearchRequest {
    parse_request(&json!({"from": from, "size": size, "query": q})).expect("parse_request")
}

/// 12 byte-identical documents over three flushes.  Identical text means an
/// exact BM25 tie on every scored path and an exact constant on every
/// filter path — the shape #191 is about.
async fn tied_corpus(idx: &std::sync::Arc<xerj_engine::index::Index>) -> Vec<String> {
    let mut ids = Vec::new();
    for batch in 0..3 {
        for i in 0..4 {
            let id = format!("doc{batch}{i}");
            idx.index_document(
                Some(id.clone()),
                json!({
                    "body": "listpack encoding for small collections",
                    "kind": "same",
                }),
            )
            .await
            .unwrap();
            ids.push(id);
        }
        idx.flush().await.unwrap();
    }
    ids
}

fn page_ids(hits: &[xerj_query::Hit]) -> Vec<String> {
    hits.iter().map(|h| h.id.clone()).collect()
}

#[tokio::test]
async fn tied_scores_bounded_pages_are_prefixes_of_the_full_page() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("tie", Schema::empty()).unwrap();
    let idx = engine.get_index("tie").unwrap();
    let inserted = tied_corpus(&idx).await;

    // Each shape exercises a different collector:
    //   match           → scored FTS admission walk (`page_worst`)
    //   term            → count-authoritative stored scan (`scan_page_worst`)
    //   match_all       → columnar top-k
    //   constant_score  → columnar top-k with the wrapper's constant score
    for (label, q) in [
        ("match", json!({"match": {"body": "listpack"}})),
        ("term", json!({"term": {"kind": "same"}})),
        ("match_all", json!({"match_all": {}})),
        (
            "constant_score",
            json!({"constant_score": {"filter": {"match_all": {}}}}),
        ),
    ] {
        let full = idx.search(&req(0, 1000, &q)).await.unwrap();
        assert_eq!(full.total.value, 12, "{label}: every doc matches");
        let full_ids = page_ids(&full.hits);
        assert_eq!(
            full_ids.len(),
            12,
            "{label}: full materialisation returns all 12"
        );

        // The tie really is exact — otherwise the test proves nothing.
        let scores: Vec<f32> = full.hits.iter().map(|h| h.score).collect();
        assert!(
            scores.windows(2).all(|w| w[0].to_bits() == w[1].to_bits()),
            "{label}: fixture must produce an EXACT tie, got {scores:?}"
        );

        // Property 1 — the tie-break is arrival order (`_doc`), matching the
        // `(score DESC, seq_no ASC, _id ASC)` order the final page sort uses.
        assert_eq!(
            full_ids, inserted,
            "{label}: an all-tied page must come back in arrival order"
        );

        // Property 2 — every bounded page is a prefix of the full page.
        for size in 1..=12usize {
            let p = idx.search(&req(0, size, &q)).await.unwrap();
            assert_eq!(
                page_ids(&p.hits),
                full_ids[..size],
                "{label}: size:{size} page disagrees with the full materialisation"
            );
            assert_eq!(
                p.total.value, 12,
                "{label}: size:{size} total must stay exact"
            );
        }

        // Property 3 — paging one hit at a time reproduces the same order.
        let walked: Vec<String> = {
            let mut acc = Vec::new();
            for from in 0..12usize {
                let p = idx.search(&req(from, 1, &q)).await.unwrap();
                acc.extend(page_ids(&p.hits));
            }
            acc
        };
        assert_eq!(
            walked, full_ids,
            "{label}: from/size walk disagrees with the full materialisation"
        );
    }
}

/// The same property when the tie is only PARTIAL: a handful of documents
/// outscore the rest, and the tied block straddles the page boundary.  This is
/// the `multi_match` / `best_fields` shape from the issue report, where four
/// documents landed on exactly 2.859662 across a `size:3` boundary.
#[tokio::test]
async fn tie_straddling_a_page_boundary_resolves_the_same_way_at_every_size() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("straddle", Schema::empty()).unwrap();
    let idx = engine.get_index("straddle").unwrap();

    // Two clear winners (the term twice in a short body), then eight
    // documents that tie each other exactly.
    for i in 0..2 {
        idx.index_document(
            Some(format!("top{i}")),
            json!({"body": "quicklist quicklist"}),
        )
        .await
        .unwrap();
    }
    idx.flush().await.unwrap();
    for batch in 0..2 {
        for i in 0..4 {
            idx.index_document(
                Some(format!("tie{batch}{i}")),
                json!({"body": "quicklist entry"}),
            )
            .await
            .unwrap();
        }
        idx.flush().await.unwrap();
    }

    let q = json!({"match": {"body": "quicklist"}});
    let full = idx.search(&req(0, 500, &q)).await.unwrap();
    let full_ids = page_ids(&full.hits);
    assert_eq!(full.total.value, 10);

    // The tied block is real: the last eight hits share one score.
    let scores: Vec<f32> = full.hits.iter().map(|h| h.score).collect();
    assert!(
        scores[2..]
            .windows(2)
            .all(|w| w[0].to_bits() == w[1].to_bits()),
        "fixture must tie the trailing block, got {scores:?}"
    );

    for size in 1..=10usize {
        let p = idx.search(&req(0, size, &q)).await.unwrap();
        assert_eq!(
            page_ids(&p.hits),
            full_ids[..size],
            "size:{size} page disagrees with the full materialisation"
        );
    }
}

/// The same property with an UNFLUSHED tail — the shape a live index is in
/// between flushes, and the one a paginating client hits most often.
///
/// The corpus is deliberately larger than the collector's 256-hit floor
/// (`materialisation_limit = (from + size + 100).max(256)`; the `from + size`
/// narrowing applies only to an EMPTY memtable, so a mixed fixture always
/// pays the floor plus 100 hits of slack).  A smaller mixed fixture proves
/// nothing at all: the whole corpus fits under the cap, the bounded collector
/// never truncates, and every page agrees trivially.
///
/// Honest scope: unlike the two tests above, this one PASSED on the pre-fix
/// engine in the runs we measured — the 100-hit slack means truncation only
/// ever reached documents past the prefixes asserted here, so the pre-fix
/// walk got away with dropping whichever segment it reached last.  It earns
/// its place as a REGRESSION guard, not a reproducer: the fix changes both
/// the segment walk order and the per-segment headroom, and the memtable
/// (which seeds the collector past its cap before the first segment is even
/// opened) is where a mistake in either would surface first.
#[tokio::test]
async fn tied_scores_page_the_same_way_with_an_unflushed_memtable() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("mixed", Schema::empty()).unwrap();
    let idx = engine.get_index("mixed").unwrap();

    let mut inserted = Vec::new();
    for batch in 0..4 {
        for i in 0..80 {
            let id = format!("seg{batch}{i:03}");
            idx.index_document(
                Some(id.clone()),
                json!({"body": "listpack encoding", "kind": "same"}),
            )
            .await
            .unwrap();
            inserted.push(id);
        }
        idx.flush().await.unwrap();
    }
    // Deliberately NOT flushed — these stay in the memtable.
    for i in 0..40 {
        let id = format!("mem{i:03}");
        idx.index_document(
            Some(id.clone()),
            json!({"body": "listpack encoding", "kind": "same"}),
        )
        .await
        .unwrap();
        inserted.push(id);
    }
    let total = inserted.len() as u64; // 360 > the 256 floor

    for (label, q) in [
        ("match", json!({"match": {"body": "listpack"}})),
        ("term", json!({"term": {"kind": "same"}})),
        ("match_all", json!({"match_all": {}})),
    ] {
        let full = idx.search(&req(0, 1000, &q)).await.unwrap();
        let full_ids = page_ids(&full.hits);
        assert_eq!(full.total.value, total, "{label}: every doc matches");
        assert_eq!(
            full_ids, inserted,
            "{label}: an all-tied page must come back in arrival order, \
             segment documents before memtable ones"
        );
        // Sizes on both sides of the 256 floor.
        for size in [1usize, 5, 50, 255, 256, 257, 300, 359] {
            let p = idx.search(&req(0, size, &q)).await.unwrap();
            assert_eq!(
                page_ids(&p.hits),
                full_ids[..size],
                "{label}: size:{size} page disagrees with the full materialisation"
            );
        }
    }
}
