# rc.12 — what is real, what is not, and what to ship

Written 2026-08-07 from a dogfooded review: seven storage/latency levers audited
against the source and the live corpora, each then attacked by an independent
adversarial verifier. Three levers survived. Four did not. Two of my own earlier
conclusions were also wrong and are corrected below.

Baseline for every number here: `main` @ 8a6fa38 (v1.0.0-rc.11), 500k docs,
force-merged to one segment, **`XERJ_DISABLE_QUERY_CACHE=1`**. Harness and raw
JSON in this directory; method in `BASELINE.md`.

---

## 1. Verdict on the stated target

The target was **2x performance and less disk for the same indexes**.

**Performance: yes, and by much more than 2x — but only on the FTS and brute-scan
paths, which is where essentially all of XERJ's remaining latency lives.** The
engine is bimodal, and that is the single most useful fact in this document:

| path | measured p50 |
|---|---:|
| doc-values (`term` keyword, wide `range`, `stats` agg) | **0.20 – 0.61 ms** |
| FTS (`match` on text) | **166 ms** |
| brute scan (`function_score`, `boosting`, `bool`+filter) | **2,709 – 4,608 ms** |

Anything served by doc-values is already excellent and should be protected by a
regression guard, not targeted for improvement. Everything else is three to four
orders of magnitude slower.

**Disk: yes, but the honest number is a range, not a headline.** Every disk lever
turned out to be strongly corpus-dependent, and the dominant artifact on real
corpora is not the one the RFC targets.

**What must NOT be claimed:** any single "2x" or "0.30x" figure. The verifiers
killed exactly that framing twice — once as a synthetic denominator, once as a
cross-corpus splice. See §5.

---

## 2. The corrected baseline (this is the reference for every rc.12 claim)

### Disk, 500k docs

| artifact | share |
|---|---:|
| `.dv` doc-values | 37.1% |
| `.seg` stored `_source` | 33.3% |
| `.post` postings | 26.5% |
| `.ids` | 2.0% |
| index / raw | **0.433x** |
| ingest | 31.9 s |
| force-merge | 139 s |

**But on 4,245 MiB of real live corpora the shares are completely different:**
`.seg` = **64.21%**, `.dv` = **6.97%** (per-index 1.71%–11.88%). The difference is
how large `_source` is relative to the indexed text. Any disk claim must name its
corpus.

### Latency, 500k docs, cache off

| query | p50 ms | | query | p50 ms |
|---|---:|---|---|---:|
| `function_score` | 4,608 | | `match_phrase` | 496 |
| `match_phrase_prefix` | 4,369 | | `match_text_multi` | 494 |
| `agg_terms_on_text` | 3,414 | | `fuzzy_text` | 306 |
| `bool_must_filter` | 2,847 | | `wildcard_text` | 181 |
| `boosting` | 2,709 | | `prefix_text` | 171 |
| `sort_on_text` | 1,535 | | `match_text_common` | 166 |
| `large_page` | 169 | | `deep_page` | 56 |
| `agg_date_histogram` | 17 | | `agg_histogram_numeric` | 10 |
| `agg_nested_terms_stats` | 3.3 | | `sort_by_numeric` | 2.4 |
| `agg_terms_high_card` | 2.8 | | `term_keyword_high_card` | 1.5 |
| `term_keyword_low_card` | 0.61 | | `range_date_narrow` | 0.53 |
| `range_long_wide` | 0.43 | | `agg_terms_low_card` | 0.41 |
| `agg_stats_numeric` | 0.20 | | | |

---

## 3. Ship list

### S1 — Cache the per-segment FTS reader *(implemented, `perf/fts-reader-cache`)*

`FtsIndexReader::open` sat inside the per-segment loop (`index.rs:14381`), doing
**two** zstd decompressions per field — the whole `.post` (ZPS1) and the whole
`.meta` (ZFM4) — into owned `Vec`s, then discarding them. Measured on one real
production field: **~50 ms and ~41 MB per (segment, field), per query.**

The verifier confirmed the mechanism at high confidence and found the audit had
*understated* it (it had missed the `.meta` decompress, 38% of the cost).

Note the coupling this exposes: `PostData`'s doc comment says `open` "allocates
almost nothing" because it mmaps — true only of the *uncompressed* path.
**Compressing `.post` to save disk is precisely what forces the per-query
inflate.** The disk and latency levers meet at that one decision.

Implementation follows the 14 existing segment-hydration caches: keyed by
`(segment, field-set)`, charged to `SegmentHydrationBudget` under a new
`FtsReader` category, evicted by prefix at merge completion. Segments are
immutable, so no invalidation. Budget refusal returns the reader uncached, so an
oversized corpus degrades to today's behaviour rather than growing unbounded.

### S2 — Honour `doc_values` and `index` on the mapping *(correctness first, disk second)*

