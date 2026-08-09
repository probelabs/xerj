//! Regression tests for issue #230: `multi_match` with `type: phrase` /
//! `phrase_prefix` must evaluate REAL positional phrase semantics — term
//! positions in the analyzed token stream — not whole-query lowercase
//! substring containment on the raw field text.
//!
//! Pre-fix both the memtable arm of `doc_matches_query` and the scoring
//! arm of `score_query_against_doc` tested `field_text.contains(query)`,
//! and the segment FTS projection DECLINED phrase types outright so the
//! stored-doc scan evaluated the same substring predicate. Substring
//! containment diverges from ES in both directions:
//!
//! * under-match — the analyzer strips punctuation, so ES's positional
//!   phrase matches `"merge policy"` against `merge, policy`; the raw
//!   text has no `"merge policy"` substring, so XERJ returned 0 hits;
//! * over-match — substring containment ignores token boundaries, so
//!   `"merge polic"` matched the doc `merge policy`, where ES's phrase
//!   terms (`merge`, `polic`) never line up;
//! * `slop` was parsed away entirely, so `{"type":"phrase","slop":2}`
//!   silently behaved as slop 0.
//!
//! Every case is asserted BOTH pre-flush (memtable) and post-flush
//! (segment), and cross-checked against the single-field `match_phrase` /
//! `match_phrase_prefix` queries, which have always been positional — ES
//! lowers `multi_match` phrase types to exactly a dis_max over per-field
//! `match_phrase` clauses.

use std::collections::BTreeSet;

use serde_json::{json, Value};
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

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

/// Run every case against the memtable, flush once, run every case again
/// against the segment, and assert the hit set is the expected one in both
/// states — semantics AND flush-invariance in one assertion.
async fn assert_both_states(idx: &std::sync::Arc<Index>, cases: &[(Value, &[&str], &str)]) {
    for (q, exp, label) in cases {
        let pre = ids(idx, q).await;
        assert_eq!(
            pre,
            set(exp),
            "{label}: PRE-flush (memtable) hit set for {q}"
        );
    }
    idx.flush().await.unwrap();
    for (q, exp, label) in cases {
        let post = ids(idx, q).await;
        assert_eq!(
            post,
            set(exp),
            "{label}: POST-flush (segment) hit set for {q}"
        );
    }
}

/// Doc 1 carries punctuation INSIDE the phrase (`merge, policy`) — the
/// standard analyzer drops it, so the phrase terms are adjacent even
/// though the raw text has no `"merge policy"` substring.
/// Doc 2 carries the bare phrase, so it exercises the over-match direction.
/// Doc 3 is a control that must never match.
async fn seed(engine: &Engine, name: &str) -> std::sync::Arc<Index> {
    engine.create_index(name, Schema::empty()).unwrap();
    let idx = engine.get_index(name).unwrap();
    idx.index_document(
        Some("1".into()),
        json!({
            "body": "the log merge, policy groups segments into buckets",
            "title": "log structured merge"
        }),
    )
    .await
    .unwrap();
    idx.index_document(
        Some("2".into()),
        json!({"body": "a tiered merge policy compacts segments", "title": "merge policy"}),
    )
    .await
    .unwrap();
    idx.index_document(
        Some("3".into()),
        json!({"body": "quick brown fox", "title": "animals"}),
    )
    .await
    .unwrap();
    idx
}

/// UNDER-MATCH: the analyzer strips the comma, so ES's positional phrase
/// matches doc 1. Pre-fix the substring test missed it in both states.
/// `match_phrase` on the same field is the in-repo oracle: it has always
/// been positional, and ES lowers `multi_match` phrase to exactly that.
#[tokio::test]
async fn phrase_matches_across_stripped_punctuation() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mmpp_punct").await;

    assert_both_states(
        &idx,
        &[
            (
                json!({"match_phrase": {"body": "merge policy"}}),
                &["1", "2"],
                "oracle match_phrase",
            ),
            (
                json!({"multi_match": {"query": "merge policy", "fields": ["body"],
                                       "type": "phrase"}}),
                &["1", "2"],
                "multi_match phrase, single field",
            ),
            (
                json!({"multi_match": {"query": "merge policy", "fields": ["body", "title"],
                                       "type": "phrase"}}),
                &["1", "2"],
                "multi_match phrase, two fields",
            ),
        ],
    )
    .await;
}

/// OVER-MATCH: `"merge polic"` is a substring of `merge policy` but the
/// analyzed terms never line up, so ES matches nothing. Pre-fix XERJ
/// returned doc 2 (and doc 1 for the `body` variant is impossible either
/// way — its raw text has no such substring).
#[tokio::test]
async fn phrase_does_not_match_partial_trailing_token() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mmpp_partial").await;

    assert_both_states(
        &idx,
        &[
            (
                json!({"match_phrase": {"title": "merge polic"}}),
                &[],
                "oracle match_phrase partial token",
            ),
            (
                json!({"multi_match": {"query": "merge polic", "fields": ["title"],
                                       "type": "phrase"}}),
                &[],
                "multi_match phrase partial token",
            ),
            // Reversed phrase must not match either (positional order).
            (
                json!({"multi_match": {"query": "policy merge", "fields": ["title", "body"],
                                       "type": "phrase"}}),
                &[],
                "multi_match phrase reversed",
            ),
        ],
    )
    .await;
}

