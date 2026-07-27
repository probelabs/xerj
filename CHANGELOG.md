# Changelog

All notable changes to XERJ are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.0-rc.5] - 2026-07-27

Fifth release candidate: the **real-client compatibility release**. Much of
it came from pointing real Elasticsearch and OpenSearch tooling at
XERJ — Kibana 8.13, Kibana OSS 7.10.2, OpenSearch Dashboards 2.11.1/3.6,
and their own shipped sample datasets (flights, eCommerce, logs) — and
fixing the wrong answers, 500s, and stalls those clients produced. The rest
is storage, ingest and `autoindex` work that did not come from client
testing. Headline for users: the query classes that silently matched
**zero** documents (booleans, `.keyword` multi-fields, `match_phrase` on
arrays — and keyword arrays before a flush, see the qualification below)
now return what ES returns; a real Kibana/OSD instance boots,
logs in, and saves objects against XERJ end to end; and a dashboard firing
several panel queries at once no longer stalls the node. ES-YAML
conformance holds at 1360 passed / 0 failed / 3 skipped. Zero-hit defects
found during this cycle that are **not** fixed here are listed under Known
limitations.

### Fixed — query semantics (classes that silently matched zero docs)

- **Keyword arrays (partial — memtable half only):** a `term`/`terms` query
  on a multi-valued keyword field only ever compared element `[0]` — the
  memtable stored just the first value and the segment keyword column is
  one ordinal per doc — so exact lookups returned silent false negatives
  while `terms` aggregations and `match` (which join all elements) looked
  correct. The memtable path is fixed: those doc-values lookups now bail
  for array fields and fall through to the array-aware source scan, so a
  `term` on a multi-valued keyword field is correct **before** a flush.
  Flushed segments are **not** fixed — the segment keyword column is still
  one ordinal per document, so after a flush a `term`/`terms` on a
  non-first array element can still silently miss. The complete fix needs
  multi-valued segment keyword columns (a storage-format change); the
  regression test
  (`xerj-engine` `test_term_matches_non_first_array_element`) is committed
  but `#[ignore]`d until that lands.
