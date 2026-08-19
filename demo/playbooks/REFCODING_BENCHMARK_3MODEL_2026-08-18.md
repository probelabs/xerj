# Reference-coding impact — 3-model controlled benchmark (2026-08-18)

**Question:** what is the *measured* impact of XERJ reference-coding on real coding
tasks, across Opus 4.6 / 4.8 / 5.0 — on tasks a model **cannot pass without it**,
and on token/cost — reported so it is **defensible**, not marketing.

**One-line answer.** On unfamiliar-API tasks with **one shot**, memory alone cannot
produce working code (Opus 4.6 & 4.8: **0/12**); XERJ takes it to **12/12**. As a
model's recall improves the *correctness* gap shrinks (Opus 5.0 one-shots **7/12**),
but the **efficiency** win is universal and consistent: XERJ spends **~1.4× fewer
output tokens than a grep-driven agent** and **4–10× fewer than memory-with-retries**,
at lower cost, on every model. The value is **gated by memorization** — on code the
model already knows, retrieval is neutral-to-harmful (measured, §5).

---

## 1. Method — why this is trustworthy

Three arms of the **same** Claude Code, same tasks, differing only by the reference:

| arm | what it has | isolates |
|---|---|---|
| `bare` | memory only, no tools (retries on its own compile errors) | parametric recall |
| `native` | the reference repo on disk — **must grep to find the API** | full agentic Claude Code |
| `xerj` | **same agent, XERJ pre-retrieved the passage** | Claude Code + reference coding |

`native` vs `xerj` is the headline (identical agent; only difference is whether XERJ
pre-retrieved or the agent had to search). `bare` is the memory floor.

- **Objective, hidden verdict.** Generated code must **compile and pass a `cargo test`
  suite the model never sees** — not a similarity score, not an LLM judge.
- **Contamination-free tasks.** 6 tasks over purpose-built crates (`sift`, `spool`,
  `trellis`, `quill`, `weft`) — real code written for the study, so in **no** model's
  training set. Memory has to *guess* the API. Soundness was proven first:
  `validate_sift.py` → golden solution passes, no-seal variant fails the test, guessed
  API won't compile → **ALL SOUND**.
- **Real tokens & dollars** from `claude -p --output-format json`; the model that ran
  each call is verified from the JSON `modelUsage` field (not assumed).
- **Integrity.** During the run, two `bare`-arm codegen subprocesses tried to obtain
  the reference (`sift`, `weft`) via cross-session messages. Both were **refused** —
  feeding a `bare` arm the reference would contaminate it. Every `bare` `weft-scan`
  trial in the final data was checked: Opus 4.6/4.8 → genuine compile-errors; Opus 5.0's
  one `weft` pass predates the message and matches its overall 7/12 one-shot strength.
  **No contamination in the reported numbers.**

Matrix: 6 tasks × 3 arms × {5-round, pass@1} × Opus 4.6/4.8/5.0 × 2 trials.

---

## 2. Correctness — "cannot pass without it" (pass@1, one shot)

| model | `bare` (memory) | `xerj` (reference-coded) |
|---|---:|---:|
| **Opus 4.6** | **0 / 12** | **12 / 12** |
| **Opus 4.8** | **0 / 12** | **12 / 12** |
| **Opus 5.0** | **7 / 12** | **11 / 12** |

Given one shot, the two older models **never** produce working code against the
unfamiliar crate API; XERJ passes every task. The frontier model (5.0) has enough
recall to one-shot 7/12 — so its correctness lift is smaller (7→11) but still positive,
and it does so at a fraction of the cost (§3).

## 3. Efficiency — output tokens & cost (5 retry rounds allowed)

| model | arm | solved | output tok | cost | mean rounds |
|---|---|---:|---:|---:|---:|
| Opus 4.6 | bare | 4/12 | 75,496 | $16.75 | 2.0 |
| | native | 12/12 | 10,901 | $2.44 | 8.0 |
| | **xerj** | **12/12** | **7,497** | **$2.01** | 5.2 |
| Opus 4.8 | bare | 12/12 | 35,272 | $6.11 | 2.0 |
| | native | 12/12 | 11,693 | $2.09 | 6.2 |
| | **xerj** | 11/12 | **8,385** | **$1.81** | 4.5 |
| Opus 5.0 | bare | 12/12 | 42,354 | $5.98 | 1.6 |
| | native | 12/12 | 11,516 | $2.15 | 5.2 |
| | **xerj** | **12/12** | **8,127** | **$1.94** | 3.4 |

**Headline ratio (native → xerj output tokens), the identical-agent comparison:**

| model | native/xerj out-tok | bare/xerj out-tok | cost native/xerj |
|---|---:|---:|---:|
| Opus 4.6 | **1.45×** | 10.07× | 1.21× |
| Opus 4.8 | **1.39×** | 4.21× | 1.15× |
| Opus 5.0 | **1.42×** | 5.21× | 1.11× |

