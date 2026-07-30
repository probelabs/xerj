# The instruction/data whitebox methodology

A reusable five-stage playbook for auditing a large codebase, and the measured
cost of running it **with** an AST/call-graph substrate versus **with grep on the
raw tree**. Run here against XERJ's own Rust engine (197 files, ~201k lines, 16
crates); the method is language-agnostic.

## The principle

> **A function is dangerous when INSTRUCTION and DATA share one channel with no
> separator.**

That is the whole test. It is sharper than a category checklist because it tells
you *why* something is a sink and *what* would actually fix it:

| Channel | Instruction component | Data component | Separated (safe) form |
|---|---|---|---|
| SQL string | `SELECT`, `WHERE`, `,`, `'` | identifiers, literals | bound parameter `?` — a structural placeholder the parser resolves *after* parsing. Identifiers cannot be bound, so the only safe forms are dialect quoting with escape-doubling, or a schema allowlist. |
| Filesystem path | `/`, `..`, an absolute prefix | a name component | a validated opaque token, plus canonicalize-then-contain **before** any destructive op |
| Process argv | a leading `-` (option) | an argument value | `--` end-of-options, or `arg("--opt=VALUE")`, or a child that does no getopt parsing at all |
| Serialized bytes | type / construction tags | field values | a fixed target type with no polymorphic tag, plus a size/depth budget |
| Query DSL / template | JSON structure, query keys | user params | parse-then-substitute-whole-nodes; never substitute-then-parse |

And the corollary that catches most real bugs:

> **A filter is only real if it removes the INSTRUCTION power from the DATA
> channel.** "It calls `escape()`" is not enough. Shell-escaping quotes
> metacharacters and does **nothing** against option injection. A containment
> check is not real if its reference point is attacker-supplied.

The two worked examples below (INJ-01, F-PATH-02) are corollary failures, not
missing-filter failures. Every one of the three filters guarding the snapshot path
is real, well-written, and tested — and the vulnerability is in the un-validated
sibling
field none of them look at.

## Two structural reasons grep loses, before any token count

1. **The unit of a hit.** An AST hit is a **function**. A grep hit is a **line**,
   which forces you to read the whole **file** to judge it. In this codebase the
   ratio is brutal: `render_template` is 20 lines inside
   `crates/xerj-api/src/es_compat.rs`, a **31,221-line / 273,864-token** file.
   The substrate hands you the 20 lines and the 8 call edges into them.
2. **Absence is not greppable.** Stage 4's whole job is finding a check that is
   *not there*. `grep -rnE 'path\.repo|repo_allowlist|snapshot_root' crates`
   returns **0 lines** on this tree — and that zero is exactly the F-PATH-02
   vulnerability (no snapshot-location allowlist exists). A `must_not` clause
   over a validator field expresses it in one query; a regex cannot express it at
   all. This is the same class as PR #69's F7/F8, which grep also could not reach.

## The five stages

Substrate: three indices over the current tree — `rustfns` (5,098 functions),
`rustcalls` (87,150 caller→callee edges), `rustroutes` (477 axum registrations).
Non-test function population: **3,437**; handler-shaped: **298**.

### Stage 1 — Flag every dangerous END function (sink), by the instruction/data test

Enumerate sinks per channel class, then judge each by whether the data component
retains instruction power.

```bash
curl -s "http://127.0.0.1:9310/rustfns/_search" -H 'Content-Type: application/json' -d '{
 "query":{"bool":{"filter":[{"term":{"is_test":false}}],
  "should":[{"term":{"sinks":"fs_path_join"}},{"term":{"sinks":"fs_write"}},
            {"term":{"sinks":"fs_delete"}},{"term":{"sinks":"fs_rename"}},
            {"term":{"sinks":"process"}},{"term":{"sinks":"deserialize"}},
            {"term":{"sinks":"sql_ish"}},{"term":{"sinks":"net_egress"}}],
  "minimum_should_match":1}},
 "_source":["crate","file","fn_name","line_start","sinks","validators","path_join_from_param","signature"],
 "size":1000}'
```

→ **318 functions**, 40,623 tokens of projected metadata.

Sink census (non-test functions), with the verdict the instruction/data test produced:

| sink class | functions | verdict |
|---|---|---|
| `log_secret` | 210 | heuristic, mostly benign logging |
| `fs_path_join` | 149 | **the productive class** — 22 join from a parameter |
| `deserialize` | 147 | fixed target types, no polymorphic tag → informational |
| `fs_write` | 49 | filenames internally generated except the snapshot path |
| `spawn` | 31 | internal task spawn, no argv |
| `fs_rename` | 16 | internal segment/WAL tokens only |
| `fs_delete` | 14 | all under a validated or reserved-prefix directory |
| `net_egress` | 13 | operator-config endpoints |
| `sql_ish` | 4 | **1 real, 3 false positives** (see the precision note) |
| `process` | 2 | 1 real, and it is safe by child design (see below) |

**Precision note, stated because a case study that hides it is worthless:**
`sinks:sql_ish` is **noisy — 4 hits, 1 real**. The three
`crates/xerj-fts/src/search.rs` hits (`search_bounded:503`, `execute_bool:1019`,
`execute_dis_max:1124`) are false positives triggered by the English word
"union" in the comment `// If no must clauses, candidate set is the union of
should docs`. Do not treat `sql_ish` as a trustworthy filter. The substrate is a
strong **filter** and a mediocre **ranker** — every candidate was read.

**Grep baseline** (the union of sink regexes an auditor would actually write):

```bash
grep -rn --include='*.rs' -E 'Command::new|process::Command|fs::(read|write|remove_file|remove_dir_all|rename|create_dir_all|File::(open|create))|\.join\(|from_str|from_slice|from_reader|reqwest|TcpStream|format!\(.*(SELECT|WHERE|FROM)' crates
```

→ **1,132 match lines across 110 distinct files = 1,585,177 tokens** to read.
That is 88% of the entire codebase, returned by one stage-1 sweep.

### Stage 2 — Trace each sink back to user input (the call graph)

```bash
curl -s "http://127.0.0.1:9310/rustcalls/_search" -H 'Content-Type: application/json' \
 -d '{"query":{"term":{"callee":"render_template"}},
      "_source":["caller","file","line","kind","resolvable"],"size":100}'
```

Walk callers until you hit a `rustroutes` handler. For the two injection/path
examples that walk was: `render_template` (8 caller edges) → `search_template` /
`msearch_template_impl` / `render_template_api`; `create_snapshot` (3) and
`validate_snapshot_path` (2) → `es_compat::create_snapshot` → route.

→ **13 caller edges across 4 queries, 1,232 tokens.** The result is a *directed*
answer: these callers, at these lines.

**Grep baseline:** `grep -rn 'render_template|create_snapshot|validate_snapshot_path'`
→ 23 lines / 5 files / **374,872 tokens** — and it is undirected (callers and
the definition and the tests, indistinguishable). More importantly it only works
**if you already know the function name**, which is the thing stage 1 had to tell
you. Discovering `render_template` by grep instead means the generic
"data merged into a parsed string" sweep: `grep -rln 'serde_json::from_str'` → 20
files / **772,182 tokens**.

### Stage 3 — Map the privilege model and where each check applies

```bash
# every route registration
curl -s ".../rustroutes/_search" -d '{"query":{"match_all":{}},"_source":["method","path","handler","file","line"],"size":500}'
# every function that performs an auth check
curl -s ".../rustfns/_search" -d '{"query":{"bool":{"filter":[{"term":{"is_test":false}},{"term":{"validators":"auth"}}]}}}'
```

→ **477 routes + 24 auth-validating functions, 40,218 tokens.** This is what
lets you say "authenticated-network" rather than guessing: the route table maps
path → handler → file:line, and the auth set shows the check lives in
middleware, not per-handler.

It also surfaces the **most important qualifier in this whole report**, at
`crates/xerj-api/src/auth.rs:96`:

```rust
// Skip auth when disabled or no admin key is configured.
if !cfg.enabled || cfg.admin_api_key.is_empty() {
    return true;
}
```

`auth.enabled = true` is the shipped default (`xerj.default.toml:47`), but an
**empty** `admin_api_key` short-circuits to fully open — and the documented
`--insecure` quickstart flag disables auth outright. Every
"authenticated-network" severity below becomes unauthenticated in those two
configurations.

