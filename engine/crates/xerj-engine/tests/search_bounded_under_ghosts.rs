//! Regression test for #179 — a scored FTS search over an index that has
//! ghosts (a delete or an overwrite anywhere in its history) must still
//! return the true global top-k, and the bounded hit collector must stay
//! O(cap) while doing it.
//!
//! ## What #179 was
//!
//! `deletes_present` (any flushed tombstone, or `ghost_events() > 0`) forces
//! the per-segment FTS cap to `usize::MAX`, so `search_bounded` hands the
//! merge a `seg_hits` list spanning EVERY physical match — the cap can no
//! longer bound it, because with ghosts a top-`cap`-by-score slice could be
//! partly tombstoned and underfill the page.  `ghost_events()` is monotonic
//! and index-global, so after a single delete/overwrite ANYWHERE this branch
//! is permanent for every later query.
//!
//! Before the fix the cross-segment merge only bounded itself when the cap
//! bounded `seg_hits`: the fill-up segment left `page_worst == None` for its
//! whole run, so it hydrated `_source` for every one of the O(matches) hits
//! and grew `all_hits` to O(matches) before the end-of-run trim.  On any
//! mature index (one that had ever seen a delete) a `size:5` scored query
//! therefore paid O(total_matches) source parses and memory — the ~8×
//! read regression this test guards against.
//!
//! ## What this test can and cannot see
//!
//! The public API exposes no "documents hydrated" counter, so the O(cap)
//! bound itself is not directly observable here (documented limitation).
//! What IS observable, and what the pre-fix path would only satisfy at
//! O(matches) cost, is the *contract* the bound must preserve while it trims:
//! on a deliberately large match set (well above the 256-doc cap) with ghosts
//! present, a small `size` must still return the exact global top-k by score,
//! must exclude deleted/superseded ghosts, and must not move when a
//! `highlight` block is added (#177) or when the page size changes.  The bound
//! itself is enforced structurally in `index.rs` by the incremental
//! `page_worst` early-out plus the 2×cap eager trim; these assertions pin the
//! behaviour that bound must not break.

use serde_json::json;
use std::sync::Arc;
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

/// A weak (low-BM25) body: a single buried occurrence of the term in a long
/// field.  The field GROWS with `i`, so BM25 length-normalisation gives every
/// weak doc a DISTINCT, strictly decreasing score — all far below the short
/// strong docs.  The distinct scores matter: they are what let the collector's
/// strict early-out fire at the cap instead of hydrating the whole match set.
fn weak_body(i: usize) -> String {
    let pad = "pad ".repeat(40 + i * 3);
    format!("{pad} quicklist {pad}")
}

fn match_body(size: usize, highlight: bool) -> xerj_query::ast::SearchRequest {
    let mut b = json!({
        "size": size,
        "query": {"match": {"body": "quicklist"}}
    });
    if highlight {
        b.as_object_mut()
            .unwrap()
            .insert("highlight".into(), json!({"fields": {"body": {}}}));
    }
    parse_request(&b).expect("parse_request")
}

/// `(id, score)` page fingerprint.
fn page(hits: &[xerj_query::Hit]) -> Vec<(String, f32)> {
    hits.iter().map(|h| (h.id.clone(), h.score)).collect()
}

/// Build the corpus: a big low-scoring segment (`weak` docs, distinct
/// decreasing scores) plus a small high-scoring segment (5 `strong` docs whose
/// members are the true, tie-free top-5).  `weak` is chosen > 256 by callers so
/// the match set exceeds the collector cap.
async fn build_corpus(idx: &Arc<Index>, weak: usize) {
    // ── Segment 0: many weak matches, distinct decreasing scores ─────────
    for i in 0..weak {
        idx.index_document(Some(format!("weak{i:03}")), json!({"body": weak_body(i)}))
            .await
            .unwrap();
    }
    idx.flush().await.unwrap();

    // ── Segment 1: 5 strong, strictly-decreasing-score matches ───────────
    // Short field + repeated term → high BM25.  Strictly decreasing term
    // frequency keeps the top-5 tie-free, so it has exactly one right answer.
    for i in 0..5 {
        let body = "quicklist ".repeat(10 - i) + &"filler ".repeat(i * 4);
        idx.index_document(Some(format!("strong{i}")), json!({"body": body}))
            .await
            .unwrap();
    }
    idx.flush().await.unwrap();
}

