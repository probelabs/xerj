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

## Experiment 3 — what reads the text `.dv`, and what it returns

This is the crux the first two experiments could not settle: if something reads the
column, dropping it trades disk for latency rather than winning outright.

Run against the 500k-doc baseline index (`BASELINE.md`), whose `body` is 40 Zipfian
tokens per document.

```sh
# terms aggregation on the analyzed text field
curl -s -X POST localhost:9400/rc12bench/_search -H 'Content-Type: application/json' \
  -d '{"size":0,"aggs":{"a":{"terms":{"field":"body","size":3}}}}'

# sort on the analyzed text field
curl -s -X POST localhost:9400/rc12bench/_search -H 'Content-Type: application/json' \
  -d '{"size":1,"sort":[{"body":"asc"}]}'
```

**Both succeed**, served from the `.dv` column. Elasticsearch rejects both by
default (`Fielddata is disabled on text fields by default`) because neither is
meaningful on analyzed text.

| operation | engine `took` | result |
|---|---:|---|
| `terms` agg on `body` | **7,817 ms** | buckets keyed by the **entire 40-token body** as one term |
| `sort` on `body` | **1,651 ms** | orders by the whole body string |

For scale: **every other query shape in the 17-query baseline suite reports
`took: 0`.** These two are the slowest operations measured anywhere in this
campaign, by three orders of magnitude.

A sample bucket key from the aggregation:

```
"bitmap0000 bitmap0004 commit0000 merge0000 shard0232 decode0000 flush0021
 merge0706 index0000 stream0011 filter0001 decode0760 ..."
```

That is one document's whole body as a single aggregation bucket. A terms
aggregation is supposed to bucket by *term*; this buckets by *document*. With
500,000 documents the result is up to 500,000 buckets each occurring once, which is
why `sum_other_doc_count` is 65,533 and the answer carries no information.

---

## What this establishes

The text doc-values column is not a trade. It is a **triple defect**:

1. **Disk.** 37.1% of the index on the 500k mixed corpus, 39.7% on a pure-text
   index. The largest single artifact in both.
2. **Semantics.** The two operations it enables return answers that are wrong in
   kind, not merely imprecise — bucketing by document instead of by term. ES
   refuses them for exactly this reason.
3. **Latency.** Those same two operations are the slowest in the engine by a wide
   margin (7.8 s and 1.65 s against `took: 0` everywhere else).

So the read-path interaction that would have made this a trade does not exist in
the form feared. Nothing *useful* reads the text `.dv`. Dropping it removes the
largest artifact, removes two wrong answers, and removes the two slowest
operations.

**Consequence to decide explicitly:** after the change, `terms`-agg and `sort` on a
`text` field must **error** the way ES does, rather than silently returning fewer
results. That is a deliberate, documented behaviour change and it needs an ES-YAML
conformance check — the suite must stay at 1360 passed / 0 failed / 3 skipped.

**Still to determine:** whether the doc-values *prefilters* (term/terms/range/bool)
or the `size:0` count shortcut ever bind to a `text` field. Those paths are real and
fast; if any of them can target `text`, they need a fallback before the column goes
away.
