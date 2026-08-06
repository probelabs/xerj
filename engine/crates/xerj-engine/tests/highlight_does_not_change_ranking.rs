//! Regression test for #177 — a `highlight` block must not change `_score`
//! or hit order.
//!
//! `highlight` is a presentation concern: it is applied by `apply_highlight`
//! as a post-pass over the already-selected, already-scored page.  On the
//! pre-fix build its mere presence widened the executor's `materialisation_limit`
//! (the arrival-order cap on the bounded hit collector), and because the
//! cross-segment merge admitted hits FIFO rather than by score, a wider cap
//! produced a *different* page.  Adding `"highlight"` therefore changed every
//! `_score` and reordered — indeed replaced — the hits.
//!
//! The test builds two segments where the second holds the genuinely
//! top-scoring documents and the first holds enough matches to fill a small
//! page on its own, then asserts:
//!   1. the page is the same with and without `highlight` (ids + scores), and
//!   2. that page is the true global top-k (matches a full-materialisation
//!      run at `size` >= total matches), and
//!   3. highlighting still produces fragments containing the matched term.

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

fn body(size: usize, highlight: bool) -> xerj_query::ast::SearchRequest {
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

#[tokio::test]
async fn highlight_block_does_not_change_scores_or_order() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("hl", Schema::empty()).unwrap();
    let idx = engine.get_index("hl").unwrap();

    // ── Segment 0: 60 long, weakly-matching docs ─────────────────────────
    // One occurrence of the term buried in a long body → low BM25 (long
    // field norm).  60 of them is more than enough to fill a size:5 page on
    // its own, which is exactly what the pre-fix FIFO merge did.
    let filler =
        "the server allocates a buffer and appends the reply object to the client output list "
            .repeat(12);
    for i in 0..60 {
        idx.index_document(
            Some(format!("weak{i:02}")),
            json!({"body": format!("{filler} quicklist {filler}")}),
        )
        .await
        .unwrap();
    }
    idx.flush().await.unwrap();

    // ── Segment 1: 5 short, strongly-matching docs ───────────────────────
    // Short field + repeated term → high BM25.  These are the true top-5.
    // Strictly decreasing term frequency keeps the ranking tie-free, so
    // "the top-5" has exactly one right answer.
    for i in 0..5 {
        let body = "quicklist ".repeat(10 - i) + &"filler ".repeat(i * 4);
        idx.index_document(Some(format!("strong{i}")), json!({"body": body}))
            .await
            .unwrap();
    }
    idx.flush().await.unwrap();

    // Ground truth: materialise every match, then take the top 5.
    let full = idx.search(&body(200, false)).await.unwrap();
    assert_eq!(full.total.value, 65, "all 65 docs match");
    let mut truth = page(&full.hits);
    truth.truncate(5);
    assert!(
        truth.iter().all(|(id, _)| id.starts_with("strong")),
        "sanity: the 5 short docs must be the global top-5, got {truth:?}"
    );

    // ── The invariant under test ─────────────────────────────────────────
    let plain = idx.search(&body(5, false)).await.unwrap();
    let lit = idx.search(&body(5, true)).await.unwrap();

    assert_eq!(
        page(&plain.hits),
        page(&lit.hits),
        "adding a `highlight` block changed the page: without={:?} with={:?}",
        page(&plain.hits),
        page(&lit.hits)
    );
    assert_eq!(
        page(&plain.hits),
        truth,
        "size:5 page is not the global top-5"
    );
    assert_eq!(
        plain.total.value, lit.total.value,
        "`highlight` changed hits.total"
    );

    // ── ...and highlighting still highlights ─────────────────────────────
    for h in &lit.hits {
        let frags = h
            .highlight
            .as_ref()
            .and_then(|m| m.get("body"))
            .unwrap_or_else(|| panic!("hit {} has no `body` highlight", h.id));
        assert!(!frags.is_empty(), "hit {} has zero fragments", h.id);
        assert!(
            frags.iter().any(|f| f.contains("<em>quicklist</em>")),
            "hit {} fragments do not mark the matched term: {frags:?}",
            h.id
        );
    }
    assert!(
        plain.hits.iter().all(|h| h.highlight.is_none()),
        "hits carried a `highlight` key without a highlight block"
    );
}

