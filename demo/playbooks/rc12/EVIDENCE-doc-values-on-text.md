# Evidence: doc-values are built for `text` fields, and `"doc_values": false` is ignored

Measured 2026-08-07 against `xerj v1.0.0-rc.11` (commit `8a6fa38`), live instance.
Two independent experiments. Both are reproducible with the commands shown.

This file records *empirical* evidence for two claims in discussion #148. It does
not project a saving — see the rc.12 plan for that arithmetic.

---

## Experiment 1 — `"doc_values": false` is accepted, stored, and silently ignored

This is a **correctness bug**, not only a footprint issue: XERJ accepts a mapping
option, echoes it back on `GET _mapping`, and then does not honour it.

```sh
curl -s -X PUT localhost:9200/dvtest -H 'Content-Type: application/json' -d '{
 "mappings":{"properties":{
   "kw_dv_off":{"type":"keyword","doc_values":false},
   "kw_dv_on":{"type":"keyword"},
   "body":{"type":"text"}}}}'
```

`GET /dvtest/_mapping` returns the field with `"doc_values": false` intact — so the
option survives round-trip. Then, with two documents indexed:

```sh
# Aggregate on the doc_values:false field
curl -s -X POST localhost:9200/dvtest/_search -H 'Content-Type: application/json' \
  -d '{"size":0,"aggs":{"a":{"terms":{"field":"kw_dv_off"}}}}'
```

**Result: it succeeds**, returning `buckets: [{alpha,1},{beta,1}]`. Sorting on the
same field also succeeds and returns a populated `sort` key.

Elasticsearch fails both with an explicit error, because doc-values are the only
structure that can serve an aggregation or a sort on a keyword field. XERJ
succeeding proves the doc-values were built regardless of the mapping.

**Why this matters beyond bytes.** A user who sets `"doc_values": false` is making
an explicit storage decision. Today they get neither the saving nor an error, and
nothing in the response tells them the option did nothing. Whatever we decide about
the footprint lever, the honest options are to *honour* the flag or to *reject* it —
silently ignoring an accepted option is the one behaviour that cannot be defended.

---

## Experiment 2 — a `text`-only index still builds a full `.dv` column

The sharper test: an index whose mapping contains **exactly one field, of type
`text`**. There is no keyword, no numeric, no date. Lucene builds **zero**
doc-values for such an index, because you cannot sort or aggregate on an analyzed
body and the bytes would be unusable.

```sh
curl -s -X PUT localhost:9200/dvtext -H 'Content-Type: application/json' \
  -d '{"mappings":{"properties":{"body":{"type":"text"}}}}'
# 2,000 docs, each a 60-token body drawn from a 5,000-term vocabulary
# ... bulk index ... then:
curl -s -X POST "localhost:9200/dvtext/_forcemerge?max_num_segments=1"
curl -s -X POST "localhost:9200/dvtext/_flush"
```

Segment artifacts after force-merge to a single segment:

| artifact | bytes | share |
|---|---:|---:|
| `.seg` stored `_source` | 39,656 | 43.3% |
| **`.dv` doc-values** | **36,333** | **39.7%** |
| `.post` postings | 9,142 | 10.0% |
| `.fst` term dictionary | 187 | 0.2% |
| **total** | **85,318** | |

**The doc-values column is four times the size of the postings** on a pure-text
index, and it is storing the full value of every analyzed body — the one thing
that can never be sorted or aggregated on.

This is consistent with the RFC's measurement on the WooCommerce corpus (`.dv` =
22.92 MiB of 68.14 MiB = 34%). The isolated experiment shows the effect is not an
artifact of that corpus's keyword fields: it is inherent to how XERJ treats `text`.

---

## What this does and does not establish

**Established:** doc-values are built for analyzed `text` fields; the
`"doc_values": false` mapping option is accepted and ignored; on a pure-text index
the resulting column is ~40% of the segment.

**Not established here:** the net saving on a realistic mixed corpus, and — the
crux — what currently *reads* `.dv` for a text field. XERJ's read path grew
doc-values prefilters for `term`/`terms`/`range`/`bool`, and the `size:0` count
shortcut reads from doc-values too. If any of those fire on `text` fields, naively
dropping the column moves queries onto a slower path instead of saving anything,
and the disk win would be paid for in latency. That interaction must be settled
before this lever is scheduled — it is the difference between a real win and a
trade.
