# XERJ Roadmap

This roadmap tracks capabilities that are **planned but not yet fully implemented**, so the project's public claims stay honest about what ships today versus what is coming. Status is verified against the actual code and by real API requests to the release binary, not aspirational.

Last reviewed: 2026-08-18 (against `v1.0.0-rc.18` and `main`). Statuses trace to issues, merged PRs, the CHANGELOG, and the conformance suite; items carried forward from the 2026-07-12 review without fresh live verification are marked as such. This review line is machine-checked: `docs_capability_lists` fails the build if a release is cut without re-reviewing this file (issue #298).

## Follow the roadmap

- **This file** is authoritative. If any other surface disagrees with it, this file wins — and that disagreement is a bug worth an issue.
- **[Milestones](https://github.com/xerj-org/xerj/milestones)** — the release-by-release view. Every open issue is triaged onto a milestone; the next RC's milestone is the short-term roadmap.
- **[Project board](https://github.com/users/xerj-org/projects/1)** — live status of every open item.
- **[Pinned issue #298](https://github.com/xerj-org/xerj/issues/298)** — the standing pointer, including how releases are cut and how to influence priorities.

## Shipping today (for context)

These are implemented and exercised by real API requests / the test suite / benchmarks:

- Elasticsearch REST wire compatibility (1,366 / 1,369 ES-YAML conformance cases; the gate on every change is **0 failed**, and the case count grows as cases are added — read the current number off CI, not off this file).
- Full-text search (BM25) and **<!-- generated:query-type-count -->50<!-- /generated:query-type-count --> query types**. Neither the list nor the number is maintained by hand here: the list is generated from `xerj_query::parser::SUPPORTED_QUERY_TYPES`, printed in full in [engine/README.md](./engine/README.md#query-types-supported) and [llms-full.txt](https://xerj.org/llms-full.txt), and pinned to `parse_query`'s dispatch table by `parser::tests::dispatch_table_matches_capability_manifest`; the number above sits in a machine-checked region pinned to that constant's length by `docs_capability_lists::published_capability_counts_match_the_constants`. A further <!-- generated:rejected-query-type-count -->2<!-- /generated:rejected-query-type-count --> keys — `has_child` and `has_parent` — are recognised and **rejected with a 400**, and are listed as such in the same places (issue #211).
  **Honest caveat, unchanged:** that count is the *dispatch* surface — every name on it parses, plans and executes, which is not a claim that every one is semantically faithful to ES. The known divergences are enumerated under *Known partials* below, and the ES-YAML conformance suite is the measured answer.
- **Aggregations: <!-- generated:agg-type-count -->62<!-- /generated:agg-type-count --> types**, likewise generated from `xerj_engine::aggs::SUPPORTED_AGG_TYPES`, printed in [engine/README.md](./engine/README.md#aggregation-types-supported) and [llms-full.txt](https://xerj.org/llms-full.txt), and pinned by the same count test. This includes the full **pipeline family**. `weighted_avg` is **not** in `SUPPORTED_AGG_TYPES` — see *Known partials*.
  **Exactness, precisely.** No probabilistic sketch sits in the metric path: `cardinality` is a true distinct count rather than an HLL estimate, and `terms` `doc_count` is precise. Two deliberate exceptions, stated the same way in `engine/README.md` and `llms-full.txt`: (1) the **sampling family** is a sample by definition — `run_sampler` sorts the matched documents by `_score` and keeps the first `shard_size` (default **200**), so every sub-aggregation under `sampler`, `random_sampler` or `diversified_sampler` is computed over that slice rather than the whole match set, `diversified_sampler` additionally caps documents per `field` value, and `random_sampler` shares the `sampler` implementation and **ignores ES's `probability`** (an accepted-and-ignored input, #204); (2) `percentiles` with the `hdr` option returns HdrHistogram-quantized values, deliberately, so ES's own outputs reproduce — the default `tdigest` path sorts every value and interpolates instead.
- **Dense-vector kNN** (`knn` query and ES 8.x top-level `knn`): unfiltered kNN on a full-precision cosine field (≥1,024 docs) is served by a **persisted HNSW graph with exact rescoring** — measured recall@10 1.00 on the official bench query, 100-probe mean 0.976 (ES 8.13.4 same protocol: 0.937); filtered/nested kNN, non-cosine similarity, SQ8 fields, and small indexes run the exact brute-force scan (cosine mapped to `(1+cos)/2`).
- **Hybrid search** — BM25 + kNN combined in a single request via the `hybrid` **query type** with `rrf|linear` fusion, verified live. (`fusion: "learned"` is parsed and **rejected** with a 400 naming the supported values — it is not implemented.) (The ES-native top-level `{query, knn}` body does **not** fuse — see *Known partials*.)
- **Zero-config folder onboarding** — `xerj autoindex <folder>` sniffs files, infers datasets, and creates one index per dataset: tree-sitter AST extraction for 34 languages (symbols, defs, line numbers — the [#295](https://github.com/xerj-org/xerj/issues/295) expansion; still open: Clojure and source-SQL wait on usable grammar crates, Nim/Crystal have none, fixed-form Fortran is deliberately unclaimed), CSV/JSON/JSONL/XML/YAML/SQLite/PDF/DOCX/HTML/log formats, `.gitignore`/`.xerjignore` support, incremental re-runs, and a machine-parseable progress stream.
- **Agent-memory REST API** (`/_memory/*`), **second-brain knowledge graph** (`/_graph`), **anomaly detection** (`_ml` with continuous datafeeds), **auto-embed on ingest** (default embedder is deterministic **lexical** feature-hashing — never described as neural; `--embed-mode neural` runs the in-binary BERT encoder, `--embed-mode proxy` an external endpoint).
- **Columnar storage** — the ZBS2 columnar block with 9 domain-aware encodings, ZSTD/LZ4 codecs, and SQ8 vector quantization, wired into the segment write path.
- Bulk / scroll / delete-by-query, aliases, index templates, **executed** index-lifecycle policies (ISM-modeled, `_ilm/*` + `_plugins/_ism/*`, since rc.15), `_cat/*`, `_cluster/health`, `_count` / `_msearch` / `_mget`, `_update` / `_update_by_query` — all live-verified.
- **A single native binary**, statically linked, no JVM, sub-second cold start.

The release-by-release record of how all of this landed (rc.1 through rc.18) is [CHANGELOG.md](./CHANGELOG.md) — this file no longer duplicates it.

## Next release — [v1.0.0-rc.19](https://github.com/xerj-org/xerj/milestones)

rc.18 was cut on 2026-08-18 — its full contents are the [CHANGELOG.md](./CHANGELOG.md)
entry, not this file. Two of its fixes are on the paths a user follows to obtain and
install XERJ: the air-gapped recipe extracted and installed on a bad digest
([#441](https://github.com/xerj-org/xerj/pull/441)), and the install page's `sha256sum -c`
step verified whatever filenames the `.sha256` listed without ever hashing the archive
([#444](https://github.com/xerj-org/xerj/issues/444)). It also carries the `--follow-symlinks`
escape fix ([#438](https://github.com/xerj-org/xerj/issues/438)), which **indexes less than
rc.17 for some setups** — see the CHANGELOG entry before upgrading.

rc.17 was cut on 2026-08-15 — 105 commits, and every
PR that was in flight at the rc.16 review had landed.

Items it retired from this roadmap:

- The **#204 fail-closed/fail-loud sweep** ([#258](https://github.com/xerj-org/xerj/pull/258))
  — the item rc.16 excluded on a failing check. It merged after four adversarial review
  rounds; the ES-compat conformance regression it carried is fixed and the gate is green.
- **First-class Unity project indexing**, relanded from community PR
  [#274](https://github.com/xerj-org/xerj/pull/274) by **@gonchar** as
  [#378](https://github.com/xerj-org/xerj/pull/378). Review of the reland found and fixed a
  global regression that silently junked CJK, Cyrillic, Greek, Hebrew and Arabic documents
  over ~4 KB.
- **`dense_vector` no longer builds a term dictionary no query path can read**
  ([#356](https://github.com/xerj-org/xerj/pull/356), closed
  [#328](https://github.com/xerj-org/xerj/issues/328)) — measured 54,068,549 B → 14,251,975 B
  (−73.6%) on a 5,000-doc × 128-dim corpus.
- **The single-node WAL tap** ([#322](https://github.com/xerj-org/xerj/pull/322), closed
  [#320](https://github.com/xerj-org/xerj/issues/320)) — the first path that pushes data out
  of the engine.
- The `fields` API deep-copy ([#311](https://github.com/xerj-org/xerj/issues/311)) and the
  `--no-graph` durable path ([#294](https://github.com/xerj-org/xerj/issues/294)) are closed.

**Open defects carried into rc.19.** This is a hand-picked shortlist, not the milestone and
not a filter you can reapply: it is what a reader evaluating XERJ would most want to know,
grouped by theme, and it includes performance and CI items that change no answer at all. The
[rc.19 milestone](https://github.com/xerj-org/xerj/milestones) is authoritative and currently
holds 26 open issues; if the two disagree about whether something is open, the milestone
wins. #469 and #450 are open but unmilestoned, listed because they are real rather
than because a milestone says so. Re-reviewed at the rc.18 cut: every item below was checked
against its issue state, and the ones rc.18 closed were removed rather than carried. Most
were found by dogfooding the engine against its own reference corpora, and each carries a
measured repro:

- **Ranking, partially fixed in rc.18.** `_score` is derived from the returned page, so it
  changes with `size` and ranking is not stable under pagination
  ([#361](https://github.com/xerj-org/xerj/issues/361)). rc.18 fixed the `filter`/`must_not`
  case with a `term`-shaped child on pages served entirely by the segment FTS path;
  `filter: [{match_all}]` / `filter: [{exists}]` and any page carrying memtable or
  stored-scan hits are unchanged, so the issue stays open.
- **Silently answering a different question.** `match` on a `semantic_text` field runs BM25
  rather than kNN ([#363](https://github.com/xerj-org/xerj/issues/363)); a refused key
  suppresses field evolution for up to 100 documents
  ([#382](https://github.com/xerj-org/xerj/issues/382)).
- **autoindex robustness.** Reconciliation aborts a whole run on the project's own
  reference corpora ([#367](https://github.com/xerj-org/xerj/issues/367)); nothing bounds
  what a magic-less binary costs ([#381](https://github.com/xerj-org/xerj/issues/381)).
- **Performance.** The neural embedder runs ~15 docs/s on short strings
  ([#366](https://github.com/xerj-org/xerj/issues/366)); nested term aggregations
  materialise all sub-buckets ([#375](https://github.com/xerj-org/xerj/issues/375)).
- **Carried forward.** The `fields` API omitting embedding companions
  ([#310](https://github.com/xerj-org/xerj/issues/310)) and the dynamic-mapping field-budget
  overshoot ([#312](https://github.com/xerj-org/xerj/issues/312)).
- **CI can only see what it is configured to run.** The Rust-1.92 clippy lints and the
  Painless stack overflow ([#353](https://github.com/xerj-org/xerj/issues/353)) came from
  this shape — a gate green only because of how it was invoked. The rc.18 cut produced
  another instance: the ROADMAP review gate is satisfied by the version string in the review
  line, so bumping the date passed it while this very section still listed six issues rc.18
  had closed. The gate needs to check the statuses, not the header.
- **Startup still announces what it has not got.** The first-launch console setup link is
  printed from the configured ES-compat port before any listener is bound, so it can name a
  port this process does not hold ([#469](https://github.com/xerj-org/xerj/issues/469)) —
  the same class rc.18 closed for the banner itself (#465).
- **ES-compat surface.** `_delete_by_query` / `_update_by_query` through a multi-index alias
  touch only the first member and report success
  ([#450](https://github.com/xerj-org/xerj/issues/450)); scroll through such an alias reports
  the alias as `_index` ([#433](https://github.com/xerj-org/xerj/issues/433)); the scroll
  snapshot cap is compared against the summed total by `/{index}/_search?scroll=` but applied
  per index by `/{index}/_search_scroll`, so a multi-index scroll that the first route refuses
  is accepted by the second. Both fail with the same 400 when they do refuse — the divergence
  is which total the ceiling is measured against, and it is permissive rather than lossy
  ([#405](https://github.com/xerj-org/xerj/issues/405)).
- **Documents that do not come back the way they went in.** `POST /_bulk` drops explicit
  `null` fields from `_source`, and whether it drops them depends on request size
  ([#415](https://github.com/xerj-org/xerj/issues/415)). #405 and #415 are cited as open by
  the rc.18 notes as well; #450 and #433 appear only here, which is the point of listing them.
- **Autoindex.** Catalog IDs are global, so two corpora sharing one byte-identical file
  collide ([#416](https://github.com/xerj-org/xerj/issues/416)); and widening an exclusion
  rule wedges the default graph path — the first rerun exits 1 and needs a documented
  recovery ([#439](https://github.com/xerj-org/xerj/issues/439)).

- **Also open, and in the same wrong-answer class**, listed by number rather than written out
  so the shortlist above stays readable: a `semantic`/`knn` clause nested in a `bool` is
  silently dropped ([#395](https://github.com/xerj-org/xerj/issues/395)); a single-clause
  `bool.must` changes `_score` and ranking versus the bare query
  ([#399](https://github.com/xerj-org/xerj/issues/399)); `match`/`multi_match` on a `keyword`
  field is mapping-aware only after flush ([#354](https://github.com/xerj-org/xerj/issues/354));
  term-level matching has two implementations and only one has a schema
  ([#423](https://github.com/xerj-org/xerj/issues/423), which consolidates eight others);
  `sort` on an unresolvable field ([#437](https://github.com/xerj-org/xerj/issues/437));
  scroll continuation pages never emit `_seq_no`/`_version`
  ([#428](https://github.com/xerj-org/xerj/issues/428)); switching `embedding.mode` from
  lexical to neural on an existing index is unguarded
  ([#434](https://github.com/xerj-org/xerj/issues/434)); and `%PDF-` is an unqualified
  printable magic, the residual of the fix rc.18 ships for `GIF8`/`BM`
  ([#403](https://github.com/xerj-org/xerj/issues/403)).

In flight at this cut: #460 (this release) and #477 ready, and #431/#446/#449/#453/#458 as drafts. #473 landed before the cut and ships in rc.18. Note #477 proposes replacing the flat 8 GiB default this release ships with a stepped 8/16/32 GiB cap chosen by machine RAM.

## The road to [v1.0.0 GA](https://github.com/xerj-org/xerj/milestone/2)

The 1.0 bar: **every public claim verified against the release binary, and every input either honoured or refused loudly.** The gate list, each item an issue:

- **Close the accepted-and-ignored class** (the [#204](https://github.com/xerj-org/xerj/issues/204) umbrella closed once its members carried their own tracking; PR [#258](https://github.com/xerj-org/xerj/pull/258) carried one pass of the sweep and is merged). Known members still open: `nested` `score_mode` parsed-then-ignored and `inner_hits` unparsed; `random_sampler`'s ignored `probability`; `weighted_avg` returning HTTP 200 with an error buried in the aggregations body instead of a 400 (the 400 is part of #258).
- **Security hardening backlog** — cargo-audit and fuzzing landed in CI with rc.16 ([#207](https://github.com/xerj-org/xerj/issues/207) closed); the deferred TLS/auth/symlink hardening items from the Phase-2 security backlog remain.
- **The mixed read-under-write p99 gap** — the 4 benchmark losses out of 85 measured comparisons, all the same root cause (reads landing on the live memtable under writer pressure). Written up in [`demo/playbooks/MIXED_READ_UNDER_WRITE_FINDING_2026-07-08.md`](./demo/playbooks/MIXED_READ_UNDER_WRITE_FINDING_2026-07-08.md); the candidate fix is a visibility/parity-mode design decision, not a micro-optimisation, and it stays on the GA gate until fixed or explicitly descoped with the benchmark loss kept public.
- **Ship-or-descope every entry in *Known partials* below.** GA does not ship with a "partial" section that reads like a feature list.

## Beyond 1.0 — themes

- **AST language expansion** — 25 further tree-sitter grammars, tiered by demand, one PR per
  tier. [#295](https://github.com/xerj-org/xerj/issues/295) delivered the expansion to 34
  languages and is closed; the remaining tiers have no tracking issue yet, so this theme is
  a plan rather than a commitment until one exists. Tier 1 (Kotlin, Swift, Scala, Dart, Lua, Perl, R, Julia, Haskell, Elixir) may land earlier in an RC if the grammar/ABI checks prove out.
- **Distributed clustering maturity** — embedded Raft handles cluster metadata today, but the default run is **single-node**; multi-node sharding/replication hardening is a post-GA track, and XERJ does not claim multi-node production readiness until it is measured.
- **Neural embedder ergonomics** — share one loaded model across indices (today each index lazily holds its own `NeuralHandle`), optional pre-warm at startup, a larger default model option.
- **Log-analytics data path** — the dedicated `xerj-logs` columnar module is still not invoked from non-test engine/server code; log-shaped analytics run through ZBS2 + the generic aggregation suite. Wire it or remove it.
- **Broader aggregation families** — geo/IP/nested/join coverage beyond the current surface; the conformance suite is the measure.

## Known partials

Honesty section: things that resolve without an error but do not implement full ES semantics. Each must be shipped or explicitly descoped before GA.

Re-verified against `main` 2026-08-11:

- **`weighted_avg`** — not in `SUPPORTED_AGG_TYPES`; still returns HTTP 200 with an embedded error instead of executing or returning 400 (the 400 is part of the #258 sweep).
- **`has_child` / `has_parent`** — recognised and rejected with a 400 (fail-loud by design until real parent-child join semantics exist; `REJECTED_QUERY_TYPES` in `parser.rs`).

Carried forward from the 2026-07-12 review, not re-verified live since:

- **`nested`** — matching is real and per-element (`test_nested_query`), but ES's separate nested-document indexing is missing: `score_mode` is parsed and ignored, `inner_hits` is not parsed (#204 members, above).
- **`span_term` / `span_or` / `span_not`** — return 0 hits **standalone**, while composite span queries (`span_near` / `span_first` / `span_containing`) using the same clauses return correct hits.
- **`type`** — mapped to `MatchAll`.
- **`combined_fields`** — mapped to `multi_match cross_fields`; scoring is not exact. `rank_feature` passes through on plain fields (no `rank_feature` field type).
- **ES-native top-level `{query, knn}`** — does not union the kNN hits; one-request BM25+kNN fusion works only through the explicit `hybrid` query type.

---

Found something claimed but not working? That is a bug in our docs or our code — please [open an issue](https://github.com/xerj-org/xerj/issues). We would rather ship an honest roadmap than an overstated feature list.
