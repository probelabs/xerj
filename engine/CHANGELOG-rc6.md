# XERJ 1.0.0-rc.6 — changelog

## New: kNN + aggregations in a single request

Aggregations may now accompany a `knn` / semantic query. The aggregations run over
the **retrieved top-`k` neighbour set** (Elasticsearch top-level-knn semantics),
independent of the `from`/`size` hit page. This makes "aggregate a semantic slice"
a single round-trip:

```json
POST /conversations/_search
{
  "query": { "knn": { "field": "vec", "query_vector": [...], "k": 200, "num_candidates": 400 } },
  "size": 0,
  "aggs": {
    "by_band":   { "terms": { "field": "band" } },
    "csat_p50":  { "percentiles": { "field": "csat", "percents": [50] } }
  }
}
```

- Covers every kNN executor (HNSW, exact brute-force, multi-kNN) — they all funnel
  through the shared result assembler.
- An `aggs`-bearing kNN takes the **exact brute-force** path (ANN recall is <100%;
  aggregate counts must be exact, not approximate).
- Aggregation is computed over all top-k sources, before `from`/`size` windowing.

**Known limitation:** `significant_terms` over a kNN set returns empty — it needs a
background corpus which the vector path does not yet supply. Use `terms` (raw
counts) within a kNN slice, or the two-step pattern (kNN → ids → agg with the full
index as background) when statistical significance is required.

Tests: `test_knn_plus_aggregations_single_request` (integration).