/// The same invariant one level up, through the ES `_search` request shape
/// used in the issue: `multi_match` over several fields, with the highlight
/// block naming a concrete field.
#[tokio::test]
async fn highlight_block_does_not_change_multi_match_ranking() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("hl2", Schema::empty()).unwrap();
    let idx = engine.get_index("hl2").unwrap();

    let filler =
        "generic prose about clients connections buffers and replies that dilutes the term "
            .repeat(10);
    for i in 0..40 {
        idx.index_document(
            Some(format!("weak{i:02}")),
            json!({"title": format!("file {i}"), "body": format!("{filler} listpack {filler}")}),
        )
        .await
        .unwrap();
    }
    idx.flush().await.unwrap();
    // Strictly decreasing term frequency → strictly decreasing scores, so the
    // top-3 is unambiguous (a tie straddling the page boundary is a different
    // question — which of several equal-scoring docs to keep — and not what
    // this test is about).
    for i in 0..4 {
        let body = "listpack ".repeat(8 - i) + &"filler ".repeat(i * 6);
        idx.index_document(
            Some(format!("strong{i}")),
            json!({"title": "listpack", "body": body}),
        )
        .await
        .unwrap();
    }
    idx.flush().await.unwrap();

    let mk = |size: usize, highlight: bool| {
        let mut b = json!({
            "size": size,
            "query": {"multi_match": {"query": "listpack", "fields": ["body", "title"]}}
        });
        if highlight {
            b.as_object_mut()
                .unwrap()
                .insert("highlight".into(), json!({"fields": {"body": {}}}));
        }
        parse_request(&b).expect("parse_request")
    };

    let full = idx.search(&mk(200, false)).await.unwrap();
    let mut truth = page(&full.hits);
    truth.truncate(3);

    let plain = idx.search(&mk(3, false)).await.unwrap();
    let lit = idx.search(&mk(3, true)).await.unwrap();

    assert_eq!(
        page(&plain.hits),
        page(&lit.hits),
        "multi_match: `highlight` changed the page"
    );
    assert_eq!(page(&plain.hits), truth, "multi_match: page is not top-3");
}

/// Selection must not depend on the page size either: the top-1 of a
/// `size:1` request is the top-1 of a `size:200` request.  This is the same
/// arrival-order-cap defect seen from the other side, and it is what made
/// `highlight` (which only widened the cap) look like a scoring knob.
#[tokio::test]
async fn top_hit_is_stable_across_page_sizes() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    engine.create_index("hl3", Schema::empty()).unwrap();
    let idx = engine.get_index("hl3").unwrap();

    let filler = "padding words that make the field long and the norm large ".repeat(10);
    for i in 0..80 {
        idx.index_document(
            Some(format!("weak{i:02}")),
            json!({"body": format!("{filler} ziplist")}),
        )
        .await
        .unwrap();
    }
    idx.flush().await.unwrap();
    idx.index_document(Some("best".into()), json!({"body": "ziplist ziplist"}))
        .await
        .unwrap();
    idx.flush().await.unwrap();

    let q = |size: usize| -> xerj_query::ast::SearchRequest {
        parse_request(&json!({"size": size, "query": {"match": {"body": "ziplist"}}}))
            .expect("parse_request")
    };

    let deep = idx.search(&q(200)).await.unwrap();
    let expected: Value = json!([deep.hits[0].id.clone(), deep.hits[0].score]);
    assert_eq!(deep.hits[0].id, "best", "sanity: `best` is the top hit");

    for size in [1usize, 2, 5, 10, 50] {
        let r = idx.search(&q(size)).await.unwrap();
        let got: Value = json!([r.hits[0].id.clone(), r.hits[0].score]);
        assert_eq!(got, expected, "size:{size} returned a different top hit");
    }
}
