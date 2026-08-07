# Measured: honouring `doc_values` on the mapping

A/B on the same seeded 500k-doc corpus, force-merged to one segment,
`XERJ_DISABLE_QUERY_CACHE=1`.

- before: `results-baseline-nocache.json` (`main` @ 8a6fa38)
- after: `results-dvskip.json` (branch `perf/fts-reader-cache`)

The mapping declares one analyzed `text` field (`body`) and no explicit
`doc_values`, so after the change `body` gets no doc-values column — matching
Elasticsearch's default.

---

## Disk — 1.553x smaller, measured end-to-end

| | baseline | dvskip | change |
|---|---:|---:|---:|
| index bytes | 154,413,167 | **99,424,772** | **−35.6% = 1.553x smaller** |
| index / raw | 0.433x | **0.279x** | |

The prediction from parsing the `DV01` envelope was **99,424,789 B**. The
measured result is **99,424,772 B** — within 17 bytes. The attribution method
in `BASELINE.md` is sound and can be trusted for sizing future levers.

### The change is surgical

| artifact | baseline | dvskip |
|---|---:|---:|
| **`.dv`** | 57,357,593 | **2,369,197** (−95.9%) |
| `.seg` | 51,449,817 | 51,449,817 |
| `.post` | 40,951,589 | 40,951,589 |
| `.ids` | 3,057,388 | 3,057,388 |
| `.meta` | 1,464,327 | 1,464,327 |
| `.fst` | 64,485 | 64,485 |
| `.norms` | 28,305 | 28,305 |

Every artifact other than `.dv` is **byte-identical**. The residual 2.37 MB of
doc-values is the keyword and numeric fields, which still get columns because
their mapping still asks for them — exactly as intended.

---

## Write path — an unpredicted 2.33x on force-merge

| | baseline | dvskip | change |
|---|---:|---:|---:|
| **force-merge to 1 segment** | 139.13 s | **59.69 s** | **2.33x faster** |
| ingest | 31.86 s | 28.22 s | 1.13x faster |

This was not predicted and is the largest write-path win of the campaign. The
cause is direct: merge re-encodes the doc-values side-car, and there are now
55 MB fewer bytes to build, sort and zstd. Flush gets a smaller version of the
same benefit.

It is worth noting *why* this matters beyond the number. The corrected baseline
identified force-merge (139 s) as one of the few large, genuinely measurable
performance costs in the engine. A change made purely for footprint reasons
turned out to attack it directly — and it does so by **not doing work**, which is
strictly better than doing the same work faster.

It also removes the tension that made merge-time zstd tiering risky: that lever
raises merge CPU, and merge just got 2.33x cheaper, so the two are far more
compatible than the audits assumed.

---

## Query latency — nothing regressed, and the two column-served shapes got faster

This was the risk: `agg_terms_on_text` and `sort_on_text` were served **from the
column this change removes**. Losing it should, on the naive reading, make them
slower. It did not.

| query | base | dvskip | ratio |
|---|---:|---:|---:|
| **`agg_terms_on_text`** | 3414.46 | **2546.75** | **1.34x FASTER** |
| **`sort_on_text`** | 1535.11 | 1553.13 | 0.99x (unchanged) |
| `bool_must_filter` | 2846.62 | 2705.97 | 1.05x |
| `function_score` | 4608.30 | 4467.75 | 1.03x |
| `boosting` | 2708.92 | 2659.36 | 1.02x |
| `match_phrase_prefix` | 4369.24 | 4395.82 | 0.99x |

**The terms aggregation on a text field got 1.34x faster by losing its
doc-values column.** Walking a 55 MB column whose every entry is a whole
document body was *slower* than the brute path it now falls back to. So the
column was not merely unused — it was actively worse than its own fallback,
while costing 35.6% of the index.

`sort_on_text` is unchanged, which is the expected outcome for the other
column-served shape.

### The apparent regressions are sub-millisecond noise

| query | base | dvskip | ratio | absolute |
|---|---:|---:|---:|---:|
| `agg_stats_numeric` | 0.20 | 0.24 | 0.85x | **+40 µs** |
| `agg_terms_high_card` | 2.75 | 3.06 | 0.90x | +0.31 ms |
| `agg_terms_low_card` | 0.41 | 0.45 | 0.93x | +40 µs |

These are the shapes `RESULT-fts-reader-cache.md` already flagged as unreadable
as ratios: a 40-microsecond move reads as "0.85x". They are a pass/fail guard,
and they pass — nothing left the sub-millisecond band.

### Full-text wins carry through

This run has **both** changes in, so the full-text numbers reproduce the FTS
reader cache result a third time: `match_text_common` 2.86x, `large_page` 2.64x,
`prefix_text` 2.54x, `wildcard_text` 2.45x, `fuzzy_text` 1.56x, `match_phrase`
1.52x. Consistent with both earlier runs to within ~2%.

---

## Correctness

ES-YAML conformance with this change **and** the FTS reader cache in:

```
1365 passed · 0 failed · 3 skipped · 1368 total
```

See `GATE-conformance.md`. The cases that could have caught a wrong
implementation — `aggregations/terms_text_docvalues.yml`, which requires an
explicit `"doc_values": true` on a text field to keep working — pass.

---

## Scope and honesty

**This corpus sits at the high end and the number will not transfer unchanged.**
`body` is ~400 B of a ~713 B document, so doc-values are 37.1% of the index here.
Measured elsewhere, `.dv` is only **6.97%** across 4,245 MiB of `xc-*` code
corpora (1.71%–11.88% per index), because `_source` dominates there at 64.21%.

The defensible public statement is therefore:

> Removes 90–99% of the doc-values side-car. That side-car is 2–37% of index
> size depending on how large your text bodies are relative to `_source` —
> 35.6% on our benchmark corpus.

**Never quote 1.553x as universal**, and never add it to the merge-tier zstd
saving: with `.dv` gone, the zstd lever has a much smaller denominator to work on.
