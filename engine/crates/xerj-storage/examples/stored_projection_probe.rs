//! Reproducible probe for selective ZBS2 row hydration.
//!
//! This is intentionally a small executable rather than a Criterion benchmark:
//! each measured decode runs in a fresh process, and canonical output is written
//! to a file so `sha256sum` can verify full/selective equality.

use serde_json::{json, Value};
use std::fs;
use std::io::Cursor;
use std::time::Instant;
use xerj_storage::stored_codec::{
    decode_stored, decode_stored_v2_rows, decode_stored_v2_rows_projected, encode_stored_v2,
    StoredV2RowHydrationResult,
};

fn process_cpu_ticks() -> u64 {
    fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|stat| {
            // comm is parenthesized and may contain spaces, so split after its
            // final ')'. Fields 14 and 15 are then offsets 11 and 12.
            let fields = stat
                .rsplit_once(')')?
                .1
                .split_whitespace()
                .collect::<Vec<_>>();
            Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
        })
        .unwrap_or(0)
}

fn peak_rss_bytes() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmHWM:")
                    .and_then(|tail| tail.split_whitespace().next())
                    .and_then(|kb| kb.parse::<u64>().ok())
                    .map(|kb| kb * 1024)
            })
        })
        .unwrap_or(0)
}

fn target_json(id: Value, seq_no: Value, source: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"_id": id, "_seq_no": seq_no, "_source": source})).unwrap()
}

fn measured<T>(operation: impl FnOnce() -> T) -> (T, u128, u64) {
    let cpu_before = process_cpu_ticks();
    let started = Instant::now();
    let value = operation();
    (
        value,
        started.elapsed().as_micros(),
        process_cpu_ticks().saturating_sub(cpu_before),
    )
}

fn generate(path: &str, rows: usize) {
    let body = "finance evidence sentence ".repeat(160);
    let docs: Vec<Value> = (0..rows)
        .map(|row| {
            json!({
                "_id": format!("filing-{row}"),
                "_seq_no": row,
                "_source": {
                    "company": if row % 3 == 0 { "Acme" } else { "Globex" },
                    "year": 2015 + row % 10,
                    "quarter": row % 4 + 1,
                    "page": row % 140,
                    "body": format!("{body}{row}"),
                    "nested": {"amount": row as f64 / 7.0, "tags": ["finance", "report"]}
                }
            })
        })
        .collect();
    let raw = serde_json::to_vec(&docs).unwrap();
    let encoded = encode_stored_v2(&raw);
    fs::write(path, &encoded).unwrap();
    println!(
        "rows={rows} decoded_bytes={} encoded_bytes={}",
        raw.len(),
        encoded.len()
    );
}

fn full(path: &str, output: &str) {
    let encoded = fs::read(path).unwrap();
    let (decoded, wall_us, cpu_ticks) = measured(|| decode_stored(&encoded).unwrap());
    let docs: Vec<Value> = serde_json::from_slice(&decoded).unwrap();
    let row = &docs[docs.len() - 1];
    let target = target_json(
        row["_id"].clone(),
        row["_seq_no"].clone(),
        json!({"body": row["_source"]["body"]}),
    );
    fs::write(output, &target).unwrap();
    println!(
        "mode=full rows={} decoded_bytes={} output_bytes={} wall_us={wall_us} cpu_ticks={cpu_ticks} peak_rss_bytes={}",
        docs.len(),
        decoded.len(),
        target.len(),
        peak_rss_bytes()
    );
}

fn selective(path: &str, output: &str, projected: bool) {
    let encoded = fs::read(path).unwrap();
    let ordinal = 19_999;
    let (result, wall_us, cpu_ticks) = measured(|| {
        if projected {
            decode_stored_v2_rows_projected(&encoded, &[ordinal], &["body"]).unwrap()
        } else {
            decode_stored_v2_rows(&encoded, &[ordinal]).unwrap()
        }
    });
    let StoredV2RowHydrationResult::Hydrated { rows, stats } = result else {
        panic!("fixture was not selectively hydrated")
    };
    let row = rows.into_iter().next().unwrap();
    let source = if projected {
        Value::Object(row.source)
    } else {
        json!({"body": row.source["body"]})
    };
    let target = target_json(row.id, row.seq_no, source);
    fs::write(output, &target).unwrap();
    println!(
        "mode={} rows=1 output_bytes={} visited={} selected_values={} decompressed_bytes={} wall_us={wall_us} cpu_ticks={cpu_ticks} peak_rss_bytes={}",
        if projected { "projected" } else { "row" },
        target.len(),
        stats.encoded_rows_visited,
        stats.selected_json_values,
        stats.decompressed_buffer_bytes,
        peak_rss_bytes()
    );
}

fn zstd_compare(iterations: usize) {
    let shapes = [
        (
            "repeated_text",
            b"finance evidence sentence ".repeat(160_000),
        ),
        (
            "json_numbers",
            serde_json::to_vec(&(0..200_000).map(|n| n % 4096).collect::<Vec<_>>()).unwrap(),
        ),
        (
            "packed_ids",
            (0_u32..500_000).flat_map(u32::to_le_bytes).collect(),
        ),
    ];
    for (name, bytes) in shapes {
        for implementation in ["stream", "bulk"] {
            let started = Instant::now();
            let mut size = 0;
            for _ in 0..iterations {
                let encoded = if implementation == "stream" {
                    zstd::encode_all(Cursor::new(&bytes), 3).unwrap()
                } else {
                    zstd::bulk::compress(&bytes, 3).unwrap()
                };
                size = encoded.len();
                assert_eq!(zstd::decode_all(encoded.as_slice()).unwrap(), bytes);
            }
            println!(
                "shape={name} input_bytes={} implementation={implementation} encoded_bytes={size} iterations={iterations} wall_us={}",
                bytes.len(),
                started.elapsed().as_micros()
            );
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("generate") => generate(&args[2], args[3].parse().unwrap()),
        Some("full") => full(&args[2], &args[3]),
        Some("row") => selective(&args[2], &args[3], false),
        Some("projected") => selective(&args[2], &args[3], true),
        Some("zstd-compare") => zstd_compare(args[2].parse().unwrap()),
        _ => panic!(
            "generate <fixture> <rows> | full|row|projected <fixture> <output> | zstd-compare <iterations>"
        ),
    }
}