/// The core #179 guard: a large match set (> cap) with ghosts present,
/// `size:5` must return the exact global top-5, exclude the ghosts, and stay
/// #177-stable.
#[tokio::test]
async fn size5_returns_global_top5_under_ghosts_on_large_matchset() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("g1", Schema::empty()).unwrap();
    let idx = engine.get_index("g1").unwrap();

    // 360 weak + 5 strong = 365 physical matches — comfortably over the 256
    // collector cap that `deletes_present` pins the query to.
    build_corpus(&idx, 360).await;

    // ── Introduce ghosts ─────────────────────────────────────────────────
    // A DELETE (tombstone) and an OVERWRITE (superseded old version) — both
    // land in already-flushed segment 0, so their stale copies are ghosts the
    // scan must skip, and either one alone flips `deletes_present` (hence
    // `fts_cap == usize::MAX`) permanently for every query below.
    assert!(
        idx.delete_document("weak000").await.unwrap(),
        "weak000 should have existed to delete"
    );
    // Overwrite weak001: its live version moves to the memtable, its segment-0
    // copy becomes a superseded ghost that the scan must not double-count.
    idx.index_document(Some("weak001".into()), json!({"body": weak_body(1)}))
        .await
        .unwrap();

    // Live matches: 360 weak - 1 deleted = 359 (weak001 overwritten, still
    // matches) + 5 strong = 364.
    let expected_total: u64 = 364;

    // ── Ground truth: full materialisation, then the top 5 ───────────────
    let full = idx.search(&match_body(500, false)).await.unwrap();
    assert_eq!(
        full.total.value, expected_total,
        "live total wrong under ghosts (deleted/superseded copies leaked into the count)"
    );
    let mut truth = page(&full.hits);
    truth.truncate(5);
    assert!(
        truth.iter().all(|(id, _)| id.starts_with("strong")),
        "sanity: the 5 short docs must be the global top-5, got {truth:?}"
    );
    assert!(
        full.hits.iter().all(|h| h.id != "weak000"),
        "deleted doc weak000 leaked into results"
    );
    assert!(
        full.hits.iter().filter(|h| h.id == "weak001").count() <= 1,
        "overwritten doc weak001 returned as a duplicate (stale ghost not skipped)"
    );

    // ── The #179 assertion: the bounded `size:5` page == the true top-5 ──
    // On the pre-fix code this same answer required hydrating all 364 live
    // matches; the bound must produce it while trimming to O(cap).
    let small = idx.search(&match_body(5, false)).await.unwrap();
    assert_eq!(
        small.total.value, expected_total,
        "size:5 reported a different total than the full run under ghosts"
    );
    assert_eq!(
        page(&small.hits),
        truth,
        "size:5 page is not the global top-5 under ghosts"
    );

    // ── #177 must still hold on the deletes-present path ─────────────────
    let lit = idx.search(&match_body(5, true)).await.unwrap();
    assert_eq!(
        page(&small.hits),
        page(&lit.hits),
        "adding `highlight` changed the page under ghosts: without={:?} with={:?}",
        page(&small.hits),
        page(&lit.hits)
    );
    for h in &lit.hits {
        let frags = h
            .highlight
            .as_ref()
            .and_then(|m| m.get("body"))
            .unwrap_or_else(|| panic!("hit {} has no `body` highlight", h.id));
        assert!(
            frags.iter().any(|f| f.contains("<em>quicklist</em>")),
            "hit {} fragments do not mark the matched term: {frags:?}",
            h.id
        );
    }
}

