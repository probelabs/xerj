//! Fuzz the Painless script tokeniser, parser and evaluator.
//!
//! Scripts arrive inside ordinary search and update bodies, so this is
//! attacker-supplied *code* — the highest-value parser in the engine and the
//! historical source of the worst ES CVEs (CVE-2015-1427, Groovy sandbox
//! escape). The harness runs both the admission check (`check_script_limits`,
//! which must reject rather than hang) and a full evaluation against a
//! realistic document, so the depth, op-count and deadline budgets are
//! exercised rather than just the grammar.
#![no_main]

use libfuzzer_sys::fuzz_target;
use xerj_engine::painless::{check_script_limits, eval_painless, PainlessCtx};

fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    // The engine's own cap is larger; a smaller one here keeps every input the
    // fuzzer generates inside the interesting (accepted) region.
    if src.len() > 4096 {
        return;
    }

    let _ = check_script_limits(src);

    let doc = serde_json::json!({
        "title": "the quick brown fox",
        "price": 12.5,
        "count": 7,
        "tags": ["a", "b", "c"],
        "nested": { "inner": [1, 2, 3] },
    });
    let params = serde_json::json!({ "factor": 2, "name": "x" });
    let ctx = PainlessCtx::new(&doc, &params, 1.0);
    let _ = eval_painless(src, &ctx);
    let _ = ctx.take_emits();
});
