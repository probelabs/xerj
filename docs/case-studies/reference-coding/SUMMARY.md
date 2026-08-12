# Reference-coding with XERJ — case study summary

**Question:** on the *same* model, does giving Claude Code a XERJ-retrieved reference
beat plain Claude Code — in tokens and in whether the code is correct?

**Method (real, reproducible).** Three arms, one objective verdict (the generated
code compiles and passes a hidden test the model never sees), real tokens and
dollars from `claude -p --output-format json`:

- **bare** — Claude Code, tools disabled: answers from memory only.
- **native** — Claude Code as-is: it greps the reference source itself.
- **xerj** — the *same* Claude Code, with the reference retrieved by XERJ and
  injected. Only the retrieval differs from native.

**Corpus.** 13 purpose-built reference libraries across **5 languages** (Rust,
Python, JavaScript, C, Java), each real (compiles, passes its own tests) but
written for this study — so it cannot be in any training set — and each carrying a
runtime **contract the compiler cannot warn about** (a seal-before-read, a
generational handle, a lazy refill, an LRU eviction order). Plus two *memorised*
public controls (tantivy; valkey + memcached) to show where retrieval is worth
nothing.

## The result

**On code the model has not memorised** (7 domains with a genuinely unrecallable
contract, 21 runs): bare solves **1/21** and burns **$21.9** flailing; native and
xerj each solve **21/21**; xerj does it at **1.5× fewer output tokens and 1.3×
lower cost than native**, and **6.5× cheaper than bare**.

**Across five languages** (16 runs): native and xerj both solve 16/16; xerj uses
**9,982 output tokens to native's 26,477 (2.7×)** and **$1.58 to native's $3.27**;
bare solves 11/16 and spends up to **278×** xerj's tokens (Java) recovering or
reinventing. Concrete: on one Java task bare wrote a **503-line** append-log and
still failed the seal contract; xerj wrote the correct 4-line solution in **103
output tokens**.

**On memorised public code, retrieval is neutral-to-harmful** — the model
reproduces even a 256-value quantization table from memory, so bare is cheapest and
injecting a large reference can be the worst arm. The value is gated by
memorization, and the line is sharp.

## Honesty notes

- The unfamiliar libraries are *synthetic* (unfamiliar by construction); a real
  private codebase is the untested end state.
- The native-vs-xerj gap is measured on small references, where `grep` on a
  distinctive name is already precise; the advantage should grow on a large private
  corpus (untested), and on behavioural/semantic queries with a neural embedder
  (untested; the shipped embedder is lexical).
- We separately tried to improve the XERJ *server* retrieval (richer symbols +
  identifier tokenization) and measured it honestly: **no improvement on realistic
  queries** on this corpus — see `SERVER_UPLIFT_SCORECARD.md`. The robust win is
  retrieval-vs-none, which both server versions deliver equally.

Full write-up: `CASE_STUDY.md`. Raw per-run data: `data/`. Captured generated
programs (the 503-line failure vs the 4-line success): `generated/`. The retrieval
tooling — and pinned, licence-reviewed definitions of the corpora used here — ship
in `tools/xerj-code/`; see "How you reproduce it" in `CASE_STUDY.md` for what is
and is not included.