- **Booleans:** `term`/`match_phrase` on a boolean field undercounted to 0
  while a `terms` aggregation on the *same* field bucketed `true`/`false`
  correctly. The trigger was the count shortcut over memtable-resident
  data — i.e. any bulk import before an explicit `_flush`, which is what
  every real importer (OSD's own sample-data loader included) produces.
- **`.keyword` multi-fields:** the brute-force scan's field resolver had no
  multi-field fallback, so `term`/`match_phrase` on `category.keyword`,
  `manufacturer.keyword` and friends matched nothing — `_source` never
  contains a literal `"category.keyword"` key. It now strips the trailing
  segment and retries against the parent when the parent is a leaf value
  (guarded, so a genuinely absent nested/object field is still absent).
- **`match_phrase` / `match_phrase_prefix` on arrays:** both arms only
  handled scalar values, so an array-valued field never matched any
  phrase. ES semantics restored: the doc matches if any element does.
- **`match_phrase` with a non-string query value:** `{"query": true}` was a
  hard parse error surfaced as `search_phase_execution_exception` — the
  exact shape OSD's filter bar sends for a boolean filter pill, which
  broke every panel on the dashboard at once. Scalars are now coerced the
  way `match` already coerced them.
- **Empty query strings:** `match`/`match_phrase`/`match_phrase_prefix`
  with `""` returned 400; ES treats an empty analyzed query as zero terms.
  They now resolve to `match_none` (200, no hits) — this is what Kibana's
  saved-objects `_find?search=*` builds, so that endpoint works again.
- **`geo_point`:** aggregations and `geo_distance`/`geo_bounding_box`
  rejected string-encoded `{"lat": "50.03", "lon": "8.57"}` coordinates
  (the shape the flights sample data ships), and `geo_centroid`/
  `geo_bounds` used a flat lookup that never found a nested geo field.
  Both now behave like ES.
- **Aggregation scripts:** `terms` required a top-level `field` and
  returned `{"buckets": []}` whenever it was absent — any script-based
  terms aggregation silently produced no buckets. `script` is now a real
  key source alongside `field`.
- **Painless date accessors:** `doc['t'].value` returned a plain string, so
  `.getHour()`, `.getDayOfWeek()`, `.getYear()` and the rest of the common
  accessor set failed the whole script. They now parse and extract the
  requested UTC component.
- **Dynamic date mapping:** ISO-8601 strings are inferred as `date` instead
  of `text`; `date_detection: false` was accepted-but-ignored and is now
  honored; a non-date value written into an inferred date field returns
  `mapper_parsing_exception` instead of being silently stored and then
  being invisible to range queries, sorts, and time filters; and
  `date_detection` survives the `PUT /_mapping` merge path.
- **Segment-path parity:** projected FTS bool queries dropped
  `minimum_should_match` (so `match_bool_prefix` with `mm=3` matched every
  single-term hit after flush) and doc-values wildcards did not case-fold;
  nested kNN returned zero hits for any parent doc living in a segment
  because the reassembled `{_id, _seq_no, _source}` shape was not
  unwrapped. `_count` now resolves `terms` lookups.

### Fixed — API surface (what real clients call)

- **Aliases:** single-index endpoints (`GET /{alias}`, `/_mapping`,
  `/_settings`, …) never resolved an alias to its backing index, and
  aliases were never persisted at all — index data survived a restart but
  the alias pointing at it did not. Aliases now resolve everywhere ES
  accepts them and are written to `aliases.json` (atomic temp+rename).
  Together this unsticks a fresh OSD container looping on "Another
  OpenSearch Dashboards instance appears to be migrating the index".
- **`_field_caps`:** never listed declared multi-fields (so `fields=*`
  omitted every `.keyword` entry and index-pattern refresh reported
  "field not found"), and only special-cased the literal `*`/`_all`
  wildcards — any real glob such as `wiki-test*` returned empty caps.
  Both fixed; wildcard/comma index specs also now resolve on 9 further
  endpoints that previously 404'd, 400'd, or — for `_refresh` and
  `_cat/count` — silently reported success / zero documents while doing
  nothing (`_refresh` answered `{"_shards":{"successful":1,…}}` without
  refreshing; `_cat/count` returned 0, indistinguishable from a genuinely
  empty index).
- **`POST /{index}/_update`** never returned a `get` block, so every Kibana
  saved-object write crashed client-side reading `body.get._source`. It is
  now returned exactly when the caller passes `_source`/`_source_includes`/
  `_source_excludes`, matching ES (absent otherwise, so `_doc` responses
  are byte-identical to before).
- **`_source` vs `stored_fields`:** the implicit `_source` suppression that
  `stored_fields` triggers is now only a default — an explicit top-level
  `_source` in the same request wins, as in ES. This is what starved
  almost every column in OSD's Discover.
- **`_bulk`** reports each item's real `_seq_no` and `_version` instead of
  a wall-clock microsecond timestamp and a hardcoded `1`.
- **`_update_by_query` / `_delete_by_query`** honor
  `wait_for_completion=false` and return the async `{"task": …}` form;
  `GET /_tasks/{id}` reports completion with the final response.
- **`/_xpack`** respects `--compat-version` (it had its own hardcoded
  `8.13.0`, which Kibana OSS 7.10.2 surfaced as an opaque "license not
  available" refusal to start), and `/_xpack` + `/_xpack/usage` report the
  real auth state instead of `security.enabled: true` — so `--insecure`
  no longer makes Kibana render a login screen for auth that is off.
- **Missing HTTP verbs:** `_refresh`, `_analyze`, `_msearch` accept GET and
  `_clone`/`_shrink`/`_split` accept PUT, as ES does — the PUT `_clone`
  405 was a fatal error during Kibana's saved-object migration.
- **Login-path endpoints:** HTTP Basic auth (Kibana's interactive realm),
  `POST /_security/profile/_activate`, `GET /_security/profile/{uid}`,
  `GET`/`POST /_security/user/_has_privileges`, and a real
  `GET`/`PUT`/`DELETE /_security/privilege` store with ES-shaped 404s —
  each of these was a 500 or a hang on the Kibana login and home pages.
- **`GET /_cat/templates/{pattern}`** (a 404 that crashed OSD) and
  **`POST /_index_template/_simulate`** (body-only template preview) added.

### Changed — index resolution (can break existing callers)

These three align index addressing with ES 8 and turn some
previously-succeeding requests into errors. Review any caller that relies
on the old behavior.

- **A comma-separated index spec now 404s when a concrete name is
  missing**, implementing ES's default `ignore_unavailable=false`. The
  missing name used to be dropped silently — `POST /real,typo/_refresh`
  answered `200` with `_shards.total: 1`. Wildcard/`_all` specs that match
  nothing are still a valid empty result (ES's `allow_no_indices` default).
- **Wildcard and `_all` expansion no longer sweeps in hidden
  (dot-prefixed) indices**, so an operation like `POST /*/_close` can no
  longer hit `.xerj_users`-class system indices. A dot-prefixed pattern
  opts back in, as ES's hidden addressing does.
- **`POST /_close` refuses a wildcard or `_all` target outright** with ES
  8's `action.destructive_requires_name` `illegal_argument_exception`.

### Added

- **OpenSearch client auto-sensing.** XERJ answers the identity endpoints
  (`GET /`, `GET /_nodes`) per request based on the caller's User-Agent, so
  one running instance serves an `opensearch-py`/`opensearch-js`/OSD client
  and an Elasticsearch client simultaneously, each seeing a
  version/distribution block its own compatibility gate accepts. Explicit
  `--compat-distribution` / `--compat-version` (and the matching env vars)
  still pin the identity for every client. The `x-elastic-product` header
  is no longer sent to a detected OpenSearch caller.
- **Opt-in ingest memory attribution (developer feature).** Setting
  `XERJ_INGEST_MEMORY_TRACE=summary` emits a bounded `xerj.ingest_memory.v1`
  NDJSON ledger — per-owner logical bytes across HTTP bodies, raw and
  parsed semantic sources, prepared docs and vectors, active memtables and
  drained flush snapshots — alongside jemalloc allocated/active/resident,
  RSS, CPU time, and accounting/dropped-event counters, plus a separate
  `/proc`-derived `xerj.process_sample.v1` stream. Default is fully off.
  Merge and read-cache owners are reported `unavailable`, not zero. This
  makes ingest retention *measurable*; it is not itself a memory bound.
  A deterministic bounded diagnostic suite
  (`demo/usecases/autoindex/scale/bounded/`) drives ingest → refresh →
  flush → force-merge → restart with exact count and sentinel checks.
- **Private pprof debugging toolkit.** A Linux-only `debug-profiling`
  feature compiles in bounded CPU and jemalloc heap profiling with no
  network endpoint; artifacts are written mode-0600 to an operator-created
  directory, with `capture.py`/`inspect.py` wrappers that hash the binary
  and every artifact. Not in any shipped feature set; no runtime overhead
  or speedup is claimed.
- **Experimental ONNX embedding backend**, wired through `autoindex` behind
  the `onnx-experimental` feature. Its lazy session initialization is
  cancellation-safe: model loading moved to one process-owned thread whose
  single terminal result is shared, so a cancelled first request can no
  longer strand every later one.
- The ES-YAML conformance runner now exits non-zero when any case fails
  (it previously reported failures but exited 0).

### Changed — performance (concurrent dashboard bursts)

- **Full-corpus aggregations no longer deep-clone the memtable under the
  shard lock.** The `need_full_corpus` path (any request with `aggs` that
  the columnar fast path can't serve) cloned every buffered document's JSON
  tree while holding the per-shard read guard, serialising concurrent
  panels behind O(docs) work each. It now Arc-shares out under the lock and
  clones after releasing it. Measured: 15 concurrent `date_histogram`
  queries against a real ~14k-doc memtable-resident index, 1.2–2.7 s →
  ~150–170 ms (commit `6a54cf5`).
- **Full-corpus aggregations no longer re-decode segments per query.** The
  same path did an unconditional open + decompress + full `serde_json`
  parse of a segment's entire stored section on every request once data had
  flushed. Segments are immutable, so it now uses the existing
  single-flight stored-value cache. Measured: 20 concurrent requests
  against a real flushed eCommerce segment, 15–26 s → 1–4 ms including the
  cold first hit (commit `f6daf70`). A segment read failure during
  full-corpus assembly is now a hard error rather than a silent skip that
  undercounted every bucket.
- **The scan path no longer stalls on cold, concurrent bursts.** Two
  compounding costs: every scanned doc's `_source` was deep-cloned to
  splice `_id` in even though only a deeply-nested `ids` clause needs it
  (now computed once per scan from the query shape), and the raw-fallback
  decode had no single-flight protection, so N concurrent requests against
  a cold segment each paid the full open+decompress. Measured against real
  Kibana OSS 7.10.2 and eCommerce sample data: a 24-concurrent
  `query_string`+`match_phrase` burst immediately after restart (worst-case
  cold caches) went from ~4.5 s per request to 100–165 ms, with the warm
  repeat at 1–5 ms (commit `7e963ff`).
- **Semantic and vector work is bounded by the request.** One absolute
  deadline computed at search entry is carried through single-flight
  waiting, admission, embedding, hybrid/multi-kNN recursion, and exact
  scanning; partials set `timed_out` and `hits.total.relation: gte`, and
  the ES handler aborts its child task when the request is dropped.
  Previously a cancelled semantic search could drain for minutes past its
  timeout. Cold vector segment loads also moved to bounded `spawn_blocking`
  producers so they can no longer pin every async worker.

### Fixed — storage, durability & ingest

- **Raw ingest validates before it publishes.** Caller bytes were appended
  to the WAL before being proven to be complete JSON, so malformed input
  could reserve sequence numbers and become durable, with the later parse
  path silently substituting `{}`. Whole-batch UTF-8/JSON/nesting
  validation now completes before any sequence, WAL, version-map, memtable,
  or schema mutation, and malformed bulk sources return per-item
  `document_parsing_exception`. The parsed `Value` is now the single
  authority for turbo ingest (live indexing and WAL replay could previously
  disagree), and `copy_to` is applied before WAL publication so GET,
  search, flush, replay, and restart all observe the same source.
- **Per-shard WAL buffers bounded to 64 KiB** (was 8 MiB). Every index
  eagerly opens one writer per ingest shard, so a 16×16 default reserved
  ~2 GiB of allocator capacity before a single document was written;
  capacity above a frame cannot batch across requests because every
  acknowledged append already drains the buffer. Measured on 256 empty
  writers: jemalloc allocated 2,147,503,496 → 16,797,064 bytes (−99.2%),
  with ingest throughput ratios of 0.976 (single shard) and 1.020
  (eight shards) and exact replay preserved (commit `761b915`). Cost:
  records larger than the 64 KiB buffer issue more write syscalls
  (measured 1,372 → 3,372 on 131 KiB × 1000), with throughput unchanged.
- **Same-ID writes are serialized.** A keyed publication coordinator now
  spans the current-state/CAS check through WAL, version map, FTS, and HNSW
  publication for single-document paths, closing races that could admit two
  creates, lose an update patch, or let a delete overtake a write. Scripted
  `_update` and `_update_by_query` run through the same boundary, so two
  concurrent `ctx._source.n += 1` requests can no longer both read `n=0`.
  Distinct IDs do not contend. (Turbo batch paths remain follow-up work.)
- **Semantic vectors stay out of full-text indexes.** The dynamic-field
  walkers treated pooled embeddings and their `_chunks` companions as
  ordinary text and fed every float into FTS as a decimal token. In one
  failed diagnostic run that produced 2.338 GiB of vector FSTs inside a
  3.449 GiB partial index. Embedding outputs are now excluded by schema
  designation (not a name heuristic) across pre-analysis, flush, and merge.
  Existing vector FST sidecars are not rewritten on upgrade — reindexing is
  the safe way to reclaim them.
- **The admin API key persists across restarts.** The server wrote
  `data_dir/admin.key` but never read it back, minting a fresh key on every
  start and locking out every client configured against the previous one.
  An existing well-formed key is now reused; anything missing or malformed
  still falls through to fresh generation.

### Fixed — autoindex

- **Content deduplication with crash-safe replacement.** Byte-identical
  paths were indexed independently (one reproduction journaled 2,442
  records for 1,221 live documents). Sources are now hashed with streaming
  XXH3-128 and digest peers byte-verified before one canonical path is
  chosen; every current name is preserved in `ax_paths`. Publication became
  an explicit durable generation transaction (synced `file_replace_start`
  → staged, source-verified extraction → synced `file_done` commit), and
  journal replay repairs torn tails and rolls back failed appends.
  Ambiguous legacy prefix collisions fail closed before any backend
  mutation instead of guessing.
- **Follow-on fixes to that rework:** resume now guarantees exclusive
  plan-key ownership (two files resolving to one key each ran the
  replacement transaction and deleted the other's documents);
  `delete_by_query` repeats until a pass deletes nothing, because the
  server implements it as a single `size:10000` pass and >10k-doc
  generations left permanent ghost documents; and both recoverable error
  paths now name the offending files or journal offset instead of
  advising a blanket `--fresh` re-extract.
- **PDFs are parsed by `pdf_oxide` in isolated same-binary workers.** The
  previous byte scanner interpreted font character codes without resolving
  page resources or ToUnicode maps, producing NUL-separated and shifted
  text that schema inference (correctly) refused to elect as
  `semantic_text`. Workers get process groups, a 1536 MiB `RLIMIT_AS`, and
  descendant kill/reap on timeout — documented as resource isolation, not
  a security sandbox, with no OS memory cap on non-Unix. New
  `--pdf-workers` and `--pdf-timeout-secs` flags. Cost: the no-default
  release binary grows 34,160,328 → 39,018,184 bytes (+4.6 MiB).
- **Text files are classified by sentence density, not line length.** The
  old `avg_len > 60` rule split markdown across two datasets with two
  different field names — adding `## headings` to a document made it *more*
  likely to be treated as a record stream, which split BM25 statistics
  across two corpora. Terminal-punctuation density separates prose
  (0.43–0.57 on the measured corpus) from logs and source (0.00–0.20);
  CSV rows fall on the record-stream side of the same threshold.

### Changed — autoindex (what it produces)

- **Line-oriented text is chunked into overlapping windows** (40 lines, 10
  overlap) instead of one document per line, and prose sections dropped
  from 32 KB to 2 KB with overlap, because BM25 scores per document and a
  single line rarely contains the caller's own wording. Measured on this
  repository (234 files, 170k LOC + docs + 460 commit messages) with 8
  "where/why is X" questions: 3/8 → 7/8 answered, 162,883 → 5,508 records,
  indexing 234 s → 1.9 s (commit `4510189`). Every chunk carries
  `start_line`/`end_line`.
- **`semantic_text` election is by language, not length.** The rule was
  "largest `text` field with `avg_len >= 200`", which elected 300-character
  base64 and concatenated-id columns while skipping a genuinely semantic
  150-character summary. A field must now actually look like natural
  language — `word_ratio >= 0.55` (tokens matching `[A-Za-z]{3,}`) **and**
  `mean_tokens >= 3`. Measured `word_ratio` is 0.00 for
  trace_id/user_id/order_id/numeric columns and 0.78–1.00 for prose, log
  messages and source. This changes which field gets embedded, so it
  affects both semantic-search quality and ingest cost (the built-in neural
  backend measures ~2.8 docs/s). The election note records the numbers that
  drove the decision.

### Added — autoindex

- **`--bulk-timeout-secs`** (default 300, range 1–3600) applies only to
  `POST /_bulk`, so a legitimately slow neural bulk can be accommodated
  without loosening the deadline on control requests. Retries are bounded
  at six attempts with the identical body and deterministic document IDs.

### Docs

- **WordPress core security audit case study**
  (`docs/case-studies/wordpress-security-audit/`): a reproducible record of
  an AI agent auditing real WordPress core (1,492 PHP files, ~619k lines)
  with XERJ as the retrieval substrate — sink census, interprocedural taint
  analysis, authorization graph, POP-gadget hunt — plus a copyable Claude
  Code skill and a step-by-step playbook. The honest result is that **core
  came back hardened**: the documented findings are three Medium-severity,
  known-class items (an incomplete SSRF deny-list missing `169.254.0.0/16`,
  an ImageMagick parse surface, and an unguarded `role` sink in
  `user-new.php`), a fourth downgraded to not-reachable-in-core, and
  verified negatives for IDOR, SQL de-escaping, and sanitizer composition.
  None of it is claimed as a novel 0-day. The published grep-wins
  counter-examples are shown alongside the wins, and the XERJ bug the audit
  surfaced (the keyword-array `term` defect partially fixed above — the
  memtable half) is disclosed.
  Site use case 06 (`/use-cases/code-security-audit.html`) is built from it.
- **Token-usage guidebook** (`docs/TOKEN_USAGE.md`): a measured decomposition
  of what a XERJ answer costs an agent in tokens — envelope overhead,
  answer, and materialized intermediate data — instead of hand-waving.
- **AST + graph + FTS vulnerability research**
  (`docs/research/ast-graph-vuln-detection.md`, `docs/examples/ast-vuln-graph/`),
  including a multi-language tree-sitter taint scanner tested at real
  WordPress scale.
- Verified embedding examples: XERJ with Google AI (EmbeddingGemma, Gemini
  API, ADK) and with any OpenAI-compatible `/v1/embeddings` endpoint.

### Known limitations

Known-open items found while validating this release and **not** fixed in
it. The first three are zero-hit defects — a query returns no documents
that ES would return:

- **`multi_match` looks inverted.** On a 6,022-document index a long query
  returned 0 hits with `operator: "or"` (the ES default) and 2 hits with
  `operator: "and"`. OR must be a superset of AND.
- **`match` against a `semantic_text` field returns 0 hits** for a long
  natural-language string, while the same query against a plain `text`
  field returns thousands.
- **`term`/`terms` on a multi-valued keyword field can still miss after a
  flush** — only the memtable half of that fix landed (see the first bullet
  of "Fixed — query semantics").
- **`autoindex` dataset clustering merges same-shape files** with different
  subjects: 213 source files sharing the schema `{text}` collapse to one
  index, and embedding centroids separate the subject groups only weakly.
- 3 of 1,363 ES-YAML conformance cases are skipped.

## [1.0.0-rc.4] - 2026-07-22

Fourth release candidate: the **production-hardening release**. A 9-review
release-readiness audit produced 17 blockers and ~60 follow-on items; all
were fixed across four hardening waves and verified against the live binary
(ES-YAML conformance 1360/1363, full-matrix benchmark 52 WIN / 0 LOSE / 25
TIE vs live ES 8.13.4). Headline for users: acknowledged writes survive
crashes, wrong-but-200 responses are gone, the node defends itself under
resource pressure, and the bundled Console gains Kibana-quality editable
dashboards.

### Fixed — durability (acknowledged writes survive failure)

- **Acked-write loss closed:** verified WAL prune + power-loss-ordered
  publish chain; `wal_sync="sync"` honored on ALL bulk paths and the
  `wal_batch_ms` fsync loop actually implemented; torn-frame recovery so a
  disk-full/crash tear cannot poison a WAL generation; consecutive `_bulk`
  delete actions no longer dropped; acked deletes survive restart
  (WAL-shard pinning); delete tombstones end WAL pinning segment-durably.
- **Merge-window reads:** GET never 404s during the merge-publish window;
  merges can never silently drop docs; `_forcemerge` is synchronous and
  quiescent like ES.
- **Startup/data safety:** exclusive `node.lock` on the data dir (second
  process fails fast); data-dir format marker refuses newer-than-supported
  or corrupt dirs BEFORE any destructive GC; refuse-on-corrupt snapshot
  restore; HNSW persistence fsyncs file + dir around rename; periodic
  background flusher no longer aborted at spawn; sharded-WAL FTS replay
  restored on reopen.

### Fixed — correctness (no silent wrong answers)

- **Fail-loud sweep:** the silent-wrong-query classes on `_search` are
  rejected with real 400s (unknown fields, unsupported constructs), as are
  CCR auto-follow (501), remote reindex, `has_child`/`has_parent`, learned
  fusion, and SQL `HAVING` — previously all silently returned wrong data.
- **Doc CRUD wire semantics:** real per-doc `_version` and ES seq_no
  convention; `POST /{index}/_doc/{id}` route added; malformed bulk docs
  rejected per-item with ES-shaped 400s instead of stored as empty `{}`.
- **Aggregations:** real `sum_other_doc_count`; composite bucket keys typed
  from the source field mapping; `multi_terms` raises `too_many_buckets`
  as a real 400 past the cap; `top_hits` emits the doc's real `_seq_no`.
- **Query semantics:** ES-exact date resolution for range bounds (rounding,
  format, date math); Painless compares strings as strings (every string
  previously compared equal) with depth + source-length guards; highlight
  offsets correct on multibyte text; `combined_fields` OR pooling;
  `query_string` fallback discloses operator handling; kNN threads
  filter+boost through top-level kNN and honors similarity cutoffs.
- **Doc-values counting (P0):** a `range` filter on non-numeric values
  admitted every memtable document in `size:0`/`_count`/filter-agg paths
  (a one-day date window over-counted 3.4×); date/keyword range bounds now
  compile to the columnar fast path instead of falling to the brute scan.
- **Multi-valued fields:** a field that is multi-valued anywhere in a
  segment no longer ships a lying doc-values column that silently dropped
  those docs from count shortcuts — consumers fall back to the exact scan.

### Added — resource governance (the node defends itself)

- Parent circuit breaker keyed on ACTUAL RSS, global search pool, disk
  flood-stage watermark, per-query memory guard, ANN coverage guard, and a
  search timeout that actually preempts term-dictionary walks; scroll and
  async-search contexts are TTL-swept and capped. Classic node-killers
  (huge `size`, deep pagination windows, bucket explosions) return bounded
  400/429 instead of taking the process down.

### Added — security

- gRPC listener authenticated; health probes exempt from auth; constant-time
  compare for the admin API key; `admin.key` and TLS private keys created
  0600; CORS configurable and restrictive by default; API keys persist
  across restart with an honest role surface; `/_memory` list paginated
  with a documented auth model.

### Added — Console: Kibana-quality editable dashboards

- Durable backend CRUD for dashboards (create/replace/patch/delete with
  ETag optimistic concurrency) — user dashboards survive localStorage
  clears AND server restarts; a real panel builder with live preview
  (11 viz types, index/query/metric pickers); free-form `{x,y,w,h}` panel
  resize + move; first-launch seeding of 13 built-in dashboards as durable
  managed rows; edit-mode chrome no longer overlaps titles or the sub-nav.

### Added — observability

- ES `_stats`/`_cat` surfaces and the 101-series Prometheus endpoint
  reflect real load (docs, bytes, search/indexing counters); slow-query
  log; structured logging minors; `_cat/indices` uuid + bytes columns and
  ES-shaped snapshot responses.

### Changed — performance

- **kNN flipped:** HNSW-served top-level kNN — official benchmark cell
  23,325 ms → 1.87 ms at recall@10 1.00 (vs ES 0.80).
- **Date-filtered aggregations:** 41–49× (one-day window 9.9 s → 241 ms)
  via keyword/date columnar range predicates; filtered `extended_stats` /
  `percentiles` / `percentile_ranks` / `median_absolute_deviation` served
  columnar with filter-aware gathers (11–264×).
- **Scored-columnar family at the ES floor:** multi_match, query_string,
  fuzzy, prefix/wildcard, highlight, match_phrase, deep pagination,
  `more_like_this`, `function_score`, composite aggs, `rare_terms` /
  `significant_terms` / percentile families — full-matrix result
  52 WIN / 0 LOSE / 25 TIE against live ES 8.13.4.
- Mixed read-under-write hardening: one memtable walk per query, flush cap,
  merge-publish count seeding, open-loop iso-load writer for honest
  measurement.

### Fixed — autoindex & agent search path

- `xerj autoindex` no longer aborts the whole run on ordinary UTF-8 in the
  SQL-dump sniffer (byte-buffer accumulation; junk files are skipped and
  recorded, never fatal) and no longer mojibakes non-ASCII SQL values.
- `highlight` is applied before `_source` filtering, so fragment-only
  responses work (measured: 3.2× fewer tokens into an agent context at
  equal recall).

### Docs

- Honesty ledger: canonical audited scorecard, ROADMAP claims flipped to
  measured reality, phantom-claim purge across README/site/docs.
- Production recipes: TLS + auth hardening, air-gapped deploy, ES→XERJ
  migration.

## [1.0.0-rc.3] - 2026-07-10

Third release candidate. Headline: XERJ gains a **built-in neural embedder** —
real in-process BERT semantics with no Python and no external service — behind a
single backend-agnostic embedding handle, plus two new end-to-end-validated
retrieval recipes.

### Added

- **Built-in neural BERT embedder — shipped in the binary.** A pure-Rust sentence
  encoder via `candle` (default `all-MiniLM-L6-v2`, 384-dim) that runs in-process
  and **downloads its weights (~90 MB) automatically on first use** (or reads them
  from `embedding.local_model_dir` for air-gapped deployments). It is compiled into
  the default release binary — end users just add `--embed-mode neural` at runtime,
  no special build and no separate binary. A progress bar and one-time-download log
  make the first run legible. The binary is ~36 MB as a result; a
  `--no-default-features` slim build without the neural backend is ~23 MB.
- **Unified three-backend embedding handle (`xerj_ai::Embedder`).** `semantic_text`
  ingest and `semantic`/`hybrid` queries run through one of three interchangeable
  backends — **lexical** (default, zero-dep feature-hash), **neural** (built-in
  BERT), or **proxy** (external OpenAI-compatible `/v1/embeddings`) — selected with
  `embedding.mode`, the `--embed-mode` flag, or `XERJ_EMBED_MODE`. Misconfiguration
  degrades to lexical, never a crash; `auto` preserves the historical behaviour.
- **Recipe — All-you-can-eat search.** One corpus retrieved five ways from a single
  index: full-text (BM25), semantic, vector kNN (more-like-this), hybrid (RRF), and
  semantic-scoped-by-keyword-filter. Guide `docs/recipes/all-way-search.md`,
  runnable `recipes/all_way_search.py`.
- **Recipe — Zero-config folder → neural semantic search.** `xerj autoindex` a
  mixed-format folder against a `--embed-mode neural` server, then search the
  discovered prose by meaning while structured files stay exactly filterable. Guide
  `docs/recipes/autoindex-semantic-search.md`, runnable `recipes/autoindex_semantic.sh`,
  sample corpus `demo/data/support-folder/`.

### Changed

- `--embed-mode {lexical|neural|proxy|auto}` CLI flag and `XERJ_EMBED_MODE` env on
  the server; new `embedding.{mode,neural_model,model_cache_dir,local_model_dir}`
  config keys.
- Documentation updated for honesty consistency (README, AGENTS.md, ROADMAP.md,
  llms.txt, recipe guides): the **default** embedder is lexical; the neural embedder
  is an **opt-in** upgrade — output is only described as neural when that mode runs.

## [1.0.0-rc.1] - 2026-07-06

First public release candidate of XERJ — an Elasticsearch-wire-compatible search,
vector, and log-analytics engine written in Rust and licensed under Apache-2.0. This
is a release candidate: the wire protocol and on-disk format are considered stable
for evaluation, but may still change before the final 1.0.0.

### Added

- **Elasticsearch-compatible REST API.** Drop-in wire compatibility with the ES
  8.x HTTP surface, served from `xerj-api` (`es_compat.rs`) on port `9200`:
  - Document APIs: `PUT`/`GET`/`DELETE /{index}/_doc/{id}` and
    `POST /{index}/_update/{id}`.
  - Search: `POST /{index}/_search` with `query`, `from`, `size`, `sort`, `aggs`,
    `_source`, and `highlight`.
  - Bulk API: `POST /_bulk` with `index`, `create`, `update`, and `delete` actions.
  - Scroll API: `POST /{index}/_search?scroll=1m` and `POST /_search/scroll`.
  - `POST /{index}/_delete_by_query`, index templates (`PUT /_index_template/{name}`),
    and aliases (`POST /_aliases` with `add`/`remove`).
- **Full-text search (`xerj-fts`).** BM25 scoring with an analyzer registry and
  on-disk postings lists. Supported query types include `match_all`, `match_none`,
  `match`, `match_phrase`, `match_phrase_prefix`, `multi_match`, `term`, `terms`,
  `range`, `prefix`, `wildcard`, `exists`, `ids`, `bool`, `fuzzy`, `regexp`,
  `query_string`, `simple_query_string`, `constant_score`, `boosting`, `dis_max`,
  and `geo_distance`.
- **Vector search (`xerj-vector`).** Dense-vector HNSW index for k-NN and semantic
  search, exposed through the `knn`, `semantic`, and `hybrid` query types.
- **Aggregations.** `terms`, `stats`, `avg`, `sum`, `min`, `max`, `value_count`,
  `cardinality`, `range`, `histogram`, `date_histogram`, `percentiles`, `filter`,
  `missing`, and `composite`, with a columnar fast path for `size: 0` aggregations.
- **Sharded ingest and storage (`xerj-storage`).** Write-ahead log with a single
  monotonic sequence-number writer, a 16-shard in-memory memtable
  (`shard = xxh3_64(doc_id) & 15`), flush to immutable segments, and background
  segment merging. WAL replay rebuilds both the storage and FTS memtables on restart.
- **Log analytics (`xerj-logs`).** Columnar log ingestion with retention.
- **AI helpers (`xerj-ai`).** Text chunking, an embedding proxy, and a memory store
  for semantic workflows.
- **Clustering (`xerj-cluster`).** Embedded Raft consensus for cluster metadata with
  no external dependencies.
- **Bundled console (`xerj-console-api`).** Dashboards, auth, preferences, and
  cluster awareness, compiled into the `xerj` binary and mounted under
  `/_xerj-console/api/v1/*`.
- **Transform pipeline (`xerj-wasm`).** Built-in transform plugins with an optional
  WASM backend.
- **Block compression (`xerj-compress`).** LZ4 and Zstd codecs for segment blocks.
- **Single static binary.** `cargo build --release -p xerj-server` produces `xerj`;
  run with `./target/release/xerj --data-dir ./data --insecure`.
- **ES-YAML conformance harness.** A workspace test runner (`es-yaml-runner`) that
  executes the ES 8.13 REST-API-spec YAML suites (search, aggregations, vectors,
  bulk, indices, scroll, cluster) against a live server. XERJ passes 1,326 of 1,329
  cases.
- **Reproducible head-to-head benchmarks.** A 91-cell XERJ-vs-Elasticsearch-8.13
  matrix (ingest, read, vector, and disk dimensions), published and reproducible at
  <https://xerj.org/benchmarks>. The scorecard is honest about both wins and losses.

### Changed

- `_forcemerge` is now synchronous and quiescent, matching Elasticsearch semantics,
  and merge status is exposed through `_stats`.
- Search hit materialization for `size > 0` is bounded to the top `from + size`
  candidates, reducing per-query cost from O(N) toward O(from + size).
- Bulk ingest avoids redundant JSON round-trips and batches schema evolution to
  raise throughput under concurrent load.

### Fixed

- Consecutive `_bulk` `delete` actions that were previously dropped are now applied
  correctly.
- `hits.total` for `size > 0` searches is delete-aware, resolving a conformance
  regression.
- Corrected top-N sort behavior and delete-awareness across the memtable/segment
  merge path.

### Known limitations

- 3 of 1,329 ES-YAML conformance cases do not yet pass.
- This is a release candidate; some Elasticsearch APIs and query/aggregation options
  outside the list above are not yet implemented. See
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and `engine/CLAUDE.md` for the
  current supported surface.

[Unreleased]: https://github.com/xerj-org/xerj/compare/v1.0.0-rc.5...HEAD
[1.0.0-rc.5]: https://github.com/xerj-org/xerj/releases/tag/v1.0.0-rc.5
[1.0.0-rc.4]: https://github.com/xerj-org/xerj/releases/tag/v1.0.0-rc.4
[1.0.0-rc.3]: https://github.com/xerj-org/xerj/releases/tag/v1.0.0-rc.3
[1.0.0-rc.2]: https://github.com/xerj-org/xerj/releases/tag/v1.0.0-rc.2
[1.0.0-rc.1]: https://github.com/xerj-org/xerj/releases/tag/v1.0.0-rc.1
