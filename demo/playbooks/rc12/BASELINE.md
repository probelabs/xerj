# rc.12 baseline — measured 2026-08-07 on `main` @ 8a6fa38 (v1.0.0-rc.11)

Produced by `demo/playbooks/rc12/measure_rc12.sh baseline-nocache 500000`.
Raw numbers: `results-baseline-nocache.json`.

> **This file was rewritten.** Its first version reported `took: 0` for all query
> shapes and concluded that query latency was "at the measurement floor" and that
> a 2x improvement was "not demonstrable". **That was wrong**, and wrong in the way
> this repo has been burned before: the harness had not yet set
> `XERJ_DISABLE_QUERY_CACHE=1`, so 40 identical repeats per shape measured the
> whole-result cache rather than the engine. `bool_must_filter` read **0.37 ms**
> cached and **2,847 ms** uncached — an ~8,000x mirage. See
> `demo/playbooks/CRITICAL_FINDING_read_perf_cache_mirage.md`; the superseded
> numbers survive only in `results-baseline-main.json`, which must not be quoted.

**Corpus.** 500,000 documents, one index, force-merged to a single segment so we
measure steady state. Mapping is explicit (6 keyword, 1 analyzed `text`, 4 numeric,
1 date, 1 boolean). The `body` field is 40 tokens drawn **Zipfian from a
60,000-term vocabulary**; an earlier 24-word vocabulary produced a meaningless
0.0595x ratio and uniformly trivial queries. The generator is seeded, so two runs
index a byte-identical corpus.

---

## Axis 1 — disk

| | value |
|---|---:|
| raw source bytes | 356,593,195 (340 MiB) |
| index on disk | 154,413,167 (147 MiB) |
| **index / raw** | **0.433x** |
| ingest | 31.9 s (≈15.7k docs/s) |
| force-merge to 1 segment | 139.1 s |

| artifact | bytes | share |
|---|---:|---:|
| **`.dv`** doc-values | 57,357,593 | **37.1%** |
| `.seg` stored `_source` | 51,449,817 | 33.3% |
| `.post` postings | 40,951,589 | 26.5% |
| `.ids` | 3,057,388 | 2.0% |
| `.meta` | 1,464,327 | 0.9% |
| `.fst` / `.norms` / rest | ~130,000 | 0.1% |

Within that `.dv`, parsing the `DV01` envelope attributes **54,988,379 B —
95.87%** — to the single analyzed `text` field `body`. That is **35.6% of the
entire index** in one field's doc-values column.

**Corpus dependence is not a footnote.** Across 4,245 MiB of real `xc-*` corpora
the same artifact is only **6.97%** of bytes (1.71%–11.88% per index) because
`_source` dominates there at **64.21%**. This corpus sits at the high end. Any
disk claim must name its corpus.

---

## Axis 2 — query latency (cache off)

| query | p50 ms | p99 ms | `took` |
|---|---:|---:|---:|
| `function_score` | 4,608.30 | 4,655.71 | 4,623 |
| `match_phrase_prefix` | 4,369.24 | 4,651.39 | 4,406 |
| `agg_terms_on_text` | 3,414.46 | 3,481.91 | 3,357 |
| `bool_must_filter` | 2,846.62 | 2,947.36 | 2,758 |
| `boosting` | 2,708.92 | 2,745.64 | 2,696 |
| `sort_on_text` | 1,535.11 | 1,558.18 | 1,530 |
| `match_phrase` | 495.58 | 518.70 | 504 |
| `match_text_multi` | 493.90 | 513.75 | 527 |
| `fuzzy_text` | 306.16 | 313.48 | 309 |
| `wildcard_text` | 180.52 | 184.67 | 177 |
| `prefix_text` | 170.91 | 180.04 | 169 |
| `large_page` | 168.96 | 221.15 | 166 |
| `match_text_common` | 166.29 | 169.30 | 167 |
| `deep_page` | 56.48 | 63.37 | 55 |
| `agg_date_histogram` | 16.90 | 17.25 | 16 |
| `agg_histogram_numeric` | 10.07 | 10.28 | 9 |
| `agg_nested_terms_stats` | 3.33 | 3.45 | 3 |
| `agg_terms_high_card` | 2.75 | 2.99 | 2 |
| `sort_by_numeric` | 2.39 | 2.60 | 1 |
| `term_keyword_high_card` | 1.48 | 1.68 | 1 |
| `term_keyword_low_card` | 0.61 | 0.84 | 0 |
| `range_date_narrow` | 0.53 | 0.81 | 0 |
| `range_long_wide` | 0.43 | 0.56 | 0 |
| `agg_terms_low_card` | 0.41 | 0.71 | 0 |
| `agg_stats_numeric` | 0.20 | 0.27 | 0 |

### The engine is bimodal, and that is the useful finding

| path | p50 range |
|---|---|
| doc-values (`term`, `range`, numeric aggs) | **0.20 – 2.4 ms** |
| FTS (`match`, `prefix`, `wildcard`, `fuzzy`) | **166 – 496 ms** |
| brute scan (`function_score`, `boosting`, `bool`+filter, text aggs) | **1,535 – 4,608 ms** |

Anything doc-values serves is already excellent and should be protected by a
regression guard rather than targeted. Everything else is three to four orders of
magnitude slower, and that is where every remaining latency lever lives.

Note `hits` saturates at exactly **10,000** for broad queries — that is the
scored-total cap, not a corpus property.

---

## How to reproduce and compare

```sh
demo/playbooks/rc12/measure_rc12.sh <label> 500000
# then diff results-<label>.json against results-baseline-nocache.json
```

The harness forces `XERJ_DISABLE_QUERY_CACHE=1` at server boot and frees the
listen port before starting, so a stale instance from a previous label cannot
silently serve the run.
