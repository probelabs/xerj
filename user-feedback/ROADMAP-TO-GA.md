# XERJ — Roadmap to 1.0.0 GA

Date: 2026-08-07. Baseline: v1.0.0-rc.12 (released today). Conformance baseline: ES-YAML
1365 passed / 0 failed / 3 skipped.

**How this document was produced.** Four domain audits of XERJ's source were run against the
user-feedback corpus (`user-feedback/`, 130 files of real Elasticsearch user pain), each audit
adversarially verified by an independent pass that opened every cited `file:line`. Domains covered:
operations/cluster/Kubernetes, memory/OOM, query and write performance with durability, and data
model/mappings/upgrades. The audit set for the remaining feedback categories (security, AI/vector,
docs/UX, log-analytics, licensing, cost, vendor, ecosystem) was not delivered into this synthesis —
see §8 for what that means for the blocking count. The fourth domain's verifier verdict was also not
delivered; its load-bearing citations were re-verified directly for this document (§8).

**Rules this document obeys.** Every claim about XERJ cites a `file:line` that was actually read.
Peer citations carry `project path:line (licence)`; `elasticsearch` (AGPL/SSPL/Elastic) is
approach-only, never copied. Every number is measured, or labelled PROJECTED with its arithmetic.
Corpus-dependent numbers ship as ranges. The default embedder is lexical feature-hashing and is
never called neural. Verifier disputes are carried inline, not silently resolved.

---

## 1. Verdict

XERJ is not ready for 1.0.0 GA today. **Ten GA-blocking items** stand between rc.12 and an honest
tag: five small, one small-to-medium, four medium, none large. The failure pattern behind almost all
ten is the same: the engine's core answers to Elasticsearch's worst pains are real and verified
(§2), but the edges lie — health endpoints that cannot report failure, a scroll API that silently
truncates exports at 10,000 documents, management state that a plain restart erases, sidecar
corruption that degrades silently instead of loudly, a Helm chart that inverts the engine's own
secure default, and published numbers (settings count, "~7 ms recovery") that do not survive a
count. The shortest honest path is three waves: a few days of verification tests (Wave 0), the five
small items in parallel (Wave 1), then the four medium items plus the lifecycle item on a
coordinated branch (Wave 2) — each wave gated on ES-YAML `0 failed` and
`demo/playbooks/rc12/measure_rc12.sh` within noise of the rc.12 baseline (§7). Two caveats bound
the verdict. First, ten is a floor, not a ceiling: it counts only the four audited domains, and the
security domain in particular needs its own audited pass before the tag (§8). Second, GA means
**single-node GA**: the cluster data plane is deliberately unwired (§5), and nothing in the blocking
set changes that.

| # | Blocker | Domain | Effort |
|---|---------|--------|--------|
| 1 | Cursor-based scroll — kill the silent 10k export truncation | query | medium |
| 2 | Persist management state across restart | data model | medium |
| 3 | Failed-index lifecycle: reason, delete, retry, fsck | ops | medium |
| 4 | Readiness must not permanently pull the pod over one failed index | ops | small |
| 5 | All health surfaces tell the same truth | ops | small |
| 6 | Sidecar/format version failures must be loud, not silent | data model | medium |
| 7 | Retention/ILM: wire the promised TTL driver or retract the claims | data model | small–medium |
| 8 | Helm chart secure-by-default | ops | small |
| 9 | Expose breaker state; document per-platform coverage | memory | small |
| 10 | Correct the settings-count claim, drift-proof the count | ops | small |

---

## 2. What XERJ already answers

The feedback corpus is a list of reasons people leave Elasticsearch. Where XERJ already removes the
reason, verified by audit and adversarial re-check, it is listed here — this is the marketing story,
and each line is citable.

**OOM — "the single most common operational failure" (user-feedback/03-jvm-and-memory/oom-incidents.md:4).**
A process-wide parent breaker is ON by default and turns memory exhaustion into HTTP 429, not death:
a summed-memtable budget (auto = 25% of the effective memory limit, floor 2 GiB, cap 50% —
engine/crates/xerj-engine/src/governor.rs:391-395) and an RSS admission watermark at 95% of the
cgroup-aware limit (engine/crates/xerj-common/src/config.rs:999-1025, 1049-1051), both latched and
auto-releasing (governor.rs:246-263), with the module doc stating the design goal plainly: "a 429
beats the OOM-killer" (governor.rs:1-26). This is incident-hardened, not checkbox: the sampler runs
on dedicated OS threads because a tokio-task version was observed starving while a MemoryMax=2G
process was OOM-killed (engine/crates/xerj-engine/src/engine.rs:1548-1613). cgroup awareness covers
v2 "max" leaves, v1 slices, and Docker bind-mounts, with a pinned regression that a 1 GiB container
cap yields a 512 MiB budget (governor.rs:638-766, 894-1107). Measured empty-install footprint:
~80.6 MiB RSS (82,560 KB by ps; one measurement, one Linux host, 2026-08-07) against the corpus's
"ate 33GB from the start" complaint (oom-incidents.md:11-13).

**GC pauses (gc-pauses.md:12-14).** Answered by construction — no JVM exists. `_nodes/stats` reports
real process RSS and host memory in the `jvm.mem` block ES clients key on, and explicitly omits GC
collector stats rather than faking them (engine/crates/xerj-api/src/es_compat.rs:19867-19874).

**The "passes pre-flight then explodes" crash class (gc-pauses.md:35-39).** Bounded structurally,
not by estimation: `max_buckets` (65,536) is enforced at the accumulator boundary on both agg
executors with an ES-shaped `too_many_buckets_exception` (config.rs:991-998;
engine/crates/xerj-engine/src/aggs.rs:125-175); result windows cap at 10,000 with a typed 400
(config.rs:981-987; engine/crates/xerj-common/src/error.rs:241-267) plus a per-query hydration
breaker that 429s before allocation (engine/crates/xerj-engine/src/index.rs:12613-12634); scripts
share the request deadline, ending the runaway-script thread-pool exhaustion
(index.rs:12659-12676); bulk caps at 50,000 actions / 100 MiB body before parse-phase amplification
(engine/crates/xerj-engine/src/bulk.rs:267-333; config.rs:958-980).

**Acknowledged-write loss — Jepsen's 33-90% (production-incidents.md:9-18).** Every mutation hits
the WAL before the in-memory index with per-frame CRC32C (engine/crates/xerj-storage/src/wal.rs:1-22);
torn tails are truncated and NACKed, mid-file corruption freezes the generation instead of
truncating acked entries (wal.rs:297-319, 395-435); generations are pruned only when every entry is
individually proven durable-or-superseded — replacing a rule that had caused 50/50 acked-doc loss on
kill -9 (wal.rs:1003-1029); replay replays everything through idempotent consumers, proportional to
genuinely-unflushed data (wal.rs:1191-1206). The default contract: acked writes survive process kill
at the moment of ack (append drains to the kernel page cache before ack, wal.rs:544-554); the
power-loss window is bounded by `wal_batch_ms=100` (config.rs:559-560); `wal_sync="sync"` gives
per-bulk group commit at ES request-durability granularity (config.rs:511-528). Any public claim
must state the 100 ms default window (see §4 item 2). Single-node default means the partition-loss
class cannot occur at all.

