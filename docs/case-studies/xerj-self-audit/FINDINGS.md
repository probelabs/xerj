# Findings

## Confirmed — Critical

### `POST /_sql` unauthenticated stack-overflow → process abort

**Severity:** Critical (unauthenticated, single request, whole-process abort).
**Status:** confirmed by crashing a live server; **fixed** on branch
`feat/rust-self-audit` (commit `657db52`) and the fix proven against the same
payload — see "The fix" below.

**The code.** `engine/crates/xerj-engine/src/sql.rs`, the WHERE-clause
recursive-descent parser:

```rust
// sql.rs:414
fn parse_or_expr(tokens, pos)  -> ... { let left = parse_and_expr(...)?; ... }
// sql.rs:440
fn parse_and_expr(tokens, pos) -> ... { let left = parse_condition(...)?; ... }
// sql.rs:466
fn parse_condition(tokens, pos) -> ... {
    // NOT: self-recursion, no depth bound
    if w == "NOT" { let inner = parse_condition(...)?; ... }   // sql.rs:471
    // '(': back to the top of the cycle, no depth bound
    if let Some(Token::LParen) = tokens.get(*pos) {
        let inner = parse_or_expr(tokens, pos)?;               // sql.rs:483
        ...
    }
}
```

`parse_or_expr → parse_and_expr → parse_condition → parse_or_expr` is a recursion
**cycle**, and every `(` in the input drives one more turn around it. There is no
`DepthGuard` (the type exists in `xerj-query` and PR #69 wired it into the
*query_string* parser — but not here). Entry is `parse_sql` (`sql.rs:317`), which
is the handler for the route the routes index gives as:

```
POST /_sql  →  sql_query   (crates/xerj-api/src/router.rs:637)
```

**Reachability:** unauthenticated network. Same class as PR #69's **F1** — that
PR fixed the query_string parser and left the SQL parser, its sibling, unguarded.

**Proof (not argument).** A fresh server, one request:

```bash
python3 -c "print('{\"query\":\"SELECT * FROM t WHERE '+'('*50000+'a = 1'+')'*50000+'\"}')" > bomb.json
curl -s -m15 http://127.0.0.1:PORT/_sql -H 'Content-Type: application/json' --data-binary @bomb.json
```

A benign `SELECT * FROM t WHERE (a = 1)` returns HTTP 200. The payload above
kills the process in ~1s:

```
thread 'xerj-rt' (63745) has overflowed its stack
fatal runtime error: stack overflow, aborting
... Aborted (core dumped)          # exit 134 = 128 + SIGABRT
```

A stack-overflow abort is not a catchable panic — `catch_unwind` cannot save it,
and the whole server (every tenant, every index) goes down.

