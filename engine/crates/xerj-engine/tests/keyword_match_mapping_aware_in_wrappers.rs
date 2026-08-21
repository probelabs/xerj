//! Issue #354 (follow-up): a keyword `match`/`multi_match` nested inside a
//! wrapper/compound query — `constant_score`, `function_score`, `boosting`,
//! `dis_max`, `nested`, `hybrid`, `pinned`, `named` — must be mapping-aware in
//! BOTH the memtable and a flushed segment, exactly like a top-level clause.
//!
//! PR #571 rewrote top-level keyword `Match`/`MultiMatch` to whole-value `Term`s
//! but recursed only into `Bool`, so a keyword match wrapped in e.g.
//! `constant_score` fell through unchanged and kept flipping its hit set at
//! `_flush` (the #354 defect, live for wrappers). This extends the rewrite's
//! recursion to the sub-query-carrying wrappers.
//!
//! One doc, `tags` mapped `keyword`, scalar value `"red"`. A wrapped
//! `match {tags: "red blue"}` must take the value WHOLE — `"red blue" != "red"`
//! — so the doc matches in NEITHER phase.
//!
//! Elasticsearch is referenced for wire semantics only; no ES code is here.

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

/// A keyword `match`/`multi_match` nested inside a wrapper query takes the
/// query text WHOLE — so `"red blue"` never matches `"red"` — identically
/// before and after `flush()`.
#[tokio::test]
async fn keyword_match_in_wrappers_takes_the_query_whole_in_both_phases() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);

    let mut schema = Schema::empty();
    schema
        .fields
        .push(FieldConfig::new("tags", FieldType::Keyword));
    engine.create_index("kw", schema).unwrap();
    let idx = engine.get_index("kw").unwrap();
    idx.index_document(Some("2".into()), json!({ "tags": "red" }))
        .await
        .unwrap();

    // Each wrapper embeds `match {tags: "red blue"}`; the keyword field takes
    // the query whole, so none match in either phase. (query, expected, label)
    let miss: &[(Value, &[&str], &str)] = &[
        (
            json!({ "constant_score": { "filter": { "match": { "tags": "red blue" } } } }),
            &[],
            "constant_score > match",
        ),
        (
            json!({ "function_score": { "query": { "match": { "tags": "red blue" } } } }),
            &[],
            "function_score > match",
        ),
        (
            json!({ "boosting": {
                "positive": { "match": { "tags": "red blue" } },
                "negative": { "match": { "tags": "zzz" } },
                "negative_boost": 0.5
            } }),
            &[],
            "boosting > positive match",
        ),
        (
            json!({ "dis_max": { "queries": [ { "match": { "tags": "red blue" } } ] } }),
            &[],
            "dis_max > match",
        ),
        (
            json!({ "dis_max": { "queries": [
                { "multi_match": { "query": "red blue", "fields": ["tags^2"] } }
            ] } }),
            &[],
            "dis_max > boosted multi_match",
        ),
    ];

    // Positive control: the whole value DOES match, wrapped, in both phases.
    let hit: &[(Value, &[&str], &str)] = &[(
        json!({ "constant_score": { "filter": { "match": { "tags": "red" } } } }),
        &["2"],
        "constant_score > match (whole value, control)",
    )];

    let all: Vec<(Value, &[&str], &str)> = miss.iter().chain(hit.iter()).cloned().collect();

    for (q, exp, label) in &all {
        assert_eq!(
            ids(&idx, q).await,
            expect(exp),
            "{label}: PRE-flush (memtable) hit set wrong for {q} — a keyword field \
             wrapped in a compound query must take the query text whole (#354)"
        );
    }
    idx.flush().await.unwrap();
    for (q, exp, label) in &all {
        assert_eq!(
            ids(&idx, q).await,
            expect(exp),
            "{label}: POST-flush (segment) hit set wrong for {q}"
        );
    }
}