**Grep baseline:** `grep -rnE 'auth|api_key|require_admin|is_admin|role|permission|middleware'`
→ 874 lines / 73 files / **1,271,510 tokens**, and it produces no route table at
all — the `.route()` calls are what you actually need, and they are one grep
further on.

### Stage 4 — Validate every filter for BYPASSES

The stage where absence-queries earn their keep: **sink present, filter absent.**

```bash
curl -s "http://127.0.0.1:9310/rustfns/_search" -H 'Content-Type: application/json' -d '{
 "query":{"bool":{
  "filter":[{"term":{"is_test":false}},{"term":{"path_join_from_param":true}}],
  "must_not":[{"term":{"validators":"containment"}},
              {"term":{"validators":"explicit_validate"}},
              {"term":{"validators":"typed_name"}}]}},
 "_source":["crate","file","fn_name","line_start","sinks","validators","path_join_args"],"size":500}'
```

→ **18 functions** (out of 22 that join a path from a parameter), **1,940
tokens**. Then read all 18 and ask the corollary question, not the presence
question. The four bypass sub-classes to test:

1. **Wrong reference point.** The check is real but measured against
   attacker-supplied data. → **F-PATH-02**: `snap_dir.starts_with(repo_canon)`
   where `repo_canon` derives from the same request field, making the assertion
   tautological.
2. **Wrong channel.** Every check is on the field you expected the attack in;
   the attack goes in the un-validated sibling. → F-PATH-02 again: three filters
   harden the repo **name** and snapshot **name**; nothing validates
   `settings.location`.
3. **Escaping the wrong alphabet.** Shell-metacharacter escaping vs option
   injection; HTML escaping vs JSON structure. → **INJ-01**: `String::replace`
   with no JSON escaping at all, then a re-parse.
4. **Order of operations.** Validate-after-destroy. Checked explicitly here:
   `guard_after_destructive_op` is `true` for exactly **1** non-test function in
   the tree, and that one is a detector false positive — PR #69's
   validate-after-`remove_dir_all` bug in `restore_snapshot` is genuinely gone.

**Grep baseline:** there is no grep for "has a join and no containment check".
The honest substitute is to read every file containing the sink:
`grep -rnE '\.join\(|Path::new|PathBuf::from|canonicalize'` → 711 lines / **66
files / 1,317,109 tokens**. Narrowed to just the write sinks
(`create_dir_all|fs::write|copy_dir_recursive`) it is still 137 lines / 27 files
/ **622,709 tokens**. And for the *absent* allowlist, grep returns 0 lines —
there is nothing to match.

### Stage 5 — State machines: TOCTOU, deadlock, lock-across-await

```bash
curl -s "http://127.0.0.1:9310/rustfns/_search" -H 'Content-Type: application/json' -d '{
 "query":{"bool":{"filter":[{"term":{"is_test":false}}],
  "should":[{"term":{"lock_across_await":true}},{"term":{"guard_after_destructive_op":true}}],
  "minimum_should_match":1}},
 "_source":["crate","file","fn_name","line_start","concurrency","lock_across_await","guard_after_destructive_op","is_async"],"size":500}'
```

→ **16 functions, 1,814 tokens** (15 `lock_across_await`, 1
`guard_after_destructive_op`). A complete, readable population.

**Grep baseline:** `grep -rnE '\.(lock|write|read)\(\)|\.await|Atomic|Ordering::Relaxed|exists\(\)|spawn_blocking|block_on'`
→ **3,377 lines / 96 files / 1,381,258 tokens** — and it is structurally the
wrong shape: "a guard held *across* an await" is a relationship between two lines
in one function body, which a line-scoped regex cannot express. You get the
`.lock()` lines and the `.await` lines as separate unordered sets.

## Measured with/without comparison

Every token figure is measured with **tiktoken `cl100k_base`** (a public
GPT-4-class BPE used as an LLM-token proxy) — not a chars÷4 estimate. The XERJ
column is the actual projected result payload; the grep column is the sum of the
distinct files each grep forces you to open, because a grep hit is a line and a
judgment needs the file.