**The fix (shipped).** An explicit `depth` parameter is threaded through
`parse_or_expr` / `parse_and_expr` / `parse_condition`, rejected past
`MAX_SQL_DEPTH = 64` (mirroring `xerj-query`'s `MAX_QUERY_DEPTH`). Proof on the
rebuilt binary:

```
# baseline still works
POST /_sql {"query":"SELECT * FROM t WHERE (a = 1)"}      → HTTP 200
# the exact payload that aborted the unpatched server
POST /_sql {"query":"SELECT * FROM t WHERE ((…50k…))"}    → HTTP 400 in ~2ms
  {"reason":"SQL parse error: WHERE clause nesting exceeds max depth of 64"}
# server still answering afterwards
GET /                                                      → HTTP 200
```

Three regression tests (deep parens, deep `NOT`, moderate nesting still parses)
plus the full `xerj-engine` lib suite pass; `fmt` + `clippy -D warnings` clean.

---

## Unsafe inventory

Completeness was an explicit goal. **22 non-test `unsafe` blocks** across the
engine (plus 2 `unsafe fn` in a test-only allocator example). Every one was read;
every one carries a SAFETY invariant that holds. Zero unsound sites.

| file | line | fn | unsafe op |
|---|---|---|---|
| `xerj-ai/src/neural.rs` | 105 | `load` | mmap safetensors (lifetime documented) |
| `xerj-api/src/es_compat.rs` | 21687–8 | `read_disk_stats` | `statvfs` FFI + `MaybeUninit` |
| `xerj-api/src/es_compat.rs` | 25020–33 | `remove_dotted_path` | `&mut *` raw reborrow (single-threaded) |
| `xerj-autoindex/src/extract/pdf.rs` | 242 | `set_worker_memory_limit` | `setrlimit` FFI |
| `xerj-autoindex/src/extract/pdf.rs` | 264,273 | `terminate_worker_*` | `kill`/`killpg` FFI |
| `xerj-engine/src/governor.rs` | 586 | `page_size_bytes` | `sysconf` FFI |
| `xerj-engine/src/governor.rs` | 699–700 | `disk_stats` | `statvfs` FFI + `MaybeUninit` |
| `xerj-engine/src/lib.rs` | 119,150,190 | `*_pool` | thread-affinity FFI |
| `xerj-engine/src/turbo_ingest.rs` | 199 | `tokenize_fast` | `from_utf8_unchecked` (see note) |
| `xerj-fts/src/index.rs` | 1148 | `mmap_file` | `Mmap::map` |
| `xerj-server/src/ingest_memory_trace.rs` | 181 | `process_cpu_time_ns` | `clock_gettime` FFI |
| `xerj-server/src/main.rs` | 979,988 | `run_cli_index` | `Vec::from_raw_parts` (arg shim) |
| `xerj-storage/src/segment.rs` | 553 | `open` | `Mmap::map` |
| `xerj-vector/src/hnsw.rs` | 264 | `prefetch_vector` | `_mm_prefetch` raw pointer |

**Two worth a reviewer's eye (both sound):**

- `turbo_ingest.rs:199` `from_utf8_unchecked`. Sound **because** the only mutation
  upstream (`simd_lowercase`) touches bytes `0x41..=0x5A` only — ASCII uppercase
  can never appear inside a UTF-8 multi-byte sequence (those are all `≥0x80`), and
  `simd_find_word_boundaries` cuts only on ASCII separators, so every slice is
  valid UTF-8. The invariant is subtle and correctly documented.
- `es_compat.rs:25016` `remove_dotted_path`. Builds a `Vec<*mut Map>` and
  re-derives `&mut *ptr` during a bottom-up prune. Sound for the single-threaded
  request path: the raw pointers target distinct parent maps (containers, not
  entries), each reborrow is fresh with no overlapping live `&mut`, and removing a
  key from a `serde_json::Map` does not move the container. A safe rewrite with
  indices is possible but not required for correctness.

---

## Not published (held to the same bar as an external report)

The traversal lens surfaced two **defense-in-depth gaps** that are *not* proven
exploitable, so they are logged, not claimed as vulnerabilities:

- `engine.rs:797 put_index_mapping(name)` does `data_dir.join(name)` guarded only
  by `is_dir()`. Writing `es_mapping.json` into an already-existing directory is a
  weak primitive, and index names are validated upstream post-#69; a containment
  `debug_assert!` (as #69 added to `Index::open`) would close the gap defensively.
- `storage/src/backend.rs:80 abs()` does `root.join(path.trim_start_matches('/'))`
  with no `..` rejection. Reachable only with an attacker-controlled storage
  **key**, which is internally generated (`{segment_id}.{ext}`), so not currently
  reachable from the network. Worth a `..` guard for future-proofing.

Neither was reproduced, so neither is a finding. (This is deliberate: a public
case study should hold its own claims to the standard it would demand of an
outside report.)

## Refuted during the pass

The blunt category queries (e.g. "any function with a `with_capacity` and no
`bounds` validator" → 87 candidates) produced many candidates that did **not**
survive reading the code — almost all were fixed-size or internally-bounded
allocations. That is exactly why the pipeline sharpens the signal
(argument-provenance: `alloc_from_param`, `alloc_product`) and why every surviving
finding was read and, where possible, executed. A finder without a verifier would
have published the false positives.