XERJ is **~1.4× fewer output tokens than grep across all three models** (consistent
with the earlier case study's ~1.65×), in fewer rounds, at lower cost. Note Opus 4.6
`bare` solved only **4/12 even with 5 retry rounds** while burning **$16.75** — the
weaker the model, the more reference-coding matters, on both axes.

## 4. How to read this honestly

- The **correctness** benefit is real but **model-dependent**: dramatic for weaker
  recall (4.6/4.8 pass@1 0/12→12/12), modest for the frontier model (5.0: 7→11).
- The **efficiency** benefit (`native`→`xerj` ~1.4×, cost 1.1–1.2×) is **consistent
  across all three models** and is the safest headline claim.
- Do **not** claim a flat "0→12 for every model." The defensible statement is the
  nuanced one above.

## 5. The honest boundary — memorized code (measured, same harness)

The value is **gated by memorization**. On the `fieldnorm-quant` task over **tantivy**
(a popular public crate the model has trained on), from the case study:

| arm | solved | med output tok | total $ |
|---|---:|---:|---:|
| bare (memory) | 3/3 | 10,213 | $0.98 |
| **native** (grep) | 3/3 | 1,440 | **$0.60 — best** |
| xerj (inject) | 3/3 | 8,504 | $1.43 — **worst** |

All arms pass because the code is memorized (`bare` reproduced the exact 256-entry
norm table from memory). Here **retrieval is the worst arm** — injecting a large
reference backfires on bulk-data reproduction. Reference-coding wins on **private,
internal, niche, or post-cutoff** code; it is overhead on popular public libraries the
model already knows. That boundary is a feature of the honest claim, not a caveat to
hide.

## 6. Recognized benchmarks (SWE-bench Verified) — transparency

The recognized benchmark that tests this capability is **SWE-bench Verified** (patch a
real repo so its hidden tests pass — the natural fit for "index the repo and retrieve
the code to change"). It **could not be run in this sandbox**, and this was verified,
not assumed:

- **No usable Docker daemon** — SWE-bench's official harness runs each repo's tests in
  per-repo Docker images; the daemon is present but not runnable here.
- **Python-version wall** — only Python 3.14 is available; SWE-bench instances pin
  2013–2020-era commits (e.g. `psf__requests-1142` @ 2013) whose `setup.py` will not
  build under 3.14 / modern setuptools. Probed end-to-end on `flask-5014` and
  `requests-1142`: both fail environment setup at baseline.

We do **not** substitute a hand-rolled n≈2 scaffold and call it "the SWE-bench score" —
that is precisely the kind of number that is not defensible. Instead: a **turnkey
SWE-bench Verified harness** (with the XERJ index/retrieve arm) is provided so the
**official** number reproduces in any Docker-capable environment — see
[`tools/xerj-code/swebench/README.md`](../../tools/xerj-code/swebench/README.md).

## 7. Reproduce

```sh
# server + corpus
xerj --insecure --port 9450 --data-dir /tmp/xbench --embed-mode lexical &   # background
XERJ_URL=http://localhost:9450 XERJ_BIN=$PWD/engine/target/release/xerj \
  bash .claude/skills/xerj-code/scripts/xc-index.sh novel-libs --fresh

# soundness (must print ALL SOUND)
cd .claude/skills/xerj-code/measure && python3 validate_sift.py

# a model, both settings
TASKS="tasks/sift-build.json tasks/sift-until.json tasks/spool-window.json \
       tasks/trellis-order.json tasks/quill-codec.json tasks/weft-scan.json"
ANTHROPIC_MODEL=claude-opus-4-8 XERJ_URL=http://localhost:9450 \
  python3 csrun.py novel-libs $TASKS --arms bare,native,xerj --trials 2 \
  --corpus-dir ~/.xerj-code/corpora/novel-libs --out results-opus48.json          # efficiency
ANTHROPIC_MODEL=claude-opus-4-8 XERJ_URL=http://localhost:9450 \
  python3 csrun.py novel-libs $TASKS --arms bare,xerj --trials 2 --rounds 1 \
  --out results-pass1-opus48.json                                                  # correctness
```

Raw result JSONs for this run: `/home/claude/ai/xerj-bench/results-*.json`.

## 8. Limitations (stated, not buried)

- n is modest (6 tasks × 2 trials/model); pass/fail on contamination-free API tasks is
  fairly deterministic, but confidence intervals at this n are wide — treat the
  *direction and magnitude* as the result, not the third significant figure.
- Purpose-built crates guarantee non-memorization but are simpler than real repos; the
  recognized-repo signal is exactly what the SWE-bench harness (§6) is for.
- `bare` is memory + compile-error retries, not pure one-shot except in the pass@1
  setting; both settings are reported so the reader picks the lens.
- A `kv-oss` re-index during this session hit an engine catalog error
  (`catalog read-back … disagrees with the sealed generation projection`) — filed as a
  separate finding; the memorized boundary (§5) is cited from the prior verified run.
