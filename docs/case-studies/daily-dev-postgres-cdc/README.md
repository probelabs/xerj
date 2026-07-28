# Case study: auto-replicate Postgres → XERJ (CDC) + hybrid search — for daily.dev

**Who:** [Ido Shamun](https://github.com/idoshamun), founder of
[daily.dev](https://daily.dev) — the personalized developer news feed (1000+
sources, open-source API at
[`dailydotdev/daily-api`](https://github.com/dailydotdev/daily-api)). daily.dev
runs on Postgres and uses `pgvector` for semantic features.

**His ask:** *"Interesting! is it possible to replicate automatically from pg?"* —
i.e. keep a search/analytics copy in XERJ **in sync with Postgres automatically**,
then run **hybrid (keyword + vector) queries** over it, moving the search workload
off `pg tsvector + pgvector`.

**Answer: yes — and it's tested end-to-end below.** Real Postgres 16 + pgvector
0.8.2 with **logical replication**, a CDC consumer that streams every
INSERT/UPDATE/DELETE into XERJ (with EmbeddingGemma vectors), then hybrid search.
Every result here is real output from the scripts in this directory.

---

## Grounded in daily.dev's real schema

Modeled on the actual `Post` entity in `daily-api`
([`src/entity/posts/Post.ts`](https://github.com/dailydotdev/daily-api/blob/main/src/entity/posts/Post.ts)):
`title`, `summary`, `tagsStr`, `sourceId`, engagement (`views`, `upvotes`,
`comments`, `score`, `trending`), a **`tsv` tsvector** (they already do lexical
search in pg), and — crucially — a **`metadataChangedAt`** watermark column
(daily.dev bumps it on every change). We add a `pgvector` `embedding` column, which
is where the "moving off pgvector" question lives: **pg's `tsvector` and `pgvector`
don't fuse into one ranked hybrid result** — you run two queries and merge in app
code. That's the friction this removes.

---

## The pipeline (tested)

```
 Postgres (daily.dev `post` table, pgvector)
   │  logical replication slot  (WAL → logical decoding)
   ▼
 cdc_sync.py  — reads changed ids, fetches current rows,
   │            embeds title+summary, bulk-upserts / deletes
   ▼
 XERJ `posts` index  (text for BM25  +  dense_vector for kNN, one store)
   │
   ▼
 hybrid query  (BM25 + vector, one request)   +   rc.6 knn+aggregations
```

### 1. Automatic replication — proven live

Initial load streamed 8 posts. Then three live changes in Postgres:
```sql
UPDATE post SET upvotes=2500, trending=99 WHERE id='p7';   -- engagement update
INSERT INTO post (...) VALUES ('p9','WebAssembly beyond the browser', ...);  -- new post
DELETE FROM post WHERE id='p5';                             -- removed post
```
One `cdc_sync.py` pass (drains the slot → XERJ), real output:
```
[cdc] synced 2 upserts, 1 deletes to XERJ  changed ids: [('p7','UPDATE'),('p9','INSERT'),('p5','DELETE')]
  p7 upvotes now 2500 trending 99 (updated)
  p5 deleted from XERJ ✓
  p9 present: WebAssembly beyond the browser
```
The WAL slot captured every mutation; XERJ reflects the update, the insert, and the
delete — no manual reindex. Run the consumer on a loop (`--loops N`) or as a daemon
for continuous, near-real-time sync.

### 2. Hybrid search — the thing pg can't do cleanly

Query *"ditching the JVM search stack for something cheaper"* (semantic; little
literal overlap with any post). Real output:

```
LEXICAL only (BM25 ≈ pg tsvector):     4.00  Why we moved off Elasticsearch
VECTOR only (≈ pgvector):              0.859 Why we moved off Elasticsearch
                                       0.845 The cost of microservices   ← semantically near but off-topic
HYBRID (BM25 + vector, one query):     2.386 Why we moved off Elasticsearch  (upvotes 1203)
                                       2.099 Building a vector search engine from scratch
```
Vector-alone drifts to "cost of microservices"; lexical-alone can't see synonyms;
**hybrid fuses both in one request** and you can layer engagement (`upvotes`,
`trending`) into ranking — exactly what a developer feed needs. In Postgres this is
two separate queries (`tsvector` and `pgvector`) merged in application code.

### 3. Bonus (rc.6): semantic slice + engagement analytics in one request
*"of posts about AI/ML, which sources, and avg upvotes?"* — one `knn`+`aggs` call
returns the source breakdown with average upvotes per source. (See the
[analytics case study](../calltree-analytics/) for that feature.)

---

## Step-by-step (runnable)

Prereqs: Docker, XERJ on `:9200`, Ollama serving `embeddinggemma`, `pip install psycopg2-binary`.

```bash
# 1. Postgres 16 + pgvector, logical replication ON
docker run -d --name pg-daily -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=dailydev -p 5433:5432 \
  pgvector/pgvector:pg16 -c wal_level=logical -c max_replication_slots=4 -c max_wal_senders=4

# 2. schema (daily.dev-modeled) + trigger + logical replication slot
docker cp setup.sql pg-daily:/tmp/ && docker exec pg-daily psql -U postgres -d dailydev -f /tmp/setup.sql
#    …then INSERT your posts (see the seed block in the case study history)

# 3. initial sync + continuous CDC
python3 cdc_sync.py --init            # create the XERJ `posts` index + first drain
python3 cdc_sync.py --loops 100       # near-real-time: drain the slot every 2s

# 4. hybrid query (see the demo in this README)
```

`cdc_sync.py` uses the slot as a **change signal** (which ids changed + op), then
reads the *current* row from Postgres and upserts to XERJ — a robust CDC pattern
that avoids brittle decoder-format parsing and always converges to Postgres state.

---

## Production CDC options (honest tradeoffs)

| approach | latency | setup | notes |
|---|---|--:|---|
| **`test_decoding` slot** (this demo) | seconds (polled `get_changes`) | built-in | readable output; great for a consumer you own; the pattern shown here |
| **`pgoutput` slot** (built-in) | streaming | built-in | native logical-replication protocol; binary — consume via a client lib |
| **`wal2json` slot** | streaming | plugin | JSON change events; easy to parse; needs the plugin installed |
| **Debezium (Kafka Connect)** | streaming | heavier | industry standard for large fleets; exactly-once, schema history, many sinks |
| **watermark polling** (`metadata_changed_at > last_seen`) | seconds–minutes | trivial | daily.dev's schema already has the column; no slot; simplest, at-least-once |

For daily.dev's scale, **Debezium → the consumer**, or a **`pgoutput` slot** read
by a small service, is the production shape; the `test_decoding` consumer here is
the same logic without the Kafka footprint. All of them feed the identical
transform+embed+bulk step.

## Scaling

- **Embedding is the cost, not the sync.** Only changed rows are re-embedded (the
  slot tells you which). A stats-only update (`upvotes`) can skip re-embedding —
  gate on whether `title`/`summary` changed. Batch embeds for throughput.
- **Backfill once, stream forever.** Initial load is a bulk pass; steady state is
  just the change stream. Re-embedding on a model change is a backfill job, not a
  query-time cost.
- **Object-store-backed XERJ** keeps the search copy cheap; hot segments cache.
- **Multi-tenant / per-source:** filter by `source_id`, or per-tenant indices; the
  vector `similarity`/dims are per-field so different embedding models coexist.
- **Ranking:** fold `upvotes`/`trending`/recency into the hybrid score with
  function-score / boosts — feed ranking and search share one index.

## Comparison with staying on pg/pgvector

| | Postgres + pgvector + tsvector | XERJ (CDC from pg) |
|---|---|---|
| hybrid keyword+vector ranking | two queries merged in app code | **one query, fused ranking** |
| search load vs OLTP | competes with primary DB | **offloaded**; pg stays transactional |
| aggregations over a semantic slice | awkward | `knn`+`aggs` in one request (rc.6) |
| keeping in sync | you build it | **logical-replication CDC**, shown here |
| BM25 relevance | `ts_rank` (coarse) | real BM25 + analyzers |
| operational model | one DB (simple) | +1 system, but search/analytics isolated |

**When to stay on pg:** small corpora, low query volume, simplicity over search
quality. **When XERJ wins for daily.dev:** feed/search at scale where hybrid
ranking + engagement signals + analytics matter and you don't want search load on
the primary, kept in sync automatically.

## Honest limitations

1. **No fused RRF hybrid node** — `bool.should[match, knn]` with boosts approximates
   it; principled RRF is a follow-up. (Score scales differ between BM25 and cosine;
   tune boosts or normalize.)
2. **`test_decoding` is polled** via `get_changes` — for true push-streaming use
   `pgoutput`/Debezium; the consumer logic is identical.
3. **Embedding model is yours** — retrieval quality is the model's job; XERJ stores
   and searches the vectors.
4. **Exactly-once** needs care — the demo consumer is idempotent (upsert by id,
   delete by id), which gives at-least-once → convergent state; add slot-LSN
   checkpointing for strict delivery guarantees.

## Files
- [`setup.sql`](setup.sql) — daily.dev-modeled schema + trigger + logical slot.
- [`cdc_sync.py`](cdc_sync.py) — the CDC consumer (slot → embed → XERJ bulk).