| Stage | XERJ query returns | XERJ tokens | grep lines | grep files | grep tokens | ratio |
|---|---:|---:|---:|---:|---:|---:|
| 1 · Flag dangerous sinks | 318 functions | 40,623 | 1,132 | 110 | 1,585,177 | **39×** |
| 2 · Trace to user input | 13 caller edges | 1,232 | 23 | 5 | 374,872 | **304×** |
| 3 · Privilege model + flows | 477 routes + 24 auth fns | 40,218 | 874 | 73 | 1,271,510 | **32×** |
| 4 · Validate filters for bypass | 18 functions | 1,940 | 711 | 66 | 1,317,109 | **679×** |
| 5 · State machines | 16 functions | 1,814 | 3,377 | 96 | 1,381,258 | **761×** |
| **Total, per-stage sum** | | **85,827** | **6,117** | — | **5,929,926** | **69×** |
| **Total, deduplicated file union** | | **85,827** | | **153** | **1,685,346** | **19.6×** |

Two totals, deliberately. The per-stage sum (69×) is what an auditor working
stage by stage actually pays. The **deduplicated union (19.6×) is the honest
floor** — and it carries the more damning number: the five grep sweeps together
touch **153 of the 197 `.rs` files**, i.e. **1,685,346 of the 1,805,277 tokens in
the entire engine (93%)**. Grepping the five stages *is* reading the codebase,
just in a worse order and without the call graph. For reference, reading all 197
files outright is 1,805,277 tokens — only 7% more than the grep union, and ~9× a
200k context window.

### Recall on this pass

| | XERJ substrate | grep baseline |
|---|---|---|
| Confirmed findings reached | **2 / 2** | 1 / 2 discoverable in principle |
| Refuted before publication | 1 | — |

- **INJ-01** is reachable by grep *only if you already know the name*
  `render_template` (2 files, cheap). Discovering it without the name costs the
  generic 20-file / 772,182-token sweep, and the defect is at line 23,160 of a
  273,864-token file.
- **F-PATH-02**'s *sink* is greppable (`create_dir_all` is in the 27-file set).
  The *defect* is not: it is (a) an allowlist that does not exist —
  `grep -rnE 'path\.repo|repo_allowlist|snapshot_root'` = **0 lines** — and (b)
  the judgment that a real containment check is measured against attacker data.
  Neither is expressible as a regex. We count this as grep-unreachable and say
  why, rather than claiming grep cannot see `create_dir_all`.

Prior calibration on labelled ground truth (PR #69's six known bugs) is in
[DETECTION-QUALITY.md](DETECTION-QUALITY.md): **6/6 vs grep's 4/6**, at 84,122 vs
2,715,003 triage tokens. That run's first attempt scored 3/6 — worse than grep —
and the test set is what exposed three real extractor defects. Recall claims
without a labelled set are worth nothing.

## What the method does not cover

- **Macro-generated code, trait-object dynamic dispatch, and `build.rs` codegen**
  are invisible to the substrate (see [COVERAGE.md](COVERAGE.md)).
- **Callee resolution is name-based.** Method-call edges (`is_method: true`) are
  unresolved; only free/path calls (`resolvable: true`) form a reliable graph. A
  taint path through a trait object will be missed.
- **Precision is the weak axis.** The five stages' candidates yielded 8 confirmed
  findings across sixteen that reached verification. The substrate filters
  thousands of functions down to a readable set; it does not rank, and it produces
  confident false positives (the `sql_ish`/"union" case). A finder without a
  verifier would have published them.
- **Half of the proposed findings were refuted** at the verify stage (8 confirmed,
  8 refuted). That 50% cull — including one where every code observation was
  correct and the finding was still wrong — is the argument for the
  adversarial-refutation step, not a footnote to it. The synthesis layer needs the
  same scrutiny: its auto-summary under-counted the confirmed set as 2, caught only
  by reconciling against the raw per-finding verdicts.

## Reproducing

Substrate build: **~3.2s extract + ~1.9s ingest**, 31 MB on disk. Extractor,
cycle-finder, quality harness and the full query cookbook:
[`../../examples/rust-ast-audit/`](../../examples/rust-ast-audit/). Step-by-step:
[REPRODUCE.md](REPRODUCE.md).
