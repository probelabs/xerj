//! Fuzz Elasticsearch index-name date math: `<logs-{now/d}>`.
//!
//! This one is reached from a request *URI*, not a body — every write and read
//! that names an index runs the name through it — and it is a second,
//! independent implementation of date math with its own hand-written brace
//! scanner on top. Both halves are attacker-controlled: the arithmetic
//! (`now+9999999999999d`) and the brace/format structure (`<a{b{c}}>`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(name) = std::str::from_utf8(data) else {
        return;
    };
    if name.len() > 1024 {
        return;
    }
    let _ = xerj_engine::index::resolve_date_math(name);
});
