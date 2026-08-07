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
