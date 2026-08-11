//! Fuzz date-math expressions and date-format patterns.
//!
//! Both key spaces are attacker-influenced: `"gte": "now-1d/d"` and
//! `"format": "yyyy-MM-dd'T'HH:mm:ss.SSSZ||epoch_millis"` arrive in a range
//! query body, and the pattern compiler is a hand-written tokeniser over
//! quoted literals, repeated field letters and `||`-separated alternatives.
//!
//! Deliberately uses the *uncached* `compile_formats`: the cached wrapper is
//! keyed on the format string, and driving unique keys through it at fuzzing
//! rates measures the cache's eviction policy instead of the parser.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if text.len() > 1024 {
        return;
    }

    // Date math applied to both epoch 0 and a realistic base, rounding both
    // ways — the rounding paths do the calendar arithmetic that can overflow.
    for base in [0i64, 1_700_000_000_000] {
        for round_up in [false, true] {
            let _ = xerj_query::dates::apply_date_math(base, text, round_up);
        }
    }

    // Pattern compilation, then parsing real values with whatever compiled.
    if let Ok(formats) = xerj_query::dates::compile_formats(text) {
        for probe in [
            serde_json::Value::String("2026-01-31T23:59:59.999Z".to_string()),
            serde_json::Value::String(text.to_string()),
            serde_json::Value::from(1_700_000_000_000i64),
        ] {
            let _ = xerj_query::dates::date_value_matches_formats(&probe, &formats);
        }
        for round_up in [false, true] {
            let _ = xerj_query::dates::resolve_date_bound_str(text, round_up, Some(&formats));
        }
    }

    // Bound resolution with no declared format — the `now`-anchored and
    // `<anchor>||<math>` branches.
    for round_up in [false, true] {
        let _ = xerj_query::dates::resolve_date_bound_str(text, round_up, None);
    }
});
