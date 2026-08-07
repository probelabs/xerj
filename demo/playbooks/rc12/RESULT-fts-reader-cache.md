# Measured: per-segment FTS reader cache

A/B on the same seeded 500k-doc corpus, force-merged to one segment,
`XERJ_DISABLE_QUERY_CACHE=1`, 40 runs per shape after 5 warm-ups.

- before: `results-baseline-nocache.json` (`main` @ 8a6fa38)
- after: `results-ftscache.json` (branch `perf/fts-reader-cache`)

The binary measured predates the `_nodes/stats` reporting commit, which changes
only JSON output and cannot affect query latency.

---

## Result

| query | base p50 ms | cached p50 ms | speedup |
|---|---:|---:|---:|
| `match_text_common` | 166.29 | **57.85** | **2.87x** |
| `large_page` | 168.96 | **64.05** | **2.64x** |
| `prefix_text` | 170.91 | **69.22** | **2.47x** |
| `wildcard_text` | 180.52 | **74.79** | **2.41x** |
| `fuzzy_text` | 306.16 | **195.25** | **1.57x** |
| `match_phrase` | 495.58 | 339.90 | 1.46x |
| `match_text_multi` | 493.90 | 342.99 | 1.44x |
| `bool_must_filter` | 2846.62 | 2764.02 | 1.03x |
| `agg_terms_on_text` | 3414.46 | 3370.50 | 1.01x |
| `boosting` | 2708.92 | 2704.39 | 1.00x |
| `function_score` | 4608.30 | 4588.56 | 1.00x |
| `sort_on_text` | 1535.11 | 1576.16 | 0.97x |
| `deep_page` | 56.48 | 59.06 | 0.96x |
| `match_phrase_prefix` | 4369.24 | 4684.25 | **0.93x** |
| **sum of 25 shapes** | 21,560.6 | 20,930.8 | **1.03x** |

Disk is unchanged (154,413,167 → 154,413,175 B, 8 bytes on 147 MiB). Ingest
31.86 → 31.24 s and force-merge 139.13 → 127.45 s are unaffected by design — the
cache is read-path only, and both deltas are within run-to-run variance.

---

## What it does and does not buy

**It buys 2.4–2.9x on the mid-tier full-text shapes**, which is exactly the set
whose cost was dominated by reopening the segment side-cars. That confirms the
mechanism: `FtsIndexReader::open` inflating the whole `.post` (ZPS1) and `.meta`
(ZFM4) per query was the dominant term for those queries and is now paid once.

**It buys nothing on the four multi-second shapes** (`function_score`,
`match_phrase_prefix`, `agg_terms_on_text`, `boosting`, `bool_must_filter`),
because their cost is brute scanning and scoring every document, not opening a
reader. Those need separate work.

### The aggregate is 1.03x, and that is the honest number for "the suite"

Because the four brute-scan shapes dominate the sum of p50s, the total barely
moves. This is a concrete demonstration that **"sum of independent p50s" is a bad
headline metric** — the same criticism an adversarial verifier levelled at the
2.70x figure in `READ_SCORECARD_2026-07-09.md`. Quote the per-shape numbers, or
quote a real workload mix; never quote the sum.

**Defensible claim for rc.12:** *"2.4–2.9x on common full-text queries
(`match`, `prefix`, `wildcard`, large result pages); 1.4–1.6x on phrase and
fuzzy; no change on brute-scan families."*

---

## Repeat run — the regression did not reproduce

A second independent run (`results-ftscache-rep2.json`, fresh instance, fresh
data dir, same seeded corpus) settles the `match_phrase_prefix` question:

| query | base | run 1 | run 2 | run 2 vs base |
|---|---:|---:|---:|---:|
| `match_phrase_prefix` | 4369.24 | **4684.25** | **4460.21** | 0.98x |
| `match_text_common` | 166.29 | 57.85 | 57.21 | **2.91x** |
| `prefix_text` | 170.91 | 69.22 | 66.94 | **2.55x** |
| `wildcard_text` | 180.52 | 74.79 | 73.52 | **2.46x** |
| `large_page` | 168.96 | 64.05 | 64.36 | **2.63x** |
| `fuzzy_text` | 306.16 | 195.25 | 197.24 | **1.55x** |
| `match_phrase` | 495.58 | 339.90 | 327.65 | **1.51x** |
| `match_text_multi` | 493.90 | 342.99 | 334.22 | **1.48x** |
| `function_score` | 4608.30 | 4588.56 | 4564.46 | 1.01x |
| `boosting` | 2708.92 | 2704.39 | 2713.27 | 1.00x |

`match_phrase_prefix` moved 4684 → 4460 between the two cache runs, a 5% swing
with no code change. Its earlier 0.93x was **run-to-run variance on a
multi-second brute-scan query, not a regression** — as suspected but, correctly,
not assumed.

**The wins are stable to within ~2% across both runs**, which is the more
important result: the 2.4–2.9x on the mid-tier full-text shapes reproduces
exactly, and the brute-scan families are flat in both.

### What this says about the harness

Multi-second shapes carry ±5% run-to-run noise at 40 repetitions. Any future
claim about `function_score`, `boosting`, `match_phrase_prefix`,
`agg_terms_on_text`, `bool_must_filter` or `sort_on_text` needs either more
repetitions or repeated runs — a single run cannot resolve anything smaller than
about 10% on those. The sub-millisecond shapes are noisy in relative terms for
the opposite reason (`agg_stats_numeric` 0.20 → 0.27 ms reads as "0.75x" but is
70 microseconds); they should be read as a pass/fail guard, not as ratios.

---

## The one regression, stated plainly (superseded by the repeat run above)

`match_phrase_prefix` moved 4369.24 → 4684.25 ms (0.93x). This is **above noise
by the p99 test**: the cached p50 (4684) exceeds the base p99 (4651).

I investigated the obvious mechanism — that the new cache competes for the shared
`SegmentHydrationBudget` and starves the stored-scan caches — and **refuted it**.
On the post-change instance:

```
limit_in_bytes      25,607,935,590
current_in_bytes     3,830,604,688     (15% of budget)
admission_refusals               0
```

Nothing was refused and the budget has ample headroom, so budget competition is
not the cause.

**What I have not established** is the actual cause. The two runs are separate
server processes over separate data directories, so page-cache state and merge
scheduling differ (force-merge took 127 s vs 139 s, implying a different merge
schedule). For a 4.4-second brute-scan query, 7% run-to-run variance between
independent instances is plausible but unproven. The small moves elsewhere
(`sort_on_text` 0.97x, `deep_page` 0.96x, `agg_stats_numeric` 0.20→0.27 ms) fit
the same variance pattern.

**This must be settled by repeated runs before the change merges.** Either it
reproduces — in which case it is a real regression in the multi-term expansion
path and needs a cause — or it does not, in which case the harness needs more
repetitions per shape for multi-second queries to be trustworthy at all. Do not
merge on the assumption that it is noise.
