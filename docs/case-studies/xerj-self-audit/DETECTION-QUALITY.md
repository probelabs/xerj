# Detection quality — measured against known ground truth

The honest question about any code-security tool is not "does it look
sophisticated" but **"does it find real vulnerabilities, and how much do you have
to read to triage what it returns?"**

PR #69 fixed six real, network-reachable vulnerabilities in this exact codebase,
at locations we know precisely. That makes them a **labelled test set**. This
document reports how the XERJ substrate scores on it, against the baseline an
auditor would otherwise use: grep on the raw tree.

Harness: [`detection_quality.py`](../../examples/rust-ast-audit/detection_quality.py).
It runs against the **pre-fix** index/tree (the bugs must be present to be found)
and, for each bug, issues the query an auditor would write **for that bug class** —
never the bug's own name or line, which would be cheating.

## Results

| ID | bug | XERJ finds it | candidates | rank | XERJ triage tokens | grep finds it | grep files | grep triage tokens |
|---|---|---|---:|---:|---:|---|---:|---:|
| F1 | query_string paren recursion → stack overflow | ✅ | 186 | 6 | 25,049 | ✅ | 21 | 1,010,124 |
| F2 | `IndexName` `..` → restore deletes parent of data_dir | ✅ | 13 | 12 | 2,894 | ✅ | 2 | 327,583 |
| F6 | `more_like_this` cross-product → unbounded alloc | ✅ | 27 | 18 | 17,895 | ✅ | 56 | 1,340,250 |
| F7 | `max_fields_per_index` not enforced on ingest | ✅ | 3 | 2 | 1,050 | ❌ | 1 | 13,366 |
| F8 | bulk NDJSON parse-phase amplification | ✅ | 30 | 9 | 25,760 | ❌ | 0 | 0 |
| F9 | snapshot name unvalidated → write outside repo | ✅ | 21 | 19 | 11,474 | ✅ | 3 | 23,680 |
| | **RECALL** | **6 / 6** | | | **84,122** | **4 / 6** | | **2,715,003** |

**Read that as two separate results.**

1. **Recall: 6/6 vs grep's 4/6.** grep cannot find F7 or F8 *even in principle*.
   Both bugs are **missing code** — an absent limit check, an absent action cap.
   You cannot grep for code that is not there. The substrate can, because
   "function on the ingest path that never consults a configured limit" is a
   property of the AST, not a string. (`grep max_actions_per_bulk` on the pre-fix
   tree returns **0 lines** — the config key only exists *after* the fix.)
2. **Triage cost: 84K vs 2.7M tokens — 32× cheaper.** This is the bigger result.
   grep's literal recall is perfect *for literals*, but a grep hit is a **file**;
   an AST hit is a **function**. Triaging `grep with_capacity` for F6 means
   reading 56 whole files (1.34M tokens). The equivalent XERJ query returns 27
   functions (17.9K tokens). Same bug, two orders of magnitude less reading.

## The part that would be dishonest to omit

**These 6/6 are in-sample.** The first run of this harness scored **XERJ 3/6 —
worse than grep's 4/6.** Publishing only the tuned number would be a lie about
the method, so here is what happened in between. The test set found three real
defects in the *extractor*, each of which I fixed:

| Miss | Root cause (a real detector bug, not a query typo) | Fix |
|---|---|---|
| **F6** | `with_capacity(fields.len() * like.len())` — the argument parser split on the first `)`, truncating to `fields.len(` and **losing the `*`** that is the entire cross-product signal | balanced-paren argument extraction |
| **F6/F8** | taint provenance tracked **only direct parameters**; the sizes come from *locals* derived from a parameter (`let fields = params.get(…)`) | one-hop local-derivation tracking |
| **F2** | the validator signal was **presence-only**. Pre-fix `restore_snapshot` calls `IndexName::new` *after* `remove_dir_all` — decorative validation scored as "guarded". Worse, a generic `starts_with` earlier in the function masked it | ordering signal (`guard_after_destructive_op`), restricted to strong path/name validators |

So the honest claim is: **the substrate reaches 6/6 after being debugged against
this set, and the debugging exposed three generalizable weaknesses** (argument
parsing, provenance depth, order-sensitivity) that any taint-style tool has to
get right. Each fix was verified to also **go quiet on the fixed code** — e.g.
`guard_after_destructive_op` is `true` for pre-fix `restore_snapshot` and `false`
after, so it discriminates rather than just matching.

**The out-of-sample result is the [`/_sql` stack overflow](FINDINGS.md)** — not in
the test set, found on current `main`, and confirmed by aborting a live server.
That is the number to weigh: one previously-unknown Critical, proven by execution.

## Precision is the weak axis — stated plainly

Recall is 6/6; **ranking is mediocre.** The true positive lands at rank 6 of 186
(F1), 12 of 13 (F2), 18 of 27 (F6), 19 of 21 (F9). The substrate is a good
*filter* (207k lines → tens of functions) and a poor *ranker*. In practice that
is acceptable — a 27-candidate list is readable in full, which is the entire point
— but nobody should read these numbers as "it points at the bug." It points at a
small set that contains the bug.

## What a clean sweep of current `main` returned

Running the same lenses on current `main` (post-#69) produced **no new confirmed
findings beyond the `/_sql` DoS**, and one candidate that was **refuted by
testing** — recorded because a finder without a verifier would have published it:

- **Refuted: highlight-tag allocation blowup.** `highlight_text_with_terms`
  (`aggs.rs:7562`) allocates `text.len() + merged.len() * (pre.len() + post.len())`
  where `pre`/`post` are the **user-supplied** `pre_tags`/`post_tags` from the
  search body — a product of two attacker-influenced values, and exactly the F6
  shape. **Tested against a live server**: a 20,000-match document with tag sizes
  from 100 B to 100 KB returned HTTP 200 in ~2 ms with no memory growth. Reading
  the response showed why — the highlighter emits a **truncated fragment** (363
  chars, tag applied once), so `merged` is the matches within one fragment, not
  the document. The allocation is bounded. **Not a bug.**
- `guard_after_destructive_op` flagged exactly **1** function on current main
  (`engine.rs:353 Engine::new`) — a startup path with operator-controlled inputs,
  not network-reachable. Not a finding.

## Reproduce

```bash
python3 detection_quality.py --url http://127.0.0.1:9310 --suffix _pre69 \
    --tree /path/to/pre-fix/checkout --astdir ./ast-pre69
```

Token counts use tiktoken `cl100k_base` (a public GPT-4-class BPE) as an
LLM-token proxy; the harness labels its fallback if tiktoken is absent.
