//! Regression tests for keyword `term` clauses inside scored `bool` queries.
//!
//! A keyword term used to make the whole bool decline the FTS projection and
//! fall back to the schema-blind stored-source matcher.  That fallback split
//! identifiers such as `is_wp_error` into three OR tokens and used its flat
//! fallback score, so a zero-matching keyword term changed both membership and
//! scores of an unrelated text clause.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use tempfile::TempDir;
use xerj_common::config::Config;
use xerj_common::types::{FieldConfig, FieldType, Schema};
use xerj_engine::{Engine, Index};
use xerj_query::executor::SearchResult;
use xerj_query::parse_request;

fn make_engine(dir: &TempDir) -> Engine {
    let mut config = Config::default();
    config.server.data_dir = dir.path().to_str().unwrap().to_string();
    Engine::new(config).expect("engine::new")
}

fn request(query: Value) -> xerj_query::ast::SearchRequest {
    parse_request(&json!({"query": query, "size": 20})).expect("parse_request")
}

async fn seed() -> (TempDir, std::sync::Arc<Index>) {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let mut schema = Schema::empty();
    schema
        .fields
        .push(FieldConfig::new("body", FieldType::Text));
    schema
        .fields
        .push(FieldConfig::new("defs", FieldType::Text));
    schema
        .fields
        .push(FieldConfig::new("ax_path", FieldType::Keyword));
    engine.create_index("bool-term", schema).unwrap();
    let idx = engine.get_index("bool-term").unwrap();

    idx.index_document(
        Some("hit".into()),
        json!({
            "body": "is_wp_error",
            "defs": "is_wp_error",
            "ax_path": "wp/hit.php"
        }),
    )
    .await
    .unwrap();
    idx.index_document(
        Some("is-only".into()),
        json!({"body": "unrelated", "defs": "is", "ax_path": "wp/is.php"}),
    )
    .await
    .unwrap();
    idx.index_document(
        Some("wp-only".into()),
        json!({"body": "unrelated", "defs": "wp", "ax_path": "wp/wp.php"}),
    )
    .await
    .unwrap();
    idx.index_document(
        Some("error-only".into()),
        json!({"body": "unrelated", "defs": "error", "ax_path": "wp/error.php"}),
    )
    .await
    .unwrap();
    idx.index_document(
        Some("neither".into()),
        json!({
            "body": "unrelated",
            "defs": "unrelated",
            "ax_path": "wp/neither.php"
        }),
    )
    .await
    .unwrap();

    // The standalone text queries use the segment FTS path.  The bools under
    // test take a different path on the base commit, so flushing is required
    // to expose the divergence deterministically.
    idx.flush().await.unwrap();
    (dir, idx)
}

fn ids(result: &SearchResult) -> BTreeSet<String> {
    result.hits.iter().map(|hit| hit.id.clone()).collect()
}

fn scores(result: &SearchResult) -> BTreeMap<String, f32> {
    result
        .hits
        .iter()
        .map(|hit| (hit.id.clone(), hit.score))
        .collect()
}

fn assert_same_hits(label: &str, expected: &SearchResult, actual: &SearchResult) {
    assert_eq!(
        actual.total.value, expected.total.value,
        "{label}: total changed"
    );
    assert_eq!(ids(actual), ids(expected), "{label}: matched IDs changed");

    let expected_scores = scores(expected);
    let actual_scores = scores(actual);
    for (id, expected_score) in expected_scores {
        let actual_score = actual_scores
            .get(&id)
            .unwrap_or_else(|| panic!("{label}: missing score for {id}"));
        assert!(
            (actual_score - expected_score).abs() < 1e-5,
            "{label}: score for {id} changed from {expected_score} to {actual_score}"
        );
    }
}

