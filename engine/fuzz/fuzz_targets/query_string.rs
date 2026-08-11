//! Fuzz the Lucene `query_string` / `simple_query_string` mini-language.
//!
//! This is the one hand-written recursive-descent grammar on the read path:
//! it tokenises quotes, escapes, field prefixes, boolean operators, grouping
//! parens, wildcards and `[a TO b]` / `field:>10` range syntax. It is reachable
//! unauthenticated wherever search is, both from a request body and from the
//! `?q=` URI parameter, which is exactly the surface ES burned itself on in
//! CVE-2023-31419 (stack overflow via crafted query strings).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(query) = std::str::from_utf8(data) else {
        return;
    };
    // The parser enforces its own length cap; staying under it keeps the
    // fuzzer's budget on grammar states rather than on the length check.
    if query.len() > 4096 {
        return;
    }

    for default_operator in ["OR", "AND"] {
        let body = serde_json::json!({
            "query_string": {
                "query": query,
                "default_field": "body",
                "default_operator": default_operator,
            }
        });
        let _ = xerj_query::parse_query(&body);
    }

    // No `default_field`: takes the `*`/multi-field lowering branch, which has
    // different range-syntax handling from the single-field one.
    let body = serde_json::json!({ "query_string": { "query": query } });
    let _ = xerj_query::parse_query(&body);

    let body = serde_json::json!({ "simple_query_string": { "query": query } });
    let _ = xerj_query::parse_query(&body);
});