**90-hour recovery death spirals (community-horror-stories.md:68-70).** Unclean restart is
automatic and bounded: a chaos suite exercises crash-during-writes, five corruption shapes, rapid
restart loops, and memory pressure (engine/crates/xerj-engine/tests/chaos_tests.rs:154, 288-342,
492, 1053); ENOSPC mid-frame rolls back cleanly (wal.rs:1830, 1903); one corrupt index quarantines
instead of bricking the node (engine.rs:495-515); a stale `node.lock` cannot wedge a reboot (OS
advisory lock, engine.rs:550-603); a data-dir written by a newer xerj is refused before any
destructive step (engine/crates/xerj-storage/src/index_store.rs:595-608, 772-778).

**Config sprawl — "3,000+ settings" (01-operational-complexity/learning-curve.md:18-20).** An empty
or absent config file is valid; every setting has a production default; a typo'd key fails startup
(`deny_unknown_fields`); schema-present-but-unwired features are rejected loudly at startup
(config.rs:42-47, 94-108, 198-229). The true count is 103 serde-visible settings — measured by
`count_user_facing_settings`, which serialises `Config::default()` and counts leaf keys, not by
hand. The published "38"/"<50" figures were wrong; they were corrected with blocker 10
(2026-08-09, #207). 103 vs 3,000+ is ~29x fewer (PROJECTED: 3000/103), told truthfully.

**Shard sizing expertise (04-scaling-and-shards, cluster-management.md:11).** No up-front shard
decision exists: ES `number_of_shards` is stored and echoed for wire-compat but never used as a
layout input; internal WAL/ingest shards auto-derive from CPU count
(engine/crates/xerj-engine/src/wal_shard_settings_tests.rs:79-94; config.rs:1106-1136). Nuance the
verifier added: an out-of-range `index.xerj_ingest_shards` override is silently ignored in favour of
the derived default, not loudly refused (wal_shard_settings_tests.rs:126-140) — do not describe it
as "refused" in public docs.

**The 1-second refresh gap and refresh=true tax (write-performance.md:9-13).** Search reads the
live memtable directly and prefers it over segment copies, so an acked write is searchable
immediately; refresh is a flush-to-disk operation, not a visibility gate (index.rs:13443-13446,
12846-12851, 7619-7686).

**Deep pagination (query-performance.md:9-15).** `search_after` is applied during collection in a
bounded top-(from+size) heap, so page N costs the same as page 1, with `seq_no` tie-breaking to ES
semantics (index.rs:30081-30095, 30048-30059). Exact hit totals by default, even past the
materialisation cap (index.rs:13433-13441), where ES defaults to lower bounds at 10k. The scroll
path is the exception — blocker 1.

**Merge storms throttling ingest (query-performance.md:22-31).** Ingest is never coupled to merge
lag; back-pressure is explicit memory/disk 429, and merges are I/O rate-limited away from foreground
writes (engine/crates/xerj-storage/src/merge.rs:19-23) with forced merges that converge and quiesce
(index.rs:7873-7934). The open half — no closed-loop raising of the fixed 100 MB/s cap
(config.rs:613-616) when merges fall behind — is §4 item 4.

**Silent bulk partial failures (data-pipeline-issues.md:37-49).** Every bulk item carries a real
per-item status with genuine `_seq_no`/`_version`, and an all-items-rejected batch escalates the
top-level HTTP status to 429 with Retry-After so clients that never read item statuses still see the
failure (es_compat.rs:13103-13145; verified on both response paths, 13241-13247 and 13305-13312).

**Mapping explosion via cluster state (06-upgrades-and-migrations/mapping-explosion.md:9-30).**
Mappings live in per-index atomic-fsync'd files, not replicated cluster state (engine.rs:848-866;
index.rs:25199-25221); a 500-field cap (stricter than ES's 1,000) is enforced on the hot ingest
path and on explicit `put_mapping` adds (index.rs:17350-17395, 2996; config.rs:954-957). Caveats
that stay on the roadmap: the cap is global-only and over-limit fields drop silently (§4 item 7).

**Relational gaps (08-data-model-limitations/relational-gaps.md:10-36).** Honest scope: no
transactions or joins (the missing multi-index transaction primitive is a documented tracked
limitation, es_compat.rs:1933-1937). What exists is stronger than expected: nested queries with true
per-element semantics that need no `nested` mapping declaration because matching runs on `_source`
structure (engine/crates/xerj-query/src/parser.rs:3038-3069; index.rs:28375-28386), inner_hits
machinery (es_compat.rs:9366, 10613, 12448), and a Chunk field type carrying a parent-document
reference for RAG (engine/crates/xerj-common/src/types.rs:224-227).

**Upgrade-proofing the data format (version-upgrades.md:9-47).** The segment format is versioned,
checksummed, and section-based: unknown versions fail loudly with `UnsupportedVersion`
(engine/crates/xerj-storage/src/segment.rs:243-246), unknown sections are skipped by design
(segment.rs:89-91), the manifest has deliberate serde-default hygiene (segment.rs:308-322), and one
compatible format evolution has already shipped (tombstones v2, segment.rs:130-150). The gaps —
exact-match version check, no WAL version field, silent sidecar fallback, no upgrade fixture — are
blocker 6.

**Kubernetes fit (kubernetes-pain.md:13-23, 101-105).** Memory budgets auto-derive from the cgroup
limit; the disk flood-stage write block auto-clears (vs the 14-hour ES watermark story,
community-horror-stories.md:42-44); liveness is deliberately engine-free to avoid restart feedback
loops (engine/crates/xerj-api/src/native.rs:719-721); the Helm chart has probes, default resource
requests and limits, runAsNonRoot with capabilities dropped, and no privileged init containers or
sysctls (deploy/helm/xerj/templates/statefulset.yaml:21-74; deploy/helm/xerj/values.yaml:16-51,
81-89) — the exact defects of ES's archived chart. Its one disqualifying default is blocker 8.

**Monitoring without a second cluster (monitoring-overhead.md:9-28).** A built-in Prometheus
registry with 19 metric families (engine/crates/xerj-common/src/metrics.rs:34-96) at `/v1/metrics`,
plus a low-privilege metrics-only scrape token so Prometheus never holds the admin key
(config.rs:383-392), plus in-engine slow-query and audit logs. The breaker-state gap is blocker 9.

**Split-brain (04-scaling-and-shards/split-brain.md).** Structurally impossible in the shipped
single-node architecture; cluster mode is off by default, fail-closed (refuses to start without a
>=16-char shared secret), and honestly labelled experimental with a plaintext-transport warning
(config.rs:1163-1277; engine/crates/xerj-server/src/main.rs:1620-1634; engine/xerj.default.toml:371-393).

**Security defaults (engine side).** First run auto-generates a 0600 admin key and reuses it across
restarts — auth on by default with no ceremony (main.rs:500-548). The chart-side inversion is
blocker 8.

**Shipped in rc.12** (listed as shipped, not future work): `doc_values` honoured on the mapping —
index 1.553x smaller on the text-heavy benchmark corpus but only ~1.6% on source-code corpora
(corpus-dependent; never quote the single figure as universal), force-merge 2.33x faster; a
per-segment FTS reader cache (common full-text queries 2.4-2.9x); index-wide BM25 statistics so
scores no longer depend on machine CPU count (#188); an autoindex multibyte-abort fix; a tokio
blocking-pool deadlock fix.

---

## 3. GA blockers, ranked

Ranking is blast-radius times silence: silent data loss on the adoption path first, then
restart-integrity, then failure visibility, then honest defaults and honest numbers.

### 1. Cursor-based scroll — kill the silent 10k truncation on the ES export path (medium)

**Pain.** "Kafka connector delivering 10% with no error" — silent pipeline truncation
(user-feedback/07-query-and-performance/data-pipeline-issues.md:37-49); scroll is how every ES
export/migration tool pulls data (elasticdump, Logstash es input, reindex-from-remote).
**Today.** The scroll handler re-issues the query with `size=10000, from=0` and snapshots a fully
hydrated `Vec<Hit>` ("For scroll: fetch ALL docs by setting a large size",
engine/crates/xerj-api/src/es_compat.rs:16911-16965); continuation only pages that snapshot
(17216-17258), so document 10,001 onward is unreachable and the exhausted-snapshot empty page is the
universal "export complete" signal. The verifier found a second, more important site with the
identical hardcoded cap inline in `search_impl` (es_compat.rs:6589-6613); because 10,000 is
hardcoded, raising `index.max_result_window` does not lift it. Continuation pages also hardcode
`_version:1/_seq_no:0/_primary_term:1` (17225-17227) and report `total` = snapshot length,
contradicting page one. Contexts are count-bounded (500) but not byte-bounded — each can pin up to
10k hydrated hits (config.rs:1320-1356; the per-hit estimate is 1 KiB, index.rs:12628).
**Build.** Rewrite `ScrollContext` to store the `SearchRequest` plus a `search_after` cursor (last
sort key + seq tiebreak) instead of hydrated hits; each continuation re-executes through the
existing `SortTopK` after-cursor machinery (index.rs:30086-30094), which already makes page N cost
page 1. Pin snapshot visibility via the existing PIT max-visible-seq mechanism
(config.rs:1286-1291). Patch BOTH construction sites (es_compat.rs:16883 and the inline block at
6589-6613), fix the total-hits inconsistency and the hardcoded version fields. Files:
engine/crates/xerj-api/src/es_compat.rs, scroll context in engine/crates/xerj-engine.
**Reference.** quickwit quickwit/quickwit-search/src/scroll_context.rs:108-130, 180-235
(Apache-2.0) — scroll context stores the request plus lightweight partial hits, re-executes with
`search_after = last_hit` per batch, and encodes cursor state in the scroll_id itself.
**If shipped without.** Week-one migration users export 10,000 of N documents with zero errors and
conclude XERJ loses data. The ES bridge is the product's stated wedge; this poisons it at first
contact.

### 2. Persist management state across restart (medium)

**Pain.** "Upgrade = replace binary + restart" is the corpus response to the upgrade horror stories
(user-feedback/06-upgrades-and-migrations/version-upgrades.md:9-47) — the promise only holds if
restart is lossless.
**Today.** `Engine::new` restores index dirs, `es_mapping.json`, API keys, and aliases — nothing
else (engine/crates/xerj-engine/src/engine.rs:473-547, re-verified for this document). Index
templates, legacy templates, component templates, ILM policies, ingest pipelines, and data streams
live in plain DashMaps with no boot load (engine.rs:264-336). Concretely: a data stream degrades to
a bare alias after restart (create/rollover write only the DashMap, engine.rs:1439-1494 —
re-verified), `GET /_data_stream` 404s, the generation counter resets; and the API layer's
`engine.index_settings` map is never rehydrated, so `index.default_pipeline` silently stops being
applied (`resolve_effective_pipeline` reads only that map,
engine/crates/xerj-api/src/es_compat.rs:2208-2225, re-verified) even though the engine-side Index
reloads its own `settings.json` (index.rs:5568) — the two views diverge after every restart.
**Build.** One flush/load pair per store, following the existing
`flush_aliases`/`load_persisted_aliases` and api-keys pattern (engine.rs:953-1017): write
`<data_dir>/{data_streams,templates,component_templates,legacy_templates,ilm_policies,pipelines}.json`
via `write_file_atomic` (index.rs:25205-25221) at every mutation site, load in `Engine::new`;
rehydrate `engine.index_settings` per index at boot from each index's on-disk `settings.json`
(index.rs:25229-25233). No API changes. Files: engine/crates/xerj-engine/src/engine.rs.
**Reference.** quickwit
quickwit/quickwit-metastore/src/metastore/file_backed/file_backed_index/serialize.rs:29-60
(Apache-2.0) — version-tagged JSON metadata with upgrade-on-read.
**If shipped without.** A competent user sets up data stream + template + pipeline, restarts, and
finds the management plane amnesiac with ingest silently un-piped — directly contradicting the
single-binary-upgrade positioning.

### 3. Failed-index lifecycle: surface the reason, allow delete/retry without restart, expose fsck (medium)

**Pain.** Red indexes are "often quite difficult to figure out, often involving arcane JSON
commands you hope you get right" (user-feedback/01-operational-complexity/cluster-management.md:51).
**Today.** An index whose directory fails to open at boot is quarantined in `failed_indices` and the
server boots (engine.rs:495-515) — good — but the quarantined index is then unreachable by every
API: absent from listings, undeletable (`delete_index` consults only `self.indices`,
engine.rs:1135-1177), unretryable, its stored error string exposed by no endpoint. Native health
goes red forever. An fsck primitive already exists with no surface (`Index::fsck_segments`,
index.rs:11739-11752). The verifier resolved the audit's open question about `create_index` over a
quarantined name: the already-exists check consults only `self.indices` (engine.rs:821), so the
create flows to `IndexStore::open` on the same directory (index.rs:5344-5355) — persistent
corruption makes the create fail with the real underlying error (today's only path that surfaces the
reason), while a transient boot failure makes the create silently ADOPT the old directory with all
its segments while overwriting `schema.json` with the caller's schema (index.rs:5400-5403); in both
cases `failed_indices` is never cleared, so health stays red regardless.
**Build.** Let `delete_index` and a new retry-open admin op consume `failed_indices`; make
`create_index` refuse (or explicitly clear-and-adopt) a quarantined name; expose the failed list
with reasons in `/v1/cluster/health` and compat surfaces (`_cluster/health?level=indices`,
`_cat/indices` rows, `cluster_allocation_explain` returning the real stored error instead of "does
not exist", es_compat.rs:23684-23724); wire `fsck_segments` to an admin endpoint; add a failed-index
gauge to metrics.rs; regression-test the create-over-quarantined path. Files:
engine/crates/xerj-engine/src/engine.rs, engine/crates/xerj-api/src/es_compat.rs,
engine/crates/xerj-api/src/native.rs, engine/crates/xerj-common/src/metrics.rs.
**Reference.** qdrant src/main.rs:452-457 (Apache-2.0) — degraded state is loudly explained to the
operator with a docs link, never silent.
**If shipped without.** Any week-one disk glitch produces a permanently red node with no API-visible
cause and no API remedy — the "needs a specialist with shell access" experience XERJ exists to kill.

### 4. Readiness must not permanently remove a serving pod over one failed index (small)

**Pain.** Kubernetes pods pulled from rotation over partial failure (kubernetes-pain.md), silent
escalation (monitoring-overhead.md:19-22).
**Today.** Readiness 503s on any-index-failed red (native.rs:723-734) directly under a comment
claiming "at least one index is queryable" (native.rs:711-716) — the comment describes the right
behavior, the code implements the wrong one. The verifier strengthened this: because liveness is
deliberately engine-free (native.rs:719-721), the pod is never restarted AND never ready — it sits
dark in the Service forever, and `failed_indices` has no removal path, so there is no self-healing.
**Build.** Distinguish transient startup from steady-state partial failure; return ready when the
engine is serving; surface the failed set via health detail and the blocker-3 metrics. Document the
probe policy in deploy/helm/xerj/values.yaml comments. Files: engine/crates/xerj-api/src/native.rs.
**Reference.** qdrant src/main.rs:452-457 (Apache-2.0) — keeps serving in recovery mode with a
warning rather than going dark.
**If shipped without.** The first K8s user with a single corrupt index loses the whole node from the
load balancer permanently — a worse outcome than ES yellow, and a public "XERJ took down my service"
story.

### 5. All three health surfaces must tell the same truth (small)

**Pain.** "By the time alerts fire the cluster is degraded" (monitoring-overhead.md:19-22);
operators poll the ES-shaped endpoints.
**Today.** Native health is real (red on failed indices — native.rs:638-656, 744-764). The ES-compat
`_cluster/health` derives status only from stored replica settings and structurally cannot report
red (es_compat.rs:317-326, per-index breakdown yellow/green-only at 450). `_cat/health` actually
calls `engine.health()` and then discards the status, formatting the literal string "green"
(es_compat.rs:15834-15854, the fetch at 15835 and the literal at 15844). Monitoring that cannot
report failure is a silent fake by this repo's own stub-audit standard.
**Build.** `cat_health` uses the status it already fetched; `cluster_health_inner` folds
`engine.health()` red (failed indices) into status and the per-index breakdown. Regression test: a
quarantined index turns both compat endpoints red. Files: engine/crates/xerj-api/src/es_compat.rs.
**Reference.** ES `_cluster/health` semantics (AGPL/SSPL — APPROACH ONLY; wire behavior already
mirrored via the ES-YAML suite, no code needed).
**If shipped without.** Every ES-ecosystem monitor (curl scripts, Grafana ES datasources, uptime
checks) reports a healthy node while data is unavailable. The honest-claims rules say this cannot
ship as 1.0.0.

### 6. Sidecar/format version failures must be loud: no silent mapping loss (medium)

**Pain.** "After upgrade, data directory layout changes prevent downgrade... you cannot go back"
(version-upgrades.md:9-47). XERJ currently answers the downgrade pain with something worse than
refusing: silence.
**Today.** `Index::open` falls back to an EMPTY DYNAMIC mapping when `schema.json` fails to parse
(`load_schema(...).unwrap_or_else(|_| ManagedSchema::dynamic())`, index.rs:5578, re-verified) —
exactly what a downgrade to a binary lacking a newer `FieldType` variant produces, after which
dynamic inference silently re-types fields from first values. The code's own comment names the class
"a mapping-loss corruption" (index.rs:25199-25204, re-verified); `settings.json` has the same
`unwrap_or(Null)` pattern (index.rs:5568), and both loaders swallow all errors
(index.rs:25223-25233). The segment version check is exact (`!=`) with no supported range
(segment.rs:243-246, re-verified); the WAL header has no version field (wal.rs:10-14, 86-87); no
cross-version upgrade test fixture exists anywhere (audit retrieval found none — §8).
**Build.** `Index::open` fails into the existing `failed_indices` quarantine path (engine.rs:511-514)
on sidecar parse failure instead of defaulting; version-tag `schema.json`/`settings.json` via a
serde-tagged enum with per-version upgrades; change the segment check to a supported range before
format v2 ever ships; use the WAL header's reserved bytes as a format version; add a
golden-data-dir fixture test (current release's data dir opened by HEAD) — none exists today.
Files: engine/crates/xerj-engine/src/index.rs, engine/crates/xerj-storage/src/segment.rs,
engine/crates/xerj-storage/src/wal.rs, engine/crates/xerj-engine/src/engine.rs.
**Reference.** quickwit
quickwit/quickwit-metastore/src/metastore/file_backed/file_backed_index/serialize.rs:29-60
(Apache-2.0) — version-tagged metadata, upgrade-on-read.
**If shipped without.** A torn file or version skew silently erases an index's mapping and re-types
it from first values — silent data corruption on the exact path ("just swap the binary") the product
tells users is safe. ES refuses to start; XERJ opens wrong. [Note: this domain's verifier verdict
was not delivered; the load-bearing citations above were re-verified directly for this document.]

### 7. Retention/ILM: wire the promised TTL driver or retract the claims (small–medium)

**Pain.** "When ILM encounters an error, execution HALTS"; "ILM does not trigger rollover — user
waited 2 weeks" (user-feedback/06-upgrades-and-migrations/ilm-problems.md:9-48). The corpus's
XERJ.ai Response promises "TTL-based retention: configure retention_days, background purge handles
it."
**Today.** Neither ILM nor the promised TTL retention executes. ILM policy PUT/GET/DELETE
round-trips a DashMap that nothing reads (es_compat.rs:22720-22764, re-verified — returns
`acknowledged: true` and no executor exists); data-stream rollover is manual-only with conditions
ignored (es_compat.rs:22695-22710); `LogsConfig.retention_days` (default 90, config.rs:806-822) is
config plus an undriven library — `RetentionManager::apply` logs and returns a list, and the file
itself says "In production, this would be driven by a background tokio task"
(engine/crates/xerj-logs/src/retention.rs:101-134, re-verified). Accepting an ILM policy with 200
and never executing it is a silent fake by the repo's own stub-audit standard, reproducing the
exact "waited 2 weeks" pain on the adoption bridge.
**Build (minimum honest GA bar).** (a) A background task driving `RetentionManager::apply` on
time-partitioned logs indices, making the published retention_days claim true; (b) honesty for ILM:
either a minimal executor for the rollover/delete phases actually used by data streams, or an
explicit not-implemented response plus documentation — never a silent 200; (c) correct the
user-feedback Response sections if scope shrinks. Files: engine/crates/xerj-logs/src/retention.rs,
engine/crates/xerj-server/src/main.rs (task spawn beside `spawn_periodic_flusher`, main.rs:754),
engine/crates/xerj-api/src/es_compat.rs. [This item is synthesized from the fourth audit's findings;
that domain's verifier verdict was not delivered, and the sizing is this document's.]
**If shipped without.** ES users PUT lifecycle policies, get 200s, and their indices grow forever —
discovered weeks later as a disk incident, on the exact API surface XERJ claims as its bridge.

### 8. Flip the Helm chart to secure-by-default (small)

**Pain.** The K8s onboarding path is where ES's archived, production-unsafe chart burned users
(kubernetes-pain.md:101-105); insecure defaults are their own CVE class (09-security).
**Today.** deploy/helm/xerj/values.yaml:53-55 defaults `insecure: true` — auth AND TLS off for every
`helm install xerj` — inverting the engine's own auth-on default with auto-generated 0600 admin key
(config.rs:369-403; main.rs:500-548). The verifier added: the chart has no auth plumbing at all —
its comment tells production users to supply `tls.cert/key + auth.token` (values.yaml:54) but no
such values or templates exist; flipping the default works only because the engine auto-generates
`admin.key` onto the PVC. values.yaml:11 also asserts "xerj recovers in ~7 ms" — an unverified
number under the honest-claims rules.
**Build.** `insecure: false` by default with a documented dev override (`--set insecure=true`); a
NOTES.txt documenting key retrieval (`kubectl exec ... cat /data/admin.key`); delete or substantiate
the ~7 ms figure. Files: deploy/helm/xerj/values.yaml, deploy/helm/xerj/templates/.
**If shipped without.** Every default K8s install is an unauthenticated cluster-internal search
engine — the insecure-defaults class users already rage about, reproduced on day one.

### 9. Expose breaker state; document per-platform breaker coverage (small)

**Pain.** ES users monitor breaker state as reflex after the leak incidents ("counter grows
indefinitely, node becomes unusable", oom-incidents.md:39-43).
**Today.** XERJ 429s writes by default at the memtable budget / 95% RSS watermark, but the trip
state, budgets, and refusal counts are exposed by no API: `_nodes/stats` has no `breakers` block
(verified against the live file and instance — zero occurrences across es_compat.rs),
`GovernorSnapshot` is serialized but engine-internal (governor.rs:322-346), and Prometheus has one
coarse memory gauge (metrics.rs:224-226). [Verifier correction carried inline: the audit's "operator
has only tracing logs" overstated it — every breaker 429 body is self-describing, naming the tripped
budget, usage, limit, and the setting to raise (governor.rs:112-129). The operator is inconvenienced,
not blind. The verifier kept the item blocking for the ES-bridge positioning: ES monitoring tools
poll `_nodes/stats.breakers`, and an empty spot where the dashboard looks reads as broken
monitoring.] Separately, the verifier disputed the platform-coverage documentation into the GA bar:
on non-Linux, `current_rss_bytes` returns 0 so the RSS watermark can never trip
(governor.rs:586-601), total memory falls back to a flat 8 GiB so the auto memtable budget silently
becomes 2 GiB regardless of RAM (governor.rs:617-633), and disk stats return None so the flood-stage
block never engages (governor.rs:797-800) — while Windows is an rc.9 headline platform. Shipping the
"429 beats the OOM-killer" guarantee without stating it is Linux-only is a claims problem.
**Build.** Emit a `breakers` object (parent/memtable/query: limit, estimated, tripped counter) from
GovernorSnapshot in `nodes_stats` (es_compat.rs ~19787); add trip/refusal counters and budget gauges
to metrics.rs; include the full snapshot in native `/v1` stats; document per-platform breaker
coverage in the config reference. The probe implementations themselves stay post-GA (§4 item 9).
Files: engine/crates/xerj-api/src/es_compat.rs, engine/crates/xerj-common/src/metrics.rs, docs.
**Reference.** elasticsearch HierarchyCircuitBreakerService.java (AGPL — APPROACH ONLY: match the
`_nodes/stats` breakers JSON shape for wire compatibility; write our own implementation).
**If shipped without.** Default-on 429s with no observability read as mysterious write failures;
migrating users' dashboards silently lose their breaker signal, and the engine's strongest anti-OOM
feature is experienced as a regression.

### 10. Correct the settings-count claim and make the count drift-proof (small) — DONE 2026-08-09 (#207)

> Landed as described: the count test now serialises `Config::default()` and counts leaf
> keys (`journey_zero_config`), the measured total is **103**, and the 38/56/60/`<50`
> figures are corrected across config.rs, xerj-common/src/lib.rs, engine/README.md,
> xerj.default.toml and the feedback responses. Per-sub-config annotations are corrected
> too (limits 3 → 13, storage 5 → 10, cluster 4 → 5, embedding 4 → 19, merge 5 → 8,
> tls 3 → 4, auth 2 → 3). The adversarial pass counted 102; the measured figure is 103.

**Pain.** The "<50 configuration settings (ES has 3,000+)" figure is published in the shipped
feedback responses (user-feedback/01-operational-complexity/cluster-management.md:63).
**Was (before the fix).** config.rs simultaneously claimed 38 settings (lines 1-5 and 274-276), 56 fields (89-92),
and 60 (the count test, 1592-1617 — whose own comment two lines earlier says 50, and which sums
hardcoded integers referencing no struct); per-section doc comments are also stale (config.rs:49-86:
limits "3" vs actual 13, storage "5" vs 10, cluster "4" vs 5). The true count is 102 serde-visible
settings — verified field-by-field by the adversarial pass. [The audit marked this non-blocking; the
verifier disputed it to blocking under the repo's honest-claims rules — a published number off by
2-2.7x is exactly the class that has held rc cuts before, and the fix is trivial, which argues for
doing it before GA, not waiving it. This roadmap adopts the verifier's position.]
**Build.** Fix all inconsistent counts; replace the hand-written count test with introspection
(serialize `Config::default()` to `toml::Value`, count leaf keys) so the number can never drift;
sweep public docs and the feedback responses for the stale "<50"/"38". 103 vs 3,000+ is still the
winning story, told truthfully. [DONE 2026-08-09, #207 — the measured figure is 103.] Files: engine/crates/xerj-common/src/config.rs, docs, user-feedback
response sections.
**If shipped without.** A reviewer counts the TOML keys, finds 2.7x the claimed number, and every
other XERJ number takes the credibility hit.

---

## 4. Post-GA, ranked

1. **Retrieval quality for the AI-native story: #173, #174, and the audit's own retrieval defects
   (medium-large).** Autoindex fragments a 2-repo tree into 407 indices with 1.2% semantic_text
   coverage (#173); `_passage` ships but only the kNN executor populates it, so retrieval shows file
   heads instead of matching passages (#174); and this audit cycle produced direct dogfooding
   evidence of a third defect: mid-file tokens of very large files are unsearchable — in the
   37k-line index.rs, `try_charge`@18839, `acquire_search`@12583, and `check_query_alloc`@12631
   return zero body-search hits while tokens at lines 6019 and 30941 match, which nearly caused a
   false "not wired" audit finding — plus exact-identifier search requiring the full token
   (`resource_sampler` matches nothing; `spawn_resource_sampler` does). *Why it can wait:* none of
   it corrupts or loses user data. Why it should not wait long: retrieval IS the AI-native
   positioning, and these are the defects XERJ's own self-audit hit.
2. **Per-request durability override + published durability contract (small).** The engine's answer
   to Jepsen folklore is strong (§2) but stated only in config comments; durability is node-global.
   Honour ES per-index `translog.durability` semantics (APPROACH ONLY — the primitive already
   exists: `WalWriter::set_sync_mode`, wal.rs:463-465) and publish the three-mode contract in one
   table, always naming the default 100 ms power-loss window. *Why it can wait:* the default
   contract is already stronger than ES defaults; this is plumbing plus documentation.
3. **Score determinism: #191 (tied scores not broken deterministically) and #193 (min_score uses
   per-arm scores at size:0; scalar N drops ghosts).** Not audited or sized this cycle;
   classification provisional (§8). They belong early post-GA because #188 (index-wide BM25) shipped
   specifically to make scores machine-independent, and nondeterministic tie order undercuts that
   claim.
4. **Closed-loop merge I/O throttle with a segment-backlog signal (medium).** XERJ never throttles
   ingest on merge lag (the ES pain), but nothing raises the fixed 100 MB/s merge cap
   (config.rs:613-616) when flushes outpace merging, so sustained ingest can accumulate segments and
   degrade read latency instead. Reference: lucene
   core/src/java/org/apache/lucene/index/ConcurrentMergeScheduler.java:817-907 (Apache-2.0) —
   backlog detected -> rate *= 1.20 up to 10,240 MB/s, caught up -> /= 1.10 down to a 5 MB/s floor;
   and 609-644 (maybeStall). *Why it can wait:* the failure is gradual degradation, not loss — and
   segment accumulation under sustained load has not been measured yet (Wave 0 measures it; build
   only if the measurement says so).
5. **WAL-backed changes feed, `/_changes?since_seq=N` (large).** The loudest pipeline pain — "ES
   provides no recovery log or change stream" plus dual-write divergence
   (data-pipeline-issues.md:24-49) — and the #4 all-time Kibana enhancement ask (330 reactions,
   user-feedback/kibana/themes/top-asks.md). The substrate exists: globally monotonic seq_nos across
   WAL shards (wal.rs:337, 477) and merge-sorted replay (wal.rs:1237-1285); needs a
   subscriber-aware retention floor on `prune_verified` (wal.rs:1003-1029) with PIT-style expiry.
   *Why it can wait:* new feature, not a broken promise — but it converts the WAL into the CDC story
   the agent-sync audience most wants, so it leads the feature track.
6. **Byte-weighted query/agg admission (medium).** Count caps kill the worst class, but aggregation
   STATE bytes are unaccounted: `check_query_alloc` has exactly one caller (result-window,
   index.rs:12613-12634) and the governor doc's "terms-agg" label is unwired (governor.rs:155-169).
   Fold in: pin down whether `max_buckets` bounds the nested-agg bucket product globally or
   per-invocation (verifier's working hypothesis from aggs.rs:100-111: per-invocation, i.e. ES-7+
   MultiBucketConsumer semantics NOT matched — Wave 0 tests it), and bound the per-index
   `query_cache` if Wave 0 confirms it unbounded (index.rs:5171-5179 documents version-keyed
   invalidation with no size cap while sibling caches are capped, index.rs:5127-5158). Reference:
   quickwit quickwit/quickwit-search/src/search_permit_provider.rs:38-55, 86, 197-199, 443
   (Apache-2.0) — permits carry a pessimistic byte allocation, corrected after warmup. *Why it can
   wait:* the RSS watermark backstops the node today; the wrong requester pays, which is bad but
   bounded.
7. **Dynamic-inference and mapping ergonomics (medium).** XERJ reproduces the wrong-type
   auto-detection trap: type pinned by the FIRST value seen, JSON null infers Keyword forever
   (engine/crates/xerj-common/src/schema.rs:199-220, re-verified — `Null => FieldType::Keyword`),
   short identifier-like strings become Keyword while long ones become Text with no ES-style
   text+keyword multi-field (schema.rs:227-242); over-limit fields drop silently where ES rejects
   loudly (index.rs:17350-17368); the field cap is global-only (`index.mapping.total_fields.limit`
   unwired, index.rs:5518); data-stream rollover conditions are ignored; `_reindex` runs
   synchronously inside the HTTP request with a per-doc existence GET (es_compat.rs:17661-17827).
   *Why it can wait:* every piece has a workaround (explicit mappings, strict mode, manual
   rollover), and type changes already fail loudly with the ES-verbatim error (es_compat.rs:1771-1810).
8. **Vector residency accounting with quantized/mmap tiering (large).** The HNSW graph is
   documented entirely in-memory (engine/crates/xerj-vector/src/hnsw.rs:13) and its bytes are
   invisible to every governor category; the only fence (RSS watermark) can only stop writes, so a
   large graph pins the node at 95% with all ingest 429ing and nothing sheddable — the ES off-heap
   ANN cliff (gc-pauses.md:41-44) reproduced. First step per the reference-coding mandate: retrieve
   qdrant's mmap/quantized storage design from the xerj-vector corpus before writing code (not
   retrieved this cycle; no line citation offered). *Why it can wait:* it bites only when vectors
   approach ~45% of node RAM; SQ8 quantization and the exact-scan default are the interim relief.
9. **Cross-platform resource probes (medium).** Implement Windows
   (GetProcessMemoryInfo/GlobalMemoryStatusEx/GetDiskFreeSpaceEx) and macOS (mach task_info,
   statfs) behind the existing cfg seams (governor.rs:586-633, 797-800). The GA bar only documents
   the gap (blocker 9); this closes it. *Why it can wait:* the failure direction is conservative —
   small budgets and no false trips — but a Windows node runs with materially weaker OOM protection
   until this lands.
10. **Cluster transport bind failure fails closed (small, opportunistic).** With
    `cluster.enabled=true`, a bind failure logs and continues serving standalone
    (main.rs:1664-1683) — the precondition for divergence the day replication ships. Mirror the
    abort already used for a missing auth secret (main.rs:1620-1634). Reference: qdrant
    src/main.rs:611-625 (Apache-2.0) — `Consensus::run(...).expect(...)`: distributed mode that
    cannot start consensus aborts, never serves. *Why it can wait:* the data plane is provably
    unwired today (§5), so the fallback misleads but cannot diverge. Fix while it is one line.
11. **Per-index/tenant resource quotas (large, M2+).** All limits are process-global
    (config.rs:944-1055); noisy-neighbor economics remain (multi-tenancy.md:9-12). Design should
    land before multi-node does — per-node quotas are the building block of cluster fairness.
    Reference when picked up: clickhouse per-user resource quotas (Apache-2.0). *Why it can wait:*
    the M1 posture is explicitly single-tenant, and docs keep saying so.

---

## 5. Explicitly NOT doing, and why

- **Numeric doc-values compression codecs.** Refuted by measurement: numeric codecs address 1.5% of
  the doc-values sidecar, and the prescribed encoding measured worse than plain
  frame-of-reference. The rc.12 doc-values win (1.553x on the text-heavy benchmark; ~1.6% on
  source-code corpora) came from honouring the mapping, not from codecs. No further codec work goes
  on this roadmap.
- **Positions opt-out as a storage headline.** Measured at 4-9% of the store, not the implied 14
  points. It may someday exist as a niche option; it will not be built as, or sold as, a disk-win
  lever.
- **Kibana feature parity.** The 1,414-issue ranked backlog (user-feedback/kibana/themes/top-asks.md)
  is another company's decade. Decision and costs in §6; the standing answer is integrate + minimal,
  never parity.
- **Multi-node HA in 1.0.0.** The cluster data plane is implemented and tested inside xerj-cluster
  but deliberately unwired: `WalReplicator` is never referenced outside the crate, `route_write`'s
  only caller is a test whose comment says the Raft commit handler "will use [it] in M5.3" (future
  tense), and the engine runs node_id "local" with a 1-shard router (engine.rs:462-469 — re-verified
  at 463-469; engine/crates/xerj-engine/tests/shard_router_write_path.rs:11-19, 50-56). GA is
  single-node, split-brain-impossible by construction, with cluster mode fail-closed and labelled
  experimental (xerj.default.toml:371-393). Rushing replication into GA would trade a true claim
  ("no split-brain") for an untested one.
- **Full ES ILM parity.** The corpus documents ILM itself as the pain — "execution HALTS", rollover
  regex silently fails (ilm-problems.md:9-48). XERJ's answer is TTL simplicity plus explicit
  rollover (blocker 7), not a reimplementation of the phase machine whose complexity caused the
  complaints.
- **Transactions, joins, referential integrity.** Documented tracked limitation
  (es_compat.rs:1933-1937). The answers are: per-element nested queries with no mapping prerequisite
  (index.rs:28375-28386), local `_reindex`, an upstream source-of-truth pattern, and — post-GA — the
  changes feed (§4 item 5) that makes continuous upsert cheap. Building a relational engine inside a
  search engine is how the complexity being escaped got built.
- **TB-scale claims.** No verified run exists; the honest-claims rules forbid the claim, so the
  roadmap does not chase the number for GA.
- **Calling the default embedder "neural".** The default is lexical feature-hashing. Only
  `--embed-mode neural` actually running may be described as neural, ever.

---

## 6. The console question

1,414 ranked Kibana enhancement asks exist (user-feedback/kibana/themes/top-asks.md), plus distilled
pains and design inputs. The decision, stated plainly: **XERJ integrates first, keeps its own
console minimal, and will never pursue Kibana parity.**

- **Replace (declined, permanently).** Rebuilding Kibana means adopting a backlog another company
  accumulated over a decade with a dedicated org. Effort is unbounded and every hour of it starves
  the engine — the thing users actually leave ES over. The feedback corpus itself shows the console
  is where ES pain reports concentrate on UX, not where the architectural pain lives.
- **Ship minimal (the committed path).** A console API crate already exists
  (engine/crates/xerj-console-api). Its GA scope fence: explain THIS node — health and the failed-index
  list with reasons (blocker 3), breaker/governor state (blocker 9), slow-query and audit logs,
  mapping browser, query scratchpad. Not dashboards, not alerting, not canvas/maps/ML. The cost of
  minimal is permanent ownership of every widget shipped; the fence is "explains this node," never
  "explores your data."
- **Integrate (the leverage path).** Grafana's ES datasource and every curl-based monitor work
  exactly as well as the ES-compat endpoints are honest — which is precisely why blockers 5 and 9
  (health truth, breakers block) are on the GA bar: they are the integration story. Whether Kibana
  itself can connect to XERJ has never been tested (§8); testing it is cheap and worth a Wave 0
  hour, but XERJ must not promise it untested.
- **API-shaped console asks get engine answers.** The #4 all-time ask, a Changes API (330
  reactions), is the post-GA changes feed (§4 item 5) — an engine feature, not a UI feature. Asks
  that are really about data access land in the engine roadmap; only asks about operating THIS node
  land in the console.

---

## 7. Sequencing

**Standing gates, applied at every wave boundary:**
- Conformance: ES-YAML suite at **0 failed**. The pass count is expected to GROW as blockers add
  regression tests (baseline today: 1365 passed / 0 failed / 3 skipped) — the gate is
  `failures == 0`, not an exact pass count.
- Performance: `demo/playbooks/rc12/measure_rc12.sh` against the Wave 0 baseline; reads and ingest
  within noise. Corpus-dependent results reported as ranges per the honest-claims rules.
- Builds stay scoped (`cargo build --release -j 32 -p <crate>`), never workspace-wide.

**Wave 0 — verification (days).** Capture the perf baseline. Write the tests that decide
classifications before code moves: create-over-quarantined regression (blocker 3);
quarantined-index-turns-compat-red (will fail until blocker 5); scroll-export-past-10k (will fail
until blocker 1); restart persistence for data streams/templates/pipelines/default_pipeline (will
fail until blocker 2); nested `max_buckets` product semantics (decides part of §4 item 6);
Read-confirm whether the per-index `query_cache` has any eviction path (§4 item 6); measure segment
accumulation under sustained multi-shard ingest (gates §4 item 4); try connecting Kibana to the
compat surface (§6, one hour, no promise).

**Wave 1 — the five smalls, in parallel.** Blockers 4 (native.rs), 5 (es_compat.rs), 8 (helm), 9
(es_compat.rs + metrics.rs), 10 (config.rs + docs). Collision note: 5 and 9 both edit es_compat.rs
in the health/nodes_stats region — one branch or sequenced merges. Helm gate addition: after
blocker 8, `helm install` smoke test must boot auth-on and the NOTES.txt key-retrieval instructions
must work as written.

**Wave 2 — the mediums, coordinated.** Blockers 2, 3, and 6 all touch the engine.rs boot path —
land in order 2 -> 3 -> 6 on one integration branch (worktree pattern). Blocker 1 (scroll) is
independent and can run in parallel. Blocker 7 lands after 2 (policies must persist before anything
executes them).

**GA cut** when: all ten closed, both gates green at the final boundary, docs corrections published
(settings count, per-platform breaker coverage, probe policy), and the out-of-scope-here security
audit pass has run (§8).

**Post-GA order** as ranked in §4, with two hard gates inside it: item 4 (merge throttle) builds
only if Wave 0's segment-accumulation measurement says so, and item 8 (vector residency) starts with
the mandated qdrant design retrieval, not with code.

---

## 8. What we could not determine

**Coverage of this roadmap.** This synthesis received four adversarially verified domain audits
(operations/cluster/K8s; memory/OOM; query and write performance with durability; data
model/mappings/upgrades), drawing on feedback categories 01, 03, 04, 06, 07, 08, and
02-infrastructure-costs. No audit was delivered for: 02 (pricing broadly), 05 (licensing/trust),
09 (security), 10 (documentation/UX), 11 (AI and vector search), 12 (log analytics), 13 (vendor),
14 (ecosystem), 15 (clustering-durability expectations), or the kibana/ console corpus beyond the
§6 decision. **The ten-blocker count is therefore a floor.** Security is the sharpest gap: the
repo's own backlog carries deferred TLS/auth/symlink items, and no GA tag should be cut without an
audited security pass.

**The fourth audit's verdict is missing.** Domains 1-3 were adversarially verified with every
citation opened; the data-model/upgrades audit arrived without its verdict. For this document, its
nine load-bearing citations were re-verified directly, and all confirmed as cited:
engine.rs:473-547 (boot restores only es_mapping/api_keys/aliases) and 1439-1494 (data streams are
DashMap-only), index.rs:5568/5578 (silent sidecar fallbacks) and 25199-25233 ("mapping-loss
corruption" comment; loaders swallow all errors), es_compat.rs:2171-2225 (settings/pipeline read
only the unrehydrated map) and 22720-22764 (ILM CRUD with no executor), retention.rs:101-134 (no
driver), segment.rs:243-246 (exact version check), schema.rs:199-242 (null -> Keyword inference).
Findings from that audit not re-verified here carry that caveat.

**Undetermined facts, explicitly:**
- Whether a XERJ index survives an actual cross-version binary swap: no golden-data-dir upgrade
  fixture exists anywhere (audit retrieval found none), and with only format v1 in existence the
  forward-compat discipline is design intent, not verified behavior. Blocker 6 adds the fixture.
- Whether Kibana proper can connect to XERJ's compat surface. Untested. Wave 0 tries it.
- Whether XERJ pays an ES-style global-ordinals warm-up spike on first aggregation after a flush:
  the doc-values fast path exists (index.rs:13538-13539) but its structure-build timing relative to
  segment publication was not audited.
- Segment accumulation under sustained ingest: the absence of merge/ingest coupling is established;
  actual accumulation was never measured. Wave 0 measures it.
- Whether the per-index `query_cache` (index.rs:5171-5179) has any eviction path: its documentation
  describes none and sibling caches are explicitly capped, but the mid-file search limitation below
  means an unseen path may exist. Wave 0 Read-confirms.
- Whether `max_buckets` bounds the nested-aggregation bucket product globally or per-invocation:
  verifier's working hypothesis is per-invocation (aggs.rs:100-111), meaning ES-7+
  MultiBucketConsumer semantics are NOT matched. Wave 0 tests it.
- Merge/compaction peak working-set bytes: the ingest ledger reserves Merge* categories as unwired
  vocabulary (engine/crates/xerj-engine/src/ingest_memory.rs:98-124); no finding evaluated whether
  merge memory is bounded by any budget.
- The `<500MB RSS for 1M docs` target: not verified this cycle. The empty-install ~80.6 MiB RSS is
  one measurement on one Linux host. Separately measured on the long-running corpora instance:
  21,568,585,728 bytes RSS across 9,378 open indices — PROJECTED ~2.3 MB/index average
  (21,568,585,728 / 9,378) — so per-index overhead at high index counts is real and interacts with
  #173.
- Effort for #191 and #193: not sized or audited this cycle; their §4 ranking is provisional.

**Where XERJ's own retrieval failed its own audit (dogfooding evidence).** The audits ran under a
retrieval-only rule against the selfaudit index — the strongest possible dogfood — and the record
belongs here:
- Mid-file tokens of very large files are silently unsearchable: in the 37k-line index.rs,
  `try_charge`@18839, `acquire_search`@12583, and `check_query_alloc`@12631 returned zero body-search
  hits while tokens at lines 6019 and 30941 matched. This nearly produced a false "governor
  query-side unwired" finding. Rule adopted mid-audit: absence-of-hits is not absence-of-code for
  big files. (Likely body-chunking; adjacent to #173/#174. Now §4 item 1.)
- Whole-file granularity plus #174's file-head previews meant every hit cost a follow-up Read, and
  the two giant files (index.rs, es_compat.rs) dominated almost every conceptual query. The
  `symbols` field (name/kind/line) was what made those files navigable at all — it turned every hit
  into a table of contents, and phrase queries on identifiers doubled as reliable absence proofs on
  the closed corpus (`WalReplicator` in 4 files all inside xerj-cluster; `failed_indices` in exactly
  1 file). That absence-proof technique is how the unwired cluster data plane and the API-invisible
  quarantine were established.
- Exact-identifier search needs the full token: `resource_sampler` returned nothing while
  `spawn_resource_sampler` matched — no substring match on identifiers.
- Corpus scope gaps: selfaudit covered engine/crates only, so the Helm chart, xerj.default.toml,
  and docs/ were invisible to retrieval (deploy/ was found by directory listing); one selfaudit copy
  of es_compat.rs lagged recent commits (the breakers absence was re-verified against the live
  file); and the xerj-columnar corpus was not indexed on the audit instance, so the intended
  ClickHouse MemoryTracker reference for byte-weighted admission is missing rather than faked.
- Peer-corpus retrieval was mixed: quickwit scroll/permit files and the qdrant consensus behavior
  were first-page hits; one boot-corruption query returned only AGPL Elasticsearch files plus an
  unrelated CSV test (irrelevant hits, disclosed, fell back to normal work); one query top-ranked an
  AGPL file that the licence label in the output made easy to skip.

The verdict on the dogfood cuts both ways and both halves are true: retrieval located every
subsystem and proved every absence this roadmap relies on, and its specific failures are now
themselves items on this roadmap.
