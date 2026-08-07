# rc.12 baseline — measured 2026-08-07 on `main` @ 8a6fa38 (v1.0.0-rc.11)

Produced by `demo/playbooks/rc12/measure_rc12.sh baseline-main 500000`.
Raw numbers: `results-baseline-main.json`.

**Corpus.** 500,000 documents, one index, force-merged to a single segment so we
measure steady state. Mapping is explicit (6 keyword, 1 analyzed `text`, 4
numeric, 1 date, 1 boolean). The `body` field is 40 tokens drawn **Zipfian from a
60,000-term vocabulary** — this matters: an earlier version of this harness used a
24-word vocabulary and produced a 0.0595x ratio and uniformly sub-0.5ms queries,
which flattered both axes into meaninglessness. Realistic term diversity is a
precondition for this measurement being worth anything.

---

## Axis 1 — disk

| | value |
|---|---:|
| raw source bytes | 356,593,195 (340 MiB) |
| index on disk | 154,413,168 (147 MiB) |
| **index / raw** | **0.433x** |
| ingest | 33.0 s (≈15.2k docs/s) |
| force-merge to 1 segment | **116.52 s** |

Where the bytes are:

| artifact | bytes | share | what it is |
|---|---:|---:|---|
| **`.dv`** | **57,357,593** | **37.1%** | doc-values — full-value column for every string, *including the analyzed body* |
| `.seg` | 51,449,817 | 33.3% | stored `_source`, zstd |
| `.post` | 40,951,589 | 26.5% | postings incl. positions |
| `.ids` | 3,057,388 | 2.0% | external-id → ordinal index, stored raw |
| `.meta` | 1,464,327 | 0.9% | per-term metadata |
| `.fst` | 64,485 | 0.0% | term dictionary |
| `.norms` | 28,305 | 0.0% | per-doc field length |

**`.dv` is the largest artifact at 37.1%**, independently corroborating discussion
#148's 34% measurement on a completely different corpus (WooCommerce source). See
`EVIDENCE-doc-values-on-text.md` for the isolated experiment proving doc-values are
built for `text` fields at all, and that `"doc_values": false` is silently ignored.

---

## Axis 2 — query latency, and why "2x" cannot honestly be claimed here

| query | p50 ms | p99 ms | engine `took` | hits |
|---|---:|---:|---:|---:|
| term_keyword_low_card | 0.30 | 0.49 | **0** | 10,000 |
| term_keyword_high_card | 0.21 | 0.56 | **0** | 1 |
| range_date_narrow | 0.33 | 0.58 | **0** | 50 |
| range_long_wide | 0.30 | 0.46 | **0** | 10,000 |
| match_text_common | 0.29 | 0.36 | **0** | 10,000 |
| match_text_multi | 0.31 | 0.41 | **0** | 10,000 |
| match_phrase | 0.28 | 0.54 | **0** | 10,000 |
| bool_must_filter | 0.37 | 0.74 | **0** | 10,000 |
| deep_page (from 9000) | 1.04 | 1.16 | **0** | 10,000 |
| large_page (size 500) | **4.72** | 5.94 | **0** | 10,000 |
| sort_by_numeric | 1.17 | 1.38 | **0** | 10,000 |
| agg_terms_low_card | 0.21 | 0.30 | **0** | 10,000 |
| agg_terms_high_card | 0.24 | 0.35 | **0** | 10,000 |
| agg_stats_numeric | 0.19 | 0.31 | **0** | 10,000 |
| agg_date_histogram | 0.33 | 0.46 | **0** | 10,000 |
| agg_histogram_numeric | 0.23 | 0.38 | **0** | 10,000 |
| agg_nested_terms_stats | 0.21 | 0.61 | **0** | 10,000 |

### The engine reports `took: 0` for every single query.

That is the headline, and it constrains what rc.12 may claim. At 500k documents on
a warm single segment, XERJ's own timer cannot resolve any of these queries, and
the 0.2–0.6 ms wall-clock is HTTP, JSON serialisation, and loopback — not engine
work. **You cannot demonstrate a 2x speedup on a quantity that already measures
zero.** Any "2x faster" claim built on these shapes would be measuring the harness,
not the engine, which is exactly the saturation artifact that invalidated an
earlier round of XERJ benchmarking.

Only three shapes do measurable work, and they are dominated by *materialisation*
rather than matching:

- `large_page` (4.72 ms) — hydrating 500 documents from `_source`
- `sort_by_numeric` (1.17 ms) — sorting 500k docs by a numeric column
- `deep_page` (1.04 ms) — skipping 9,000 documents

Note also that `hits` saturates at exactly **10,000** for every broad query; that is
the scored-total cap, not a corpus property.

### Where a real, measurable 2x actually lives

On this evidence the honest performance targets for rc.12 are **not** point-query
latency. They are:

1. **Force-merge: 116.52 s for 500k docs.** Real, large, and directly measurable.
   This is also the number that any merge-time compression lever must be scored
   against — the 2026-07-09 playbook's merge-path zstd-19 proposal raises merge CPU
   ~8x, and it must not be scheduled without measuring it here first.
2. **Ingest: 33.0 s (15.2k docs/s).**
3. **Hydration/paging cost**, visible in `large_page`.

To make point-query latency measurable at all would require a substantially larger
corpus or a closed-loop concurrent load generator. Until such a harness exists, the
correct statement for rc.12 is that query latency is **at the measurement floor and
must not regress** — a guard, not an improvement target.

---

## How to reproduce and compare

```sh
# baseline (already captured)
demo/playbooks/rc12/measure_rc12.sh baseline-main 500000

# after a change, on the same corpus and doc count
demo/playbooks/rc12/measure_rc12.sh <change-label> 500000

# then diff results-baseline-main.json against results-<change-label>.json
```

The corpus generator is seeded (`random.Random(20261207)`), so both runs index a
byte-identical corpus and the A/B is valid.
