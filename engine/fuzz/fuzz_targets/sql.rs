//! Fuzz the SQL parser behind `/_sql`.
//!
//! A second hand-written tokeniser and recursive-descent parser, reachable
//! with a single POST. ES's own equivalent surface produced CVE-2024-43709
//! (crafted SQL query → OutOfMemoryError → node crash), so "does a crafted
//! statement panic or allocate without bound" is the exact question here.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(sql) = std::str::from_utf8(data) else {
        return;
    };
    if sql.len() > 8192 {
        return;
    }
    let _ = xerj_engine::sql::parse_sql(sql);
});