#[tokio::test]
async fn zero_keyword_term_does_not_change_multimatch_should() {
    let (_dir, idx) = seed().await;
    let baseline = idx
        .search(&request(json!({
            "multi_match": {
                "query": "is_wp_error",
                "fields": ["body", "defs"],
                "type": "most_fields"
            }
        })))
        .await
        .unwrap();
    let with_zero_term = idx
        .search(&request(json!({
            "bool": {
                "should": [
                    {"multi_match": {
                        "query": "is_wp_error",
                        "fields": ["body", "defs"],
                        "type": "most_fields"
                    }},
                    {"term": {"ax_path": "no/such/file.php"}}
                ]
            }
        })))
        .await
        .unwrap();

    assert_eq!(baseline.total.value, 1, "fixture baseline must be one hit");
    assert!(
        baseline.hits[0].score > 1.0,
        "fixture must expose the lost scored path, got {}",
        baseline.hits[0].score
    );
    assert_same_hits("multi_match + zero term", &baseline, &with_zero_term);
}

#[tokio::test]
async fn zero_keyword_term_does_not_change_match_should() {
    let (_dir, idx) = seed().await;
    let baseline = idx
        .search(&request(json!({"match": {"defs": "is_wp_error"}})))
        .await
        .unwrap();
    let with_zero_term = idx
        .search(&request(json!({
            "bool": {
                "should": [
                    {"match": {"defs": "is_wp_error"}},
                    {"term": {"ax_path": "no/such/file.php"}}
                ]
            }
        })))
        .await
        .unwrap();

    assert_eq!(baseline.total.value, 1, "fixture baseline must be one hit");
    assert_same_hits("match + zero term", &baseline, &with_zero_term);
}

#[tokio::test]
async fn zero_keyword_term_does_not_expand_match_must() {
    let (_dir, idx) = seed().await;
    let baseline = idx
        .search(&request(json!({"match": {"defs": "is_wp_error"}})))
        .await
        .unwrap();
    let with_zero_term = idx
        .search(&request(json!({
            "bool": {
                "must": [{"match": {"defs": "is_wp_error"}}],
                "should": [{"term": {"ax_path": "no/such/file.php"}}]
            }
        })))
        .await
        .unwrap();

    assert_same_hits("must match + zero term", &baseline, &with_zero_term);
}

#[tokio::test]
async fn minimum_should_match_rejects_a_zero_matching_keyword_term() {
    let (_dir, idx) = seed().await;
    let result = idx
        .search(&request(json!({
            "bool": {
                "must": [{"match": {"defs": "is_wp_error"}}],
                "should": [{"term": {"ax_path": "no/such/file.php"}}],
                "minimum_should_match": 1
            }
        })))
        .await
        .unwrap();

    assert_eq!(result.total.value, 0);
    assert!(result.hits.is_empty());
}

#[tokio::test]
async fn matching_keyword_term_adds_score_and_union_membership() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);
    let mut schema = Schema::empty();
    schema
        .fields
        .push(FieldConfig::new("defs", FieldType::Text));
    schema
        .fields
        .push(FieldConfig::new("ax_path", FieldType::Keyword));
    engine.create_index("bool-term-additive", schema).unwrap();
    let idx = engine.get_index("bool-term-additive").unwrap();
    for (id, defs, path) in [
        ("both", "is_wp_error", "same/path.php"),
        ("text", "is_wp_error", "other/path.php"),
        ("term", "unrelated", "same/path.php"),
        ("neither", "unrelated", "other/path.php"),
    ] {
        idx.index_document(Some(id.into()), json!({"defs": defs, "ax_path": path}))
            .await
            .unwrap();
    }
    idx.flush().await.unwrap();

    let text = idx
        .search(&request(json!({"match": {"defs": "is_wp_error"}})))
        .await
        .unwrap();
    let keyword = idx
        .search(&request(json!({"term": {"ax_path": "same/path.php"}})))
        .await
        .unwrap();
    let combined = idx
        .search(&request(json!({
            "bool": {
                "should": [
                    {"match": {"defs": "is_wp_error"}},
                    {"term": {"ax_path": "same/path.php"}}
                ]
            }
        })))
        .await
        .unwrap();

    assert_eq!(
        ids(&combined),
        BTreeSet::from(["both".into(), "text".into(), "term".into()])
    );
    let text_score = scores(&text)["both"];
    let keyword_score = scores(&keyword)["both"];
    let combined_score = scores(&combined)["both"];
    assert!(
        (combined_score - (text_score + keyword_score)).abs() < 1e-5,
        "combined score {combined_score} did not add text {text_score} + keyword {keyword_score}"
    );
}
