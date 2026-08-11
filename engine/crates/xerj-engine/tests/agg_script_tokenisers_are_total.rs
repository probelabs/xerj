//! The aggregation script tokenisers must be total on any `_search` body.
//!
//! Issue #207. `painless::tokenize` was not the only Painless-subset scanner
//! walking bytes and slicing `&str`. `aggs.rs` holds two more, reached from an
//! ordinary aggregation body and from nowhere else:
//!
//! * `aggs::lex_script`      — `scripted_metric`
//! * `aggs::tokenize_script` — `bucket_script` / `bucket_selector`
//!
//! Both took `&src[i..i + 2]` to test for a two-character operator. Every arm
//! above that point is ASCII-only, so the lead byte of a multi-byte character
//! fell straight through and `i + 2` landed inside it:
//! `end byte index 2 is not a char boundary`. Both also skipped whitespace with
//! `is_whitespace()` on `bytes[i] as char`, which is true for Latin-1 NEL and
//! NBSP — bytes that in valid UTF-8 are only ever *continuation* bytes, so the
//! scanner could step into the middle of a character on its own.
//!
//! The release profile sets `panic = "abort"`, so this was not a 400 on a bad
//! script: it was the process, from an unauthenticated `_search`.

use serde_json::json;
use std::panic;

fn run(label: String, aggs: serde_json::Value, docs: Vec<serde_json::Value>) -> Option<String> {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let out = panic::catch_unwind(move || xerj_engine::aggs::run_aggs(&aggs, &docs));
    panic::set_hook(prev);
    out.err().map(|e| {
        format!(
            "{label}: {}",
            e.downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "<non-string panic>".into())
        )
    })
}

fn docs() -> Vec<serde_json::Value> {
    vec![
        json!({ "k": "a", "v": 1.0 }),
        json!({ "k": "a", "v": 2.0 }),
        json!({ "k": "b", "v": 3.0 }),
    ]
}

/// Before the fix each of these aborts the test process.
#[test]
fn a_non_ascii_aggregation_script_is_an_error_not_a_panic() {
    let mut bad = Vec::new();

    for src in [
        // Bare multi-byte character: the lead byte reaches the two-char slice.
        "\u{4e2d}",
        // Multi-byte between two operands, and after an operator.
        "1 \u{4e2d} 2",
        "params.x \u{4e2d}",
        "params.x > \u{4e2d}",
        // Four-byte character, so `i + 2` is two bytes short of the end.
        "params.x \u{1f600}",
        // NBSP: `is_whitespace()` on the 0xA0 continuation byte used to
        // advance `i` into the middle of the character.
        "params.x \u{a0}> 1",
        "\u{a0}\u{a0}\u{a0}",
        // NEL (U+0085), the other byte `is_whitespace()` accepted.
        "params.x \u{85}> 1",
        // Multi-byte at the very end, where the two-char probe runs off.
        "1 +\u{e9}",
        "\u{e9}",
    ] {
        for (label, aggs) in [
            (
                "scripted_metric",
                json!({ "m": { "scripted_metric": { "map_script": src } } }),
            ),
            (
                "bucket_script",
                json!({
                    "s": { "sum": { "field": "v" } },
                    "bs": { "bucket_script": { "buckets_path": { "x": "s" }, "script": src } },
                }),
            ),
            (
                "bucket_selector",
                json!({ "by": {
                    "terms": { "field": "k" },
                    "aggs": {
                        "s": { "sum": { "field": "v" } },
                        "sel": { "bucket_selector": {
                            "buckets_path": { "x": "s" }, "script": src } },
                    },
                }}),
            ),
        ] {
            if let Some(msg) = run(format!("{label} script={src:?}"), aggs, docs()) {
                bad.push(msg);
            }
        }
    }

    assert!(
        bad.is_empty(),
        "aggregation scripts that panic run_aggs. `panic = \"abort\"` is set in \
         the release profile, so each of these is process death from a _search \
         body:\n  {}",
        bad.join("\n  ")
    );
}

/// The narrowed scan must not have narrowed the language: ASCII scripts,
/// including the two-character operators the guard now gates, still evaluate.
#[test]
fn ascii_aggregation_scripts_still_evaluate() {
    // bucket_script over a sibling sum, using a two-character operator path
    // (`>=` is tokenised by the same guarded branch).
    let out = xerj_engine::aggs::run_aggs(
        &json!({
            "s": { "sum": { "field": "v" } },
            "bs": { "bucket_script": {
                "buckets_path": { "x": "s" },
                "script": "params.x >= 6 ? params.x * 2 : params.x",
            }},
        }),
        &docs(),
    );
    assert_eq!(
        out["bs"]["value"].as_f64(),
        Some(12.0),
        "bucket_script over sum(v)=6 with `>=` and a ternary: {out}"
    );

    // bucket_selector keeps the bucket whose sum passes the predicate and
    // drops the other one.
    let out = xerj_engine::aggs::run_aggs(
        &json!({ "by": {
            "terms": { "field": "k" },
            "aggs": {
                "s": { "sum": { "field": "v" } },
                "sel": { "bucket_selector": {
                    "buckets_path": { "x": "s" },
                    "script": "params.x >= 3 && params.x != 99",
                }},
            },
        }}),
        &docs(),
    );
    let buckets = out["by"]["buckets"].as_array().expect("buckets");
    assert_eq!(buckets.len(), 2, "both buckets sum to >= 3: {out}");

    let out = xerj_engine::aggs::run_aggs(
        &json!({ "by": {
            "terms": { "field": "k" },
            "aggs": {
                "s": { "sum": { "field": "v" } },
                "sel": { "bucket_selector": {
                    "buckets_path": { "x": "s" },
                    "script": "params.x > 3",
                }},
            },
        }}),
        &docs(),
    );
    let buckets = out["by"]["buckets"].as_array().expect("buckets");
    assert_eq!(buckets.len(), 0, "neither bucket sums above 3: {out}");
}
