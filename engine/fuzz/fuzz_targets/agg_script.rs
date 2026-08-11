//! Fuzz the two script tokenisers that live in the aggregation engine.
//!
//! `xerj_engine::painless::tokenize` is not the only Painless-subset scanner in
//! the tree. `aggs.rs` holds two more with the same byte-walk / `&str`-slice
//! shape, reached from an ordinary `_search` body and nothing else:
//!
//! * `aggs::lex_script`      — `scripted_metric` (`init/map/combine/reduce`)
//! * `aggs::tokenize_script` — `bucket_script` / `bucket_selector`
//!
//! Neither was covered when the `painless` target found its char-boundary
//! crash, and both had the identical defect: `&src[i..i + 2]` for a two-char
//! operator, taken at a byte index that can sit inside a multi-byte character
//! because every arm above it is ASCII-only. `{"map_script":"中"}` was
//! `end byte index 2 is not a char boundary` — the process, under
//! `panic = "abort"` (#207).
//!
//! The harness supplies only the *script source* and wraps it in fixed agg
//! bodies over fixed documents. That is deliberate: fuzzing whole aggregation
//! JSON would spend its budget on `date_histogram` intervals and bucket counts
//! and turn a security gate into an out-of-memory lottery, while the parsers
//! this file exists to protect would barely be reached.
#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::json;

fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    // Both evaluators reject longer sources on a length check before they
    // tokenize; past that bound the fuzzer is only exercising `len()`.
    if src.len() > 4096 {
        return;
    }

    let docs = vec![
        json!({ "k": "a", "v": 1.0 }),
        json!({ "k": "a", "v": 2.0 }),
        json!({ "k": "b", "v": 3.0 }),
    ];

    // scripted_metric -> lex_script
    let _ = xerj_engine::aggs::run_aggs(
        &json!({ "m": { "scripted_metric": {
            "init_script": src,
            "map_script": src,
            "combine_script": src,
            "reduce_script": src,
        }}}),
        &docs,
    );

    // bucket_script (sibling pipeline agg) -> tokenize_script
    let _ = xerj_engine::aggs::run_aggs(
        &json!({
            "s": { "sum": { "field": "v" } },
            "bs": { "bucket_script": { "buckets_path": { "x": "s" }, "script": src } },
        }),
        &docs,
    );

    // bucket_selector (parent pipeline agg) -> tokenize_script
    let _ = xerj_engine::aggs::run_aggs(
        &json!({ "by": {
            "terms": { "field": "k" },
            "aggs": {
                "s": { "sum": { "field": "v" } },
                "sel": { "bucket_selector": { "buckets_path": { "x": "s" }, "script": src } },
            },
        }}),
        &docs,
    );
});