/// `operator` is meaningless for a phrase in ES — its phrase parser never
/// consults it. Pre-fix `{"type":"phrase","operator":"and"}` fell into the
/// memtable's token-AND branch (tested before the phrase branch) and
/// silently stopped being a phrase: the reversed, non-adjacent query below
/// matched because both tokens were merely present in the same field.
#[tokio::test]
async fn operator_does_not_override_phrase_semantics() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mmpp_operator").await;

    assert_both_states(
        &idx,
        &[
            (
                json!({"multi_match": {"query": "policy merge", "fields": ["title", "body"],
                                       "type": "phrase", "operator": "and"}}),
                &[],
                "phrase + operator:and stays a phrase (reversed → no hit)",
            ),
            (
                json!({"multi_match": {"query": "merge policy", "fields": ["body"],
                                       "type": "phrase", "operator": "and"}}),
                &["1", "2"],
                "phrase + operator:and stays a phrase (in order → hit)",
            ),
        ],
    )
    .await;
}

/// `phrase_prefix`: head terms form an ordered phrase, the LAST term is a
/// prefix over the analyzed term dictionary. Pre-fix the memtable arm used
/// substring containment, so the punctuated doc was missed.
#[tokio::test]
async fn phrase_prefix_is_positional() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mmpp_prefix").await;

    assert_both_states(
        &idx,
        &[
            (
                json!({"multi_match": {"query": "merge poli", "fields": ["body"],
                                       "type": "phrase_prefix"}}),
                &["1", "2"],
                "multi_match phrase_prefix across punctuation",
            ),
            (
                json!({"match_phrase_prefix": {"body": "merge poli"}}),
                &["1", "2"],
                "oracle match_phrase_prefix across punctuation",
            ),
            // The head phrase still has to be an ordered adjacent phrase.
            (
                json!({"multi_match": {"query": "policy mer", "fields": ["body", "title"],
                                       "type": "phrase_prefix"}}),
                &[],
                "multi_match phrase_prefix reversed head",
            ),
        ],
    )
    .await;
}

/// `slop` was parsed away by `parse_multi_match`, so a sloppy phrase
/// silently behaved as an exact one. Doc 1's `body` analyses to
/// [the, log, merge, policy, …]: `"log policy"` needs slop >= 1.
#[tokio::test]
async fn phrase_slop_is_honoured() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mmpp_slop").await;

    assert_both_states(
        &idx,
        &[
            (
                json!({"multi_match": {"query": "log policy", "fields": ["body"],
                                       "type": "phrase"}}),
                &[],
                "slop 0 (default): one intervening token → no match",
            ),
            (
                json!({"multi_match": {"query": "log policy", "fields": ["body"],
                                       "type": "phrase", "slop": 1}}),
                &["1"],
                "slop 1: one intervening token → match",
            ),
            (
                json!({"multi_match": {"query": "log policy", "fields": ["body"],
                                       "type": "phrase", "slop": 5}}),
                &["1"],
                "slop 5: match",
            ),
        ],
    )
    .await;
}

/// A memtable phrase hit must also SCORE above zero — `score_query_against_doc`
/// carried its own copy of the substring predicate, so a doc admitted by the
/// (fixed) membership test would still score 0.0 and be dropped by scored paths.
#[tokio::test]
async fn memtable_phrase_hit_scores_nonzero() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let idx = seed(&engine, "mmpp_score").await;

    let r = idx
        .search(&req(json!({
            "multi_match": {"query": "merge policy", "fields": ["body", "title"],
                            "type": "phrase"}
        })))
        .await
        .unwrap();
    assert_eq!(r.total.value, 2, "memtable phrase hit count");
    for h in &r.hits {
        assert!(
            h.score > 0.0,
            "memtable multi_match phrase hit {} scored {} (expected > 0)",
            h.id,
            h.score
        );
    }
}

/// `slop` on a `phrase_prefix` cannot be honoured by either evaluator —
/// the segment clause (`xerj_fts::search::PhrasePrefixQuery`) carries no
/// slop, and the stored-doc walk requires an adjacent head phrase. ES does
/// honour it, so accepting the parameter and answering an exact phrase
/// would silently answer a different query (#204's defect class). It must
/// be refused, loudly.
#[test]
fn slop_on_phrase_prefix_is_refused_not_ignored() {
    let err = parse_request(&json!({
        "query": {"multi_match": {"query": "merge poli", "fields": ["body"],
                                  "type": "phrase_prefix", "slop": 2}}
    }))
    .expect_err("slop on phrase_prefix must be refused, not silently dropped");
    let msg = err.to_string();
    assert!(
        msg.contains("slop") && msg.contains("phrase_prefix"),
        "error must name both the parameter and the type, got: {msg}"
    );

    // slop 0 is the default and stays accepted.
    parse_request(&json!({
        "query": {"multi_match": {"query": "merge poli", "fields": ["body"],
                                  "type": "phrase_prefix", "slop": 0}}
    }))
    .expect("slop 0 on phrase_prefix is the default and must parse");
}

/// A negative `slop` is a client error in ES; accepting it silently would
/// be another accept-and-ignore. Assert the parser rejects it.
#[test]
fn negative_slop_is_rejected() {
    let err = parse_request(&json!({
        "query": {"multi_match": {"query": "merge policy", "fields": ["body"],
                                  "type": "phrase", "slop": -1}}
    }))
    .expect_err("negative slop must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("slop"),
        "error should name the offending parameter, got: {msg}"
    );
}
