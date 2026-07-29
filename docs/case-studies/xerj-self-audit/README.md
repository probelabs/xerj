# Case study: XERJ audits its own Rust with XERJ

**What this is.** A reproducible whitebox security audit of **XERJ's own engine**
(197 Rust files, ~201k lines across 16 crates) performed by an AI agent using
XERJ as its retrieval and reasoning substrate — the [WordPress
audit](../wordpress-security-audit/README.md) method turned on XERJ itself. XERJ
had no Rust parser, so the substrate is a tree-sitter-rust extractor
([`../../examples/rust-ast-audit/`](../../examples/rust-ast-audit/)) that turns
the source into three queryable indices: functions, call-edges, and routes.

Every number here is reproducible; see [REPRODUCE.md](REPRODUCE.md). The one
confirmed vulnerability is in [FINDINGS.md](FINDINGS.md), proven by crashing a
real server, not by argument.

The point is **not** "we 0-day'd our own engine." It is the **method and its
credibility**: what an AI-assisted Rust audit costs, what the call graph catches
that grep cannot, and — the part most demos skip — a **ground-truth recall test**
that measures whether the substrate actually finds real bugs before we trust it
on unknown ones.

## The credibility test: does this substrate actually find known bugs?

The day before this audit, PR #69 fixed **six** real, network-reachable
vulnerabilities in this exact codebase, at locations we know precisely. So we
indexed the **pre-#69 commit** and asked whether the substrate's queries surface
each one — and re-ran them on current `main` to confirm the signal flips when the
bug is fixed.

| Bug (PR #69) | Substrate signal | Pre-#69 | Current main |
|---|---|---|---|
| **F1** query_string stack overflow | recursion **cycle** with no depth guard | cycle **UNGUARDED** (rank #5 of 186) | same cycle **GUARDED** |
| **F7** field-limit bypass on ingest | `evolve_schema_*` reads a config limit | `reads_config_limit: false` | `reads_config_limit: true` |
| **F2/F9** path traversal | `.join(param)` with no containment check | present, no `containment` validator | `containment` validator present |
| **F6** MLT cross-product OOM | allocation arg is a **product** of params | present | capped |

The two we instrumented most sharply — F1 and F7 — **flip exactly on the fix**.
That is the difference between "this looks like a security tool" and "this
detects the class of bug it claims to." It also means the signal is honest: when
the code is fixed, the query goes quiet.

## The headline: XERJ-assisted vs. "the agent reads all 197 files"

| | Agent reads all files | XERJ-assisted audit |
|---|---|---|
| **Tokens to load the codebase once** | ~2,000,000 (approx: 8.0 MB source ÷ 4) | index once, then read only flagged functions |
| **Fits one context?** | No — ~10× a 200k window; must chunk | Yes — targeted queries return tens of candidates |
| **Interprocedural reach** | Lost at chunk boundaries | 87,137 call edges preserved across all files |
| **Finds a recursion *cycle*?** | Only if the whole cycle lands in one chunk | One query over the call graph |
| **Grounded?** | Skimming 2M tokens → guesses | Every finding cites a real `file:line`, verified by reading it |
| **Reproducible?** | No | Yes — extractor + queries reproduce exactly |

Substrate build: **~3.2s extract + ~1.9s ingest** for the whole workspace; 31 MB
on disk. The token figure is a labelled approximation (source bytes ÷ 4), not a
measured tokenizer count.

## Coverage (the explicit goal was 100%)

- **197/197 files parsed, 0 tree-sitter ERROR nodes.**
- **5,095 / 5,095 function nodes emitted — 100.0% function coverage.** (Verified
  by counting `function_item` AST nodes independently of emitted records; a first
  cut hit 97.2% because nested `fn` items were skipped — that gap is now closed
  and asserted in `coverage.json`.)
- 299 axum-handler-shaped functions, 477 route registrations, 21 functions
  containing `unsafe` (see below).

What the AST substrate **cannot** see is stated in [COVERAGE.md](COVERAGE.md):
macro-generated code, trait-object dynamic dispatch, and `build.rs` codegen are
out of reach, and callee resolution is name-based (method calls are deliberately
excluded from the call graph — see the note below).

## What the audit found

**One confirmed, empirically-proven Critical**, plus an unsafe inventory that came
back clean:

- **`POST /_sql` stack-overflow DoS** (Critical, unauthenticated). The SQL WHERE
  parser (`parse_or_expr → parse_and_expr → parse_condition → parse_or_expr`)
  recurses on every `(` with **no depth guard**. This is the *same class* as F1,
  which PR #69 fixed — in the query_string parser only. The SQL parser is the
  unfixed sibling. A single `POST /_sql` with ~50k nested parens aborts the
  process:

  ```
  thread 'xerj-rt' has overflowed its stack
  fatal runtime error: stack overflow, aborting
  ... Aborted (core dumped)
  ```

  Found by the call-graph **cycle** query (a grep cannot express "mutually
  recursive functions with no depth bound"), traced to the `/_sql` route via the
  routes index, and **proven by killing a live server** (exit 134 / SIGABRT).
  Full reproduction in [FINDINGS.md](FINDINGS.md).

### Complete `unsafe` inventory — 21 functions, all reviewed, all sound

Completeness was an explicit ask, so the full table is in
[FINDINGS.md](FINDINGS.md#unsafe-inventory). Breakdown: 12 FFI syscalls
(`statvfs`/`sysconf`/`clock_gettime`), 4 `MaybeUninit` for those FFI structs, 2
`Vec::from_raw_parts` in a CLI arg shim, 1 raw-pointer prefetch in HNSW, 1
`from_utf8_unchecked` in the SIMD tokenizer, and a raw-pointer JSON-tree walk.
Each carries a documented SAFETY invariant; on review each invariant holds. "0
unsound sites" is the honest result — and the inventory is the deliverable.

## Honest limits of this pass

This is a **single-agent** audit of the highest-value lenses (recursion/DoS,
unsafe, path-traversal), run while the multi-agent verification fleet was
unavailable. The broader lenses — full auth-flow mapping, business-logic
invariants, every `as`-cast triage — are scoped but not exhausted here; the
extractor emits the fields for them (`has_narrowing_cast`: 375 non-test sites,
`lock_across_await`: 15) and the queries are in the
[cookbook](../../examples/rust-ast-audit/QUERY_COOKBOOK.md), ready for the
adversarial-verification fleet to grind through. What is published here is only
what was **proven** or **completely enumerated** — not everything the substrate
flagged.

## Why this beats reading-all *and* beats grep

- **vs reading-all:** the codebase does not fit one context; chunking destroys
  the interprocedural reach that found the SQL cycle, and skimming 2M tokens
  invites hallucinated findings. Here every claim cites code that was read.
- **vs grep:** grep is line-scoped. It cannot express "a recursion *cycle* across
  three functions with no depth guard anywhere in it," or "a `.join()` of a
  request parameter with no containment check in the same function." The call
  graph makes both a single query.
