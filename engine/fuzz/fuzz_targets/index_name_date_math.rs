//! Fuzz Elasticsearch index-name date math: `<logs-{now/d}>`.
//!
//! This one is reached from a request *URI*, not a body — every write and read
//! that names an index runs the name through it — and it is a second,
//! independent implementation of date math with its own hand-written brace
//! scanner on top. Both halves are attacker-controlled: the arithmetic
//! (`now+9999999999999d`) and the brace/format structure (`<a{b{c}}>`).
//!
//! **The resolver that runs on the wire is `xerj_api::es_compat`'s.** The first
//! version of this harness fuzzed `xerj_engine::index::resolve_date_math`
//! instead, which is a re-implementation with no production callers — grep
//! `crates/` and the only hits are `lib.rs`'s re-export, its own `#[cfg(test)]`
//! module and this file. So the harness passed while the resolver that actually
//! runs on the index path parameter (`es_compat.rs` `create_index`) and on the
//! `_search` index list still aborted the process on
//! `<logs-{2026-01-01||+9999999999999d}>`. `resolve_date_math_index` is fuzzed
//! first here for that reason; the engine copy is kept because it is exported
//! from `xerj-engine`'s public API and costs nothing to drive with the same
//! input (#207).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(name) = std::str::from_utf8(data) else {
        return;
    };
    if name.len() > 1024 {
        return;
    }
    // The one that runs on the wire.
    let _ = xerj_api::es_compat::resolve_date_math_index(name);
    // The public engine re-implementation, same input.
    let _ = xerj_engine::index::resolve_date_math(name);
});
