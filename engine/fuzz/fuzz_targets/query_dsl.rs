//! Fuzz the Elasticsearch-compatible search DSL parser.
//!
//! Reachable by any client that can POST to `/{index}/_search`, so the whole
//! body is untrusted: nesting depth, clause count, field names, numeric
//! ranges, `_source` filters, sort specs and aggregation shapes all come
//! straight off the wire.
//!
//! The harness runs parse *and* rewrite, because the rewriter walks the tree
//! the parser produced — a parser that accepts a pathological shape and a
//! rewriter that then recurses on it are one bug, not two.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // serde_json is a third-party parser with its own fuzzing; this harness
    // is about what xerj does with the value once it is JSON.
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };

    if let Ok(request) = xerj_query::parse_request(&value) {
        // The parser's depth and clause budgets are the only thing standing
        // between an untrusted body and unbounded recursion here.
        let _ = xerj_query::rewrite(request.query);
    }

    // `parse_query` is also called directly (binary protocol, percolate-style
    // paths), so exercise it on the same input rather than only through a
    // well-formed request envelope.
    let _ = xerj_query::parse_query(&value);
});