`"doc_values": false` is accepted, echoed back by `GET _mapping`, and **silently
ignored** — proven live: aggregating and sorting on such a field both succeed.
`"index": false` is ignored the same way, same root cause. An accepted-but-ignored
mapping option is a correctness bug regardless of any byte savings, and that is
the reason to do this.

Shape: **default off for text/`semantic_text`, honour an explicit
`doc_values: true`.** That matches ES, and the conformance gate stays green (all
three `terms_text_docvalues.yml` cases survive a columnless field — verified).

Key implementation detail the RFC omits entirely: key the policy on the **declared
mapping**, not the resolved `FieldType`, and cover `semantic_text` — it is
**86–92%** of `.dv` on real corpora where plain `text` is under 5%.

### S3 — Merge-time zstd tier at **L6** *(not L9, not L19)*

Recover the tiering documented as "DONE" in `DISK_SIZE_2026-07-09.md` but never
committed (`DV_MERGE_ZSTD_LEVEL` appears nowhere in git history; `index.rs:8333`
merge and `:22548` flush both call the same level-3 encoders).

Ship **L6**: measured −14.4% stored, −14.3% `.dv`, −15.4% `.meta`, −1.4% `.post`.
L9 buys a little more but **doubles the decoder window from 2.00 to 4.00 MiB**,
which point-gets pay on every streaming decode. L19 takes it to 8.00 MiB.

Two conditions bind any published number: it describes **merge-written bytes
only** (a read-only index gains nothing until force-merged), and it **does not
stack** with S2 — if doc-values go first, the L6 win falls from 6.75 to ~3.47 MiB.

---

## 4. Refuted — do not schedule on these

| lever | why it failed |
|---|---|
| Type-aware numeric codecs | Numerics are **1.5%** of `.dv`; keyword columns are 98.5%. Worth ~−0.36% of `.dv`. Worse, the RFC's prescribed "delta + zigzag + FOR" is **not** Lucene's design and unconditionally applied is *worse* than plain FOR (shuffled timestamps: 586,262 B vs 575,375 B). Lucene uses min-subtraction + GCD + a ≤256-value dictionary per 16,384-doc block. |
| Vector-field flattening | "Real but much smaller and far more workload-dependent than claimed." |
| Positions opt-out | Numerator solid (53–85% of `.post`) but `.post` is only ~6% of segment bytes → **4–9% of store**, not the RFC's implied ~14 points. Also prospectively unsafe: without a guard, `match_phrase` on an opted-out field returns `hits.total: 0` with HTTP 200. Lucene throws (`PhraseQuery.java:504-511`). |
| Ingest / merge cost | Refuted. |

---

## 5. Where I was wrong

Recorded because the corrections are more useful than the conclusions.

1. **"2x isn't demonstrable."** Wrong, and my own error: I benchmarked without
   `XERJ_DISABLE_QUERY_CACHE=1`, so 40 identical repeats per shape measured the
   whole-result cache. `bool_must_filter` read 0.37 ms cached and **2,847 ms**
   uncached — an ~8,000x mirage. This repo has a playbook named
   `CRITICAL_FINDING_read_perf_cache_mirage.md` about exactly this.
2. **"Whole-value bucketing is semantically wrong and ES refuses it."** Wrong for
   modern ES, which supports text doc-values behind `mapper.text.doc_values`, and
   XERJ's own suite encodes the behaviour.
3. **"Implementing the doc-values lever breaks the gate."** I reported this from
   the audit; the verifier checked all three cases and refuted it.
4. **`.dv` is 37% of the index.** True of my benchmark corpus, false as a headline
   — it is 6.97% across 4,245 MiB of real data.

Two claimed bugs were also **not reproduced** when I tested them (a multi-valued
count undercount and a silent `match_phrase` zero-hits); both were honestly
labelled "code-derived" by their agents, and the stored-source fallback covers
them today. One audit also contained a **fabricated** bug — a cross-segment
`fts_handled` latch at `index.rs:14348` that is in fact inside the loop.

---

## 6. The redirect worth taking seriously

**`.seg` stored `_source` is 64.21% of real indices, and no RFC lever touches it.**
Every lever in discussion #148 aims at `.dv` (6.97%) or `.post` (~6%). A serious
disk campaign should start where the bytes actually are.

---

## 7. Gate for cutting rc.12

1. `demo/playbooks/rc12/measure_rc12.sh` run on the base commit and on the release
   candidate, same doc count, cache disabled. Both JSONs committed.
2. ES-YAML conformance at **1360 passed / 0 failed / 3 skipped**.
3. Every published number names its corpus and says MEASURED or PROJECTED.
4. No composed disk figure (S2 and S3 do not add).
5. Latency shapes currently at 0.2–0.6 ms must not regress — they are the guard.