/// The #179 re-expand path, exercised end to end: a page that reaches PAST the
/// strong docs and INTO the big, truncated, ghost-bearing segment.  Under the
/// observer fix that segment is scanned bounded (heap = cap) but re-expands
/// uncapped because it truncated (`seg_total > cap`) AND holds ghosts
/// (`dead_matches > 0`), so the walk still sees every live candidate.  Without
/// the re-expand the top-`cap`-by-score slice — whose highest members here are
/// the very docs that were deleted/overwritten — would drop live hits and the
/// page would be wrong.  Asserted black-box against a full-materialisation
/// ground truth, so it holds whatever the exact BM25 order turns out to be.
#[tokio::test]
async fn page_reaching_into_truncated_ghost_segment_is_exact() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("g3", Schema::empty()).unwrap();
    let idx = engine.get_index("g3").unwrap();

    // 360 weak (distinct DECREASING scores: weak000 is the highest-scoring
    // weak doc) + 5 strong.  Match set 365 > 256 cap.
    build_corpus(&idx, 360).await;

    // Ghost the TWO highest-scoring weak docs — exactly the ones a bounded
    // top-cap slice keeps first, so a broken bound would surface them.
    assert!(idx.delete_document("weak000").await.unwrap());
    idx.index_document(Some("weak001".into()), json!({"body": weak_body(1)}))
        .await
        .unwrap();

    let expected_total: u64 = 364;

    // Ground truth: full materialisation.
    let full = idx.search(&match_body(500, false)).await.unwrap();
    assert_eq!(
        full.total.value, expected_total,
        "live total wrong under ghosts"
    );

    // Pages that reach well into the weak segment must equal the ground-truth
    // prefix — this is the assertion the re-expand exists to satisfy.
    for size in [10usize, 25, 50, 100] {
        let mut truth = page(&full.hits);
        truth.truncate(size);
        let r = idx.search(&match_body(size, false)).await.unwrap();
        assert_eq!(
            r.total.value, expected_total,
            "size:{size} total drifted under ghosts"
        );
        assert_eq!(
            page(&r.hits),
            truth,
            "size:{size} page reaching into the truncated ghost segment != global order"
        );
        assert!(
            r.hits.iter().all(|h| h.id != "weak000"),
            "size:{size}: deleted weak000 leaked into the page"
        );
        assert!(
            r.hits.iter().filter(|h| h.id == "weak001").count() <= 1,
            "size:{size}: overwritten weak001 returned as a duplicate (stale ghost not skipped)"
        );
    }
}

/// Cap-independence must hold on the `deletes_present` path too: the top hit of
/// a `size:1` request is the top hit of a `size:200` request, unchanged by the
/// presence of ghosts.  This is the incremental `page_worst` doing its job — a
/// worst-kept score frozen at segment entry would let the fill-up segment's
/// arrival order leak into small pages.
#[tokio::test]
async fn top_hit_stable_across_page_sizes_under_ghosts() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("g2", Schema::empty()).unwrap();
    let idx = engine.get_index("g2").unwrap();

    build_corpus(&idx, 300).await;

    // One delete → `deletes_present` is now permanently true.
    assert!(idx.delete_document("weak100").await.unwrap());

    let deep = idx.search(&match_body(400, false)).await.unwrap();
    let expected_top = (deep.hits[0].id.clone(), deep.hits[0].score);
    assert_eq!(
        expected_top.0, "strong0",
        "sanity: strong0 is the global top hit"
    );

    for size in [1usize, 2, 5, 10, 50, 100] {
        let r = idx.search(&match_body(size, false)).await.unwrap();
        let got = (r.hits[0].id.clone(), r.hits[0].score);
        assert_eq!(
            got, expected_top,
            "size:{size} returned a different top hit under ghosts (bounded merge lost the global top-k)"
        );
    }
}

/// Stress guard for the re-expand fallback at scale: a big single segment
/// (well over the cap) whose HIGHEST-scoring doc is a ghost, so every page that
/// reaches the top must skip it via an uncapped re-expand and still return the
/// exact global order.  This is the large-match-set analogue of
/// `page_reaching_into_truncated_ghost_segment_is_exact`; it exists because the
/// bounded scan sheds the pre-#179 full-set sort/allocation here — the win that
/// grows with the match-set size — and this pins the correctness that shedding
/// must preserve.
#[tokio::test]
async fn large_single_segment_with_top_ghost_returns_exact_order() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("g4", Schema::empty()).unwrap();
    let idx = engine.get_index("g4").unwrap();

    // 1500 distinct-scoring matches in ONE segment; weak0000 is the top.
    let n = 1500usize;
    for i in 0..n {
        idx.index_document(Some(format!("weak{i:04}")), json!({"body": weak_body(i)}))
            .await
            .unwrap();
    }
    idx.flush().await.unwrap();

    // Delete the top-scorer (inside the kept top-cap → forces the re-expand)
    // and overwrite the runner-up (superseded ghost).
    assert!(idx.delete_document("weak0000").await.unwrap());
    idx.index_document(Some("weak0001".into()), json!({"body": weak_body(1)}))
        .await
        .unwrap();

    let full = idx.search(&match_body(500, false)).await.unwrap();
    assert_eq!(
        full.total.value,
        (n as u64) - 1,
        "live total wrong under a top ghost"
    );
    for size in [10usize, 50, 200] {
        let mut truth = page(&full.hits);
        truth.truncate(size);
        let r = idx.search(&match_body(size, false)).await.unwrap();
        assert_eq!(
            page(&r.hits),
            truth,
            "size:{size} order wrong with a top-scoring ghost"
        );
        assert!(
            r.hits.iter().all(|h| h.id != "weak0000"),
            "deleted top-scorer leaked"
        );
    }
}
