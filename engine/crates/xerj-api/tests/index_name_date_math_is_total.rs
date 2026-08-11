//! `resolve_date_math_index` must be total on every index name a request can
//! carry.
//!
//! Issue #207. The `date_math` fuzz target found that `chrono`'s
//! `Duration::days`/`hours`/… **panic** on a count that does not fit a
//! `TimeDelta`, and that a byte-indexed `[..1]` splits a multi-byte unit
//! character in half. Three copies of that pattern existed. The first pass at
//! #207 fixed two of them — `xerj_engine::index::resolve_date_math` and the
//! `now` branch here — and missed the third: `apply_date_math_tail`, the
//! *anchored* branch (`<logs-{2026-01-01||+1d}>`) sixty lines above the hunk
//! that was patched.
//!
//! That matters because this resolver is the one that runs on the wire. It is
//! called on the index path parameter in `create_index` (`es_compat.rs:671`),
//! on alias resolution (`:825`) and on the `_search` index list (`:6763`), so
//! every input below arrives in a request URI from an unauthenticated client,
//! and the release profile sets `panic = "abort"` — each one was the process,
//! not a 400.
//!
//! This test is at the crate boundary on purpose: the defect survived a
//! same-file fix because nothing exercised the public entry point.

use std::panic;
use xerj_api::es_compat::resolve_date_math_index;

/// Resolve `name`, converting a panic into a reportable string instead of
/// letting it abort the test binary under `panic = "abort"`-adjacent settings.
fn resolve(name: &str) -> Result<String, String> {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let out = panic::catch_unwind(|| resolve_date_math_index(name));
    panic::set_hook(prev);
    out.map_err(|e| {
        e.downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "<non-string panic>".to_string())
    })
}

/// Before the fix every one of these aborts the process.
#[test]
fn an_anchored_index_name_never_panics() {
    let mut bad = Vec::new();
    for name in [
        // Offset that does not fit a TimeDelta — `Duration::days` panics.
        "<logs-{2026-01-01||+9999999999999d}>",
        "<logs-{2026-01-01||-9999999999999d}>",
        "<logs-{2026-01-01||+99999999999h}>",
        "<logs-{2026-01-01||+999999999999999m}>",
        "<logs-{2026-01-01||+99999999999999999s}>",
        "<logs-{2026-01-01||+9999999999999w}>",
        // `n * 30` / `n * 365` overflow i64 before `Duration` is even asked.
        "<logs-{2026-01-01||+9223372036854775807M}>",
        "<logs-{2026-01-01||+9223372036854775807y}>",
        // Representable offset, but the resulting DateTime is out of range.
        "<logs-{2026-01-01||+100000000000d}>",
        // Multi-byte character where the unit is read — `r2[..1]` splits it.
        "<logs-{2026-01-01||+1\u{4e2d}}>",
        "<logs-{2026-01-01||+1\u{1be}}>",
        "<logs-{2026-01-01||-1\u{1f600}}>",
        "<logs-{2026-01-01||/\u{4e2d}}>",
        "<logs-{2026-01-01||\u{4e2d}}>",
        // Same shapes with a timestamp anchor and a format suffix.
        "<logs-{2026-01-01T00:00:00||+9999999999999d{yyyy.MM.dd}}>",
        "<logs-{2026-01-01T00:00:00||/\u{4e2d}{yyyy}}>",
        // Chained tails, so the loop runs more than once.
        "<logs-{2026-01-01||+1d+9999999999999d/d}>",
        "<logs-{2026-01-01||/d+1\u{4e2d}-1d}>",
        // Controls: the `now` branch the first pass did fix.
        "<logs-{now+9999999999999d}>",
        "<logs-{now+1\u{4e2d}}>",
    ] {
        if let Err(msg) = resolve(name) {
            bad.push(format!("{name}  ->  PANIC: {msg}"));
        }
    }
    assert!(
        bad.is_empty(),
        "index names that panic resolve_date_math_index. `panic = \"abort\"` is \
         set in the release profile, so each of these is process death from a \
         request URI:\n  {}",
        bad.join("\n  ")
    );
}

/// The fix must not have made anchored date math stop working: an
/// unrepresentable offset degrades to "no offset", everything representable
/// still resolves.
#[test]
fn anchored_date_math_still_resolves() {
    assert_eq!(
        resolve("<logs-{2026-01-01||+1d}>").unwrap(),
        "logs-2026.01.02"
    );
    assert_eq!(
        resolve("<logs-{2026-01-01||-1d}>").unwrap(),
        "logs-2025.12.31"
    );
    assert_eq!(
        resolve("<logs-{2026-01-15||/M{yyyy.MM}}>").unwrap(),
        "logs-2026.01"
    );
    assert_eq!(
        resolve("<logs-{2026-01-01||+2M}>").unwrap(),
        // 2 * 30 days, which is what this resolver's calendar-free `M` means.
        "logs-2026.03.02"
    );
    // Out-of-range offset degrades to the anchor rather than to a crash.
    assert_eq!(
        resolve("<logs-{2026-01-01||+9999999999999d}>").unwrap(),
        "logs-2026.01.01"
    );
    // An unreadable multi-byte unit is not a recognised unit, so likewise.
    assert_eq!(
        resolve("<logs-{2026-01-01||+1\u{4e2d}}>").unwrap(),
        "logs-2026.01.01"
    );
    // Names without date math pass through untouched.
    assert_eq!(resolve("my-index").unwrap(), "my-index");
}
