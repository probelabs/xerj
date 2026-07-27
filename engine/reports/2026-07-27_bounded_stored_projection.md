# Bounded stored projection probe — 2026-07-27

## Scope and source

This report measures the storage primitive only. The later engine composition
uses the primitive for identity-map construction and point reads, but the
numbers below remain storage-only measurements. They do not establish an
end-to-end engine speedup or an RSS reduction for autoindex.

The engine composition also refuses publish-time stored-slice warming before
decode when this framing-only bound is unavailable, exposes those decisions as
`predecode_bound_skips` in node stats, and continues warming unrelated doc-value
and sort-shadow families. Query-time `stored_slices_for` still performs its full
decode before retained-cache admission; bringing that miss path under the same
predecode discipline is the immediate dependent task, not a claim of this
report.

- Source commit: `031e9cc9286d8e8970704438acebef93f5ab72a8`
- Upstream base: `16d6df0`
- Probe source SHA-256:
  `86326cab5a4e2f7f2fb141d081855ac8ba790a3403f9103a973732a19c109953`
- Probe: `crates/xerj-storage/examples/stored_projection_probe.rs`
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`, LLVM 22.1.6
- Host: x86-64 Linux 6.12.95, 62.8 GiB RAM, no swap
- Build: release profile; no workspace-wide build and no `cargo clean`

Exact commands, from `engine/`:

```sh
cargo build --release -j 32 -p xerj-storage --example stored_projection_probe
probe=target/release/examples/stored_projection_probe
fixture=/workspace/.tmp/stored-decode-probe/finance-stream-final.zbs2
"$probe" generate "$fixture" 20000
sha256sum "$fixture"
for mode in full row projected; do
  for run in 1 2 3; do
    "$probe" "$mode" "$fixture" "/workspace/.tmp/stored-decode-probe/${mode}-stream-final-${run}.json"
  done
  sha256sum /workspace/.tmp/stored-decode-probe/${mode}-stream-final-*.json
done
for run in 1 2 3; do
  "$probe" zstd-compare 20
done
```

The deterministic fixture contains 20,000 finance-like rows and 86,834,777
bytes of input JSON. Its encoded size is **137,876 bytes** and its SHA-256 is
`71aef08c71077cec61a2482a3d8089571007f5167e7fa6a0b1d89b6b6f25e76d`.
This supersedes the earlier 139,062/139,064-byte scratch fixtures; neither is
evidence for this source commit.

## Decode results

All nine canonical output files have the same 4,225-byte content and SHA-256:
`d505b2317c26eec30cf7ff40604d37a5b54986b652bb7348730c5d7ce5d2b35d`.
This is the equality check between full decode, selective row hydration, and
selective row-plus-field projection.

| Mode | Wall µs, run 1 | Run 2 | Run 3 | Median | Peak RSS range |
|---|---:|---:|---:|---:|---:|
| Full | 352,653 | 327,372 | 336,715 | 336,715 | 334,647,296–337,702,912 |
| One row | 1,350,147 | 1,454,028 | 1,392,531 | 1,392,531 | 5,636,096–5,742,592 |
| One row, `body` only | 1,410,533 | 1,360,279 | 1,356,389 | 1,360,279 | 5,500,928–5,644,288 |

Raw probe output:

```text
mode=full rows=20000 decoded_bytes=86832275 output_bytes=4225 wall_us=352653 cpu_ticks=35 peak_rss_bytes=336138240
mode=full rows=20000 decoded_bytes=86832275 output_bytes=4225 wall_us=327372 cpu_ticks=32 peak_rss_bytes=334647296
mode=full rows=20000 decoded_bytes=86832275 output_bytes=4225 wall_us=336715 cpu_ticks=33 peak_rss_bytes=337702912
mode=row rows=1 output_bytes=4225 visited=80000 selected_values=8 decompressed_bytes=47264 wall_us=1350147 cpu_ticks=135 peak_rss_bytes=5636096
mode=row rows=1 output_bytes=4225 visited=80000 selected_values=8 decompressed_bytes=47264 wall_us=1454028 cpu_ticks=144 peak_rss_bytes=5689344
mode=row rows=1 output_bytes=4225 visited=80000 selected_values=8 decompressed_bytes=47264 wall_us=1392531 cpu_ticks=139 peak_rss_bytes=5742592
mode=projected rows=1 output_bytes=4225 visited=60000 selected_values=3 decompressed_bytes=0 wall_us=1410533 cpu_ticks=140 peak_rss_bytes=5517312
mode=projected rows=1 output_bytes=4225 visited=60000 selected_values=3 decompressed_bytes=0 wall_us=1360279 cpu_ticks=136 peak_rss_bytes=5500928
mode=projected rows=1 output_bytes=4225 visited=60000 selected_values=3 decompressed_bytes=0 wall_us=1356389 cpu_ticks=135 peak_rss_bytes=5644288
```

Raw logical-work counters:

- One row: 80,000 encoded rows visited, 8 selected JSON values, 47,264
  decompressed buffer bytes.
- One row plus field projection: 60,000 encoded rows visited, 3 selected JSON
  values, zero whole-column decompressed-buffer bytes.

Interpretation: selective hydration cuts peak RSS by roughly 59x, but is about
4x slower in this RAW-column fixture because streaming JSON still visits O(N)
rows. That is a useful bounded-memory primitive, not a latency win. LZ4 columns
still require whole-column decompression before row selection. A single giant
selected cell is not globally budgeted by this API.

`VmHWM` comes from `/proc/self/status`; CPU is reported by the probe as Linux
process ticks. Fresh processes are used for each measurement.

## Stream `encode_all` versus bulk zstd

Each cell below is one process timing 20 compression/decompression iterations.
The three raw wall-time runs are retained, rather than presenting only a
favorable aggregate.

| Shape | Input bytes | Stream bytes | Bulk bytes | Stream wall µs (3 runs) | Bulk wall µs (3 runs) |
|---|---:|---:|---:|---|---|
| Repeated text | 4,160,000 | 413 | 417 | 53,707 / 55,046 / 55,614 | 86,177 / 82,769 / 91,054 |
| JSON numbers | 945,611 | 8,507 | 8,510 | 6,671 / 6,920 / 6,876 | 5,216 / 6,251 / 5,290 |
| Packed IDs | 2,000,000 | 1,335,160 | 1,335,163 | 63,980 / 73,980 / 69,865 | 60,947 / 65,366 / 66,119 |

Bulk frames are 3–4 bytes larger. They are modestly faster for two shapes but
materially slower for repeated text. Therefore this branch does **not** change
the production encoder from `zstd::encode_all` to `zstd::bulk::compress`.
Current stream frames commonly omit a content-size field, so
`stored_slices_retained_upper_bound` deliberately returns `Ok(None)` and cache
admission must skip warming. Emitting content-size frames should be evaluated
with the future cache-resident integration, where its end-to-end benefit can be
measured.

## Verification

```text
cargo test -p xerj-storage --lib
123 passed; 0 failed; 0 ignored

cargo clippy -p xerj-storage --all-targets -- -D warnings
passed

cargo fmt --all --check
passed
```
