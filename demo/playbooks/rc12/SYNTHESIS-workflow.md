Confirmed — `body` is a plain `text` field and it is 95.87% of the doc-values sidecar. I have everything I need.

# XERJ v1.0.0-rc.12 — engineering plan

**Author's note on provenance.** Every number below is labelled MEASURED or PROJECTED. "MEASURED (this session)" means I produced it myself, read-only, in this pass — chiefly by parsing the `DV01` envelope of the live rc.12 benchmark segment. No builds, no tests, no server boot (swap is at 141/148 GB; the neighbouring 41 GB process makes a boot unsafe). Where I fell back from `xc.py` to direct corpus grep, I say so.

---

## 0. Two things changed under the audits, and both invalidate parts of them

**(a) `main` has four unpushed commits, one of which is an engine change.**

```
f953b6e perf(search): cache the per-segment FTS reader instead of rebuilding it per query   ← ENGINE
61a52c4 test(perf): correct the text doc-values evidence — the target is semantic_text
dba6209 test(perf): prove nothing useful reads the text doc-values column
a28302c test(perf): add the rc.12 measurement harness and capture the baseline
origin/main is still at 8a6fa38.
```

`f953b6e` is the query-hot-path audit's lever L1, already implemented. It has never been through CI, never been reviewed, and **never been measured** — `engine/target/release/xerj` is dated 10:38, the commit 12:01. It must not stay on `main` unpushed. Move it to a branch and PR it.

**(b) `demo/playbooks/rc12/BASELINE.md` is wrong, and it is wrong in the way this repo has been burned before.**

Its query table reports `took: 0` for all 17 shapes and concludes:

> *"You cannot demonstrate a 2x speedup on a quantity that already measures zero… the correct statement for rc.12 is that query latency is at the measurement floor and must not regress — a guard, not an improvement target."*

That run (`results-baseline-main.json`, captured by `a28302c` at 11:35) was taken **before `XERJ_DISABLE_QUERY_CACHE=1` was added to the harness**. I verified this directly:

```
$ git show a28302c:demo/playbooks/rc12/measure_rc12.sh | grep DISABLE_QUERY_CACHE
NOT PRESENT in a28302c
```

The harness's own comment, added later at `measure_rc12.sh:54-60`, says it outright: *"A first run of this harness without it reported `took: 0` for all 17 shapes and nearly led to the conclusion that no query work was measurable at all."* This is the second occurrence of the exact failure documented in `demo/playbooks/CRITICAL_FINDING_read_perf_cache_mirage.md`.

The corrected cache-off run is `results-baseline-nocache.json`. **It reverses the verdict completely.** BASELINE.md must be rewritten before anything quotes it.

---

## 1. Verdict on the target

### Disk: **YES — 1.55x smaller, MEASURED, from one lever. Not 2x.**

MEASURED (this session) on `rc12bench`, the exact 500k-doc corpus the release will be measured on, by parsing the `DV01` envelope (`engine/crates/xerj-storage/src/doc_values.rs:629-671`) of `/tmp/claude-1001/xerj-rc12-bench/baseline-nocache/rc12bench/segments/653d62cd-…dv`:

| field | column kind | payload bytes | share of `.dv` |
|---|---|---:|---:|
| **`body`** (the one `text` field) | **keyword** | **54,988,379** | **95.87%** |
| `doc_id` | keyword | 1,335,785 | 2.33% |
| `ts` | numeric | 923,293 | 1.61% |
| all 11 others | — | 108,205 | 0.19% |

`.dv` is 57,357,593 B = 37.15% of the 154,413,168 B index. The single analyzed `text` field is **35.61% of the entire index**.

| | bytes | index/raw |
|---|---:|---:|
| today | 154,413,168 | 0.433x |
| − text doc-values (lever D1) | **99,424,789** | **0.279x** |
| − merge-tier zstd L6 (lever D2, PROJECTED) | ~90,903,187 | ~0.255x |

**D1 alone is 1.553x smaller — MEASURED numerator, exact byte count, on the release corpus.** With D2, ~1.70x PROJECTED.

Two honesty caveats that must ship with this number:

1. **It is corpus-dependent and `rc12bench` sits at the high end.** Its `body` is ~400 B of a ~713 B document. The verifier measured `.dv` at only **6.97%** across the `xc-*` code corpora (1.71%–11.88% per index); the in-tree `EVIDENCE-doc-values-on-text.md` measured 39.7% on a synthetic pure-text index. The defensible public statement is *"removes 90–99% of the doc-values sidecar, which is 2–37% of the index depending on how large your text bodies are relative to `_source` — 35.6% on our benchmark corpus."* Never quote 1.55x as universal.
2. **2x on disk is not reachable in rc.12.** Getting there needs the remaining `.post` (40.9 MB, 41% of the post-D1 index), and 60–65% of `.post` is positions. Positions cannot be dropped by default — ES's default for `text` is positions and `match_phrase` is in our own benchmark suite. The positions opt-out is a user-selected mapping option, not a release headline.

The ask was *"less disk"*. 1.55x measured clears it by a wide margin.

### Latency: **YES, and by far more than 2x — but only on specific shapes, and the honest claim is per-shape, never an average.**

MEASURED, `results-baseline-nocache.json`, cache off, `main @ 8a6fa38` (i.e. *before* `f953b6e`), 500k docs, one segment:

| shape | engine `took` | share of suite |
|---|---:|---:|
| `function_score` | 4,623 ms | 21.5% |
| `match_phrase_prefix` | 4,406 ms | 20.5% |
| `agg_terms_on_text` | 3,357 ms | 15.6% |
| `bool_must_filter` | 2,758 ms | 12.8% |
| `boosting` | 2,696 ms | 12.6% |
| `sort_on_text` | 1,530 ms | 7.1% |
| `match_text_multi` / `match_phrase` | 527 / 504 ms | 4.8% |
| `fuzzy` / `wildcard` / `prefix` / `match` / `large_page` | 309/177/169/167/166 ms | 4.1% |
| 9 keyword/numeric/agg shapes | ≤ 3 ms | ~0% |

16 of 25 shapes do measurable engine work; geometric mean **399 ms**; the top four are **70.5%** of all engine time in the suite.

The single most damning number is a comparison the audits missed:

```
match(body:index0000)                                  →   167 ms
the same match + term(status) filter + range filter    → 2,758 ms      16.5x
```

That is the ordinary search-box-plus-filters shape, and it is 16.5x slower than the search box alone. **2x is not the question here; the question is whether we can stop being 1000x off on four shapes.** We can.

What I will *not* claim: a uniform "XERJ is 2x faster". Nine shapes are already at the HTTP floor and will not move. An unweighted sum of 25 p50s is a synthetic denominator — the verifier was right to reject that framing for the read matrix and it applies equally here.

**Recommended release language:** *"rc.12 removes 35.6% of the on-disk index (0.433x → 0.279x index/raw, measured) and takes the four worst query shapes from seconds to milliseconds. Point-query latency was already at the measurement floor and is unchanged."*

---

## 2. Ranked work plan

Ordered by (expected win × confidence) / effort.

### R0 — Merge PR#186 and unblock CI. Effort: trivial.

`main` CI and Release are both **red at 8a6fa38** (runs 2026-08-06T17:08Z), from `cargo fmt` drift after the #179/#177 merge. PR#186 (`fix(ci): restore cargo fmt cleanliness on main`) is MERGEABLE and fixes exactly this. **Nothing else in this plan can be validated until main is green.** No format change, no risk.

### R1 — Correct `BASELINE.md`. Effort: trivial. Win: prevents shipping a false claim.

Replace the query table with `results-baseline-nocache.json` and delete the "query latency is at the measurement floor, a guard not an improvement target" conclusion. Add a one-line note that `results-baseline-main.json` is retained only as the recorded cache-mirage artifact. This is an honest-claims fix, not bookkeeping.

### R2 — Measure `f953b6e` (FTS reader cache). Effort: small (one harness run). Win: unknown, and that is the point.

Already landed, unmeasured. It removes a fixed per-query cost — a zstd inflate of the whole `.post` (40,951,589 B compressed on this corpus) plus `.meta` (1,464,327 B) — from every shape that opens the FTS reader. It is the floor under `match_text_common` (167 ms), `large_page` (166 ms), `match_phrase` (504 ms), `fuzzy`/`wildcard`/`prefix`, `boosting` and `function_score`.

**Every other latency attribution in this plan is contaminated until this is measured**, because the audits' numbers all predate it. Run the harness on `f953b6e` and diff against `results-baseline-nocache.json`. Do this before writing any more code.

Risk note found while reading it: the cache key is `(segment_id, sorted field set)`. A query needing a wider field set than a cached entry mints a second entry for the same segment. On an index with many text fields, distinct query shapes can multiply resident readers. It is budget-charged (`SegmentHydrationBudget`, `FtsReader` category) so it degrades rather than grows unbounded, but the harness should record `retained_bytes` per category alongside latency.

### R3 — Doc-values: default off for `text`/`semantic_text`, honour an explicit `doc_values: true`. Effort: medium. **Win: 1.553x disk, MEASURED. On-disk format: NO change.**

The highest-confidence item in the release.

**What exists.** `build_doc_value_columns` (`engine/crates/xerj-engine/src/index.rs:17334`) takes `sources: impl Iterator<Item = Option<&'a Value>>` — no schema parameter, so no mapping option *can* be consulted. `es_properties_to_fields` (`engine/crates/xerj-api/src/es_compat.rs:13495`) never reads `doc_values` or `index` (I grepped its whole body: zero hits). `FieldOptions.doc_values` (`engine/crates/xerj-common/src/types.rs:280`) exists, defaults `true`, and is populated by nothing. Call sites: flush `index.rs:22616` (under the comment *"Doc-values side-car (always built)"* at 22613), merge `index.rs:8475`.

**Changes.**
1. `es_compat.rs:13495` — in the per-field loop, set `fc.options.doc_values` and `fc.options.indexed` from the mapping, with ES's per-type defaults (`false` for `text`, `annotated_text`, `match_only_text`, `semantic_text`, `binary`).
2. `engine/crates/xerj-engine/src/engine.rs:2123` `es_type_to_field_type` — the second, divergent converter used by `apply_index_template`. Populate the same options here **and** add the missing `semantic_text` arm (see bug B4).
3. `index.rs` — new `fn doc_value_excluded_fields(schema: &Schema) -> HashSet<String>` beside `build_fts_field_configs`, keyed on **`options.doc_values`, not on `FieldType`**. This is load-bearing: `infer_field_type` makes every dynamically-mapped non-date string `FieldType::Text`, so a `FieldType`-keyed policy would strip columns from every dynamic index.
4. `index.rs:17334` — add a `skip: &HashSet<String>` parameter, `continue` before the per-doc `match val`, thread to both call sites.

**Both `text` and `semantic_text` map to `FieldType::Text` (`es_compat.rs:13678`), so one schema check captures both.** A policy keyed on the literal string `"text"` would capture under 5% on the `xc-*` corpora and be nearly worthless — this is the most important implementation detail and neither RFC #148 nor the original audit mentions it.

**Gate.** The dv-on-text audit rated this "HIGHEST RISK — takes the gate to 1357/3/3". **The verifier refuted that and I side with the verifier**, having re-traced it: `tests/es-compat-yaml/yaml/aggregations/terms_text_docvalues.yml` sets `doc_values: true` *explicitly*, so under "default off + honour the flag" all three cases keep their column and keep passing, unchanged. The risk rating was wrong, but the *design* it argued for is right anyway — for ES fidelity and because silently ignoring an accepted mapping option is indefensible on its own terms.

**Do not** share the default table at `es_compat.rs:3801-3809` verbatim as the audit's sketch instructs. That table is `!matches!(ftype, "text"|"annotated_text"|"match_only_text"|"binary")`; the sketch widens it with `semantic_text`/`object`/`nested`. `rewrite_unqueryable_exists` is the one place `doc_values` is honoured today, and widening its table would change `exists` answers and risk `search/160_exists_query.yml`. Keep them separate.

**Latency interaction, deliberate.** `agg_terms_on_text` (3,357 ms) and `sort_on_text` (1,530 ms) are served *from* this column. The harness already records the intended outcome at `measure_rc12.sh` in the shape comments: *"If the 'no doc-values for text' lever lands, these must turn into ERRORS, not faster queries — an `error` entry here is the SUCCESS condition, not a regression."* That is ES parity (`Fielddata is disabled on text fields by default`). Erroring is also the right call because the alternative — a silent fall-back to a brute scan — is precisely the "silent fake" class the stub audit exists to prevent.

**Reference-coding** (Apache-2.0 Lucene, verified byte-exact in `~/.xerj-code/corpora/lucene`, adaptable with attribution): `TextField.java:35-43` never calls `setDocValuesType`; `FieldType.java:44` defaults `DocValuesType.NONE`; `KeywordField.java:56-59` explicitly sets `SORTED_SET` — the discrimination XERJ's builder is missing. `SortedSetDocValuesField.java:49` is the "add a second field" mechanism modern ES uses to implement `doc_values: true` on `text`, which is why the lever must be *default-off-but-overridable* rather than *never*. `IndexingChain.java:1499-1508` enforces at the writer, the structural analogue of putting the check inside `build_doc_value_columns`.

### R4 — Stop a `bool` with a non-FTS filter from abandoning the FTS path. Effort: medium-large. **Win: 2,758 → ~170 ms MEASURED baseline (16.5x), and likely far better after R2.**

**This is the largest latency finding in the plan and no audit identified it.**

**Root cause, verified in code.** `query_node_to_fts` (`index.rs:31505`) projects the whole query or nothing. Its `Bool` arm at `index.rs:31724`:

```rust
for sub in must.iter().chain(filter.iter()) {
    let fq = query_node_to_fts(sub, text_fields, exact_fields)?;   // ← bails the ENTIRE bool
```

and its own comment three lines above says what happens: *"Project them as `must`, or fall back to the stored scan when one can't project (**Term/Range/etc. all project to None, so classic filters keep taking the doc-scan path as before**)."*

`QueryNode::Term { .. } => None` is at `index.rs:31672` with the rationale *"Trade-off: slower on huge segments, but segments are merged aggressively and most term queries are highly selective."* `Range` has no arm at all and falls to the catch-all.

So `bool { must: [match(body)], filter: [term(status), range(latency_ms)] }` returns `None`, `needs_fts` goes false, `scan_stored` goes true, and the query brute-scans 500,000 stored `_source` documents at ~5.5 µs/doc. The `match` alone is 167 ms; with two ordinary filters it is 2,758 ms.

**The engine already has both halves.** Doc-values prefilters exist for `Term` (`build_term_prefilter_cached`, `index.rs:20905`) and `Range` (`index.rs:20914`), and the top-level path already builds a `PrefilterSet` for a bare `Range` or `Term` (`index.rs:15033`, `15093`). They are simply never combined with an FTS lead.

**Change.** Make the `Bool` projection return a *hybrid* plan instead of `None`: project the FTS-expressible `must`/`should` children to `FtsQuery` as today, and carry the non-projectable `Term`/`Range` `filter`/`must` children as a prefilter set to be intersected against the FTS candidate stream. Fall back to the stored scan only when a child is expressible as *neither* (scripts, `terms_set`, geo). The `?`-on-every-child rule stays correct for those.

**Reference-coding** — retrieved by direct grep of `~/.xerj-code/corpora`, because the xc server is down and booting it here risks OOM (mandate rule 2: say so and fall back).

- **Lucene `BooleanScorerSupplier.java:137`** (Apache-2.0): `req(subs.get(Occur.FILTER), subs.get(Occur.MUST), leadCost, topLevelScoringClause)` — FILTER and MUST are combined into one required conjunction. Lucene has no "give up and scan stored fields" path at all.
- **Lucene `IndexOrDocValuesQuery.java`** (Apache-2.0) is the exact design, and its javadoc (lines 28-45) states XERJ's case precisely: *"it will use points in the case that they perform better, ie. when we need a good lead iterator that will be almost entirely consumed; **and doc values otherwise, ie. in the case that another part of the query is already leading iteration but we still need the ability to verify that some documents match**."* That is our `bool`: the FTS postings lead, the filters verify. The cost rule is `IndexOrDocValuesQuery.java:173-183` — `threshold = cost() >>> 3`, an explicit 8x penalty on doc-values because *"they still need to perform one comparison per document."*
- **tantivy `src/query/intersection.rs:59-70`** (MIT): `Intersection` + `go_to_first_doc` — the leapfrog to adopt for the intersection itself.

**Risk.** Medium, and it is score-semantics territory: `filter` clauses must not contribute to `_score` while `must` clauses must. The ES-YAML suite covers `bool` heavily, which is real protection, but gate this with an A/B that diffs hit `_id` sets and `_score` against the current stored-scan path on the same data before running the suite.

### R5 — `boosting` and `function_score` off the brute path. Effort: medium. **Win: 2,696 → ~170 ms and 4,623 → ~170 ms MEASURED baselines (16x, 27x).**

`is_doc_scan_query` (`index.rs:26753`) lists `QueryNode::Boosting` (26791) and `QueryNode::FunctionScore` (26803), forcing `scan_stored = true` and a whole-corpus stored walk. Both benchmark shapes have an FTS-expressible positive/base — `match(body:index0000)`, the 167 ms shape.

Cheapest correct form: peel the base with the existing `peel_function_score_boosted` (`index.rs:33777`), run it through the normal FTS route for candidate generation, and keep `apply_function_score` (`index.rs:15586`) as the post-pass it already is. `ScoredLeafKind` (`index.rs:32194`) has only `Keyword`/`BoolEq`/`NumericEq` and `function_score_columnar` covers only `field_value_factor` over `match_all` — do **not** try to widen those for rc.12; that is the structural version and it is a bigger change.

Sequence after R4: both land in the same "stop abandoning the FTS path" family and share the candidate-generation machinery.

### R6 — Merge-tier zstd at level 6. Effort: small. **Win: ~8.5 MB PROJECTED (~8.6% of the post-R3 index). On-disk format: NO change.**

Three production level sites, all hardcoded to 3: `stored_codec.rs:168` (`STORED_ZSTD_LEVEL`), `xerj-fts/src/index.rs:196` (`ZSTD_DURABLE_LEVEL`), `doc_values.rs:651` (a bare literal). Flush and merge call sites are cleanly separated (3 and 3), verified by the zstd-tiering audit and endorsed by its verifier.

Applying the verifier's independently re-measured L6 ratios (.seg −14.39%, .post −1.35%, .meta −15.44%, .dv −14.31%) to the post-R3 rc12bench layout: ~8,521,602 B.

**L6, not L9, and never L19.** L9 doubles the streaming-decode window from 2.00 to 4.00 MiB (the verifier measured this) — an unbudgeted memory cost on a project with a 112 GB OOM in its history. L19 costs 46x L9's CPU for 1.8x the bytes.

**CPU check, PROJECTED:** merge zstd input on this corpus is ~95 MB; at L6's measured 110 MB/s vs L3's 340 MB/s that is +0.6 s against a **116.5 s** force-merge — 0.5%. The ingest-merge audit's fear of merge-CPU blowup applies to L19, not L6. But measure it (§5), because force-merge is the metric it would damage.

**Flush stays at 3, non-negotiably**, behind a non-defaultable `CodecProfile { Flush, Merge }` enum plus a test asserting the flush sites pass `Flush`. The 2026-04-25 P0 (1.55M → 21k docs/s) is why.

Land the separate `zstd::encode_all` → `zstd::bulk::compress` change (`stored_codec.rs:515/2063/2073/2237`, `doc_values.rs:651`) as its own commit. Its certain value is fixing bug B7; the ~0.5–1.0% size gain is inferred, not measured on the proposed fix, and must be labelled that way.

### R7 — `match_phrase_prefix` (4,406 ms). Effort: medium. Win: MEASURED baseline, 20.5% of suite engine time.

The worst single shape after R3–R5. `execute_phrase` (`xerj-fts/src/search.rs:670-687`) builds a complete `HashMap<u32,(u32,Vec<u32>)>` for every phrase term before anchoring, and the prefix variant unions one such build per expansion (default `max_expansions` 50). Defer to a follow-up if the release is time-boxed — but it will be the largest remaining number, so say so rather than letting it look fixed.

### Dropped — refuted by the verifier, not quietly retained

| Lever | Why dropped |
|---|---|
| **Type-aware numeric/date doc-values codec** (RFC #148 lever 2) | REFUTED, high confidence. The verifier re-measured with the null bitmap fairly carried and netted against the free zstd-19 alternative: marginal whole-index value **−0.18%**, not −0.62%. Plain zstd-19 on today's format beats the proposed codec on 223 of 393 real integer columns. On rc12bench, all numeric columns together are 1,021,811 B — **0.66% of the index**. There is nothing here. |
| **Vector fields flattened into lexical FSTs** | REFUTED, high confidence. Every "measured" input was lifted verbatim from `af55577`'s commit body — a fix **already on main**. Incremental win on semantic workloads is **zero**, and 78% of the claimed saving was `_chunks`, which an explicit `dense_vector` mapping never produces. |
| **Positions opt-out as an rc.12 headline** | Cannot be a default (ES default for `text` is positions; `match_phrase` is in our own suite). Keep the mapping option as a user-facing feature for a later release. The `.pos` sidecar variant (Lucene `Lucene104PostingsFormat.java:331-340`, tantivy `segment_component.rs:11-13`) is the better idea and needs no mapping surface — cost it separately. |
| **Ingest-merge S1 (narrow the publication barrier)** | REFUTED as sketched. XERJ has no sealed memtable: `drain_shard_inner` (`memtable.rs:1663-1670`) `mem::take`s the shard, so moving `begin()` past the drain reopens a ~600 ms window in which drained docs are in neither the memtable nor a segment. It would fail `index.rs:850-874`, the test the audit named as its own gate. The real prerequisite is fjall's sealed-memtable design (`keyspace/mod.rs:721-760`) — LARGE, not rc.12. |
| **Ingest-merge S5 (flush-side postings reuse)** | REFUTED. `type DocId = Arc<str>` (`memtable.rs:201`) — postings are keyed by external id string, not ordinal, so it must add a remap and sort. And it only covers `store_positions == false` fields, which get `KeywordTokenizer` (one token/field/doc) — ~70% of the tokenize is in the one text field it cannot touch. ≤5% ceiling, possibly negative. |

---

## 3. Correctness bugs found

### Release-blockers

**B1 — `multi_match` on an unmapped field returns 0 hits for any multi-token query.** `engine/crates/xerj-engine/src/index.rs:27667`: the final arm of `doc_matches_query`'s `MultiMatch` case is `field_texts.iter().any(|ft| ft.contains(&q_lower))` — a whole-query **substring** test. For ES's default (`best_fields` + `operator: or`) a multi-token query must appear as a contiguous substring of some field, i.e. phrase semantics. Single-token queries pass; multi-token queries return nothing. ES requires only that one token match in one field. This is the open `multimatch-unmapped-field-bug`, and **the memory's suspected location is wrong** — the lowering at `index.rs:31555-31567` is correct; retract the suspicion of `query_node_to_fts`. Fix: token-intersection, mirroring the `is_cross` arm. Not caught by the ES-YAML suite. One line.

**B2 — `doc_values: false` is accepted, echoed back on `GET _mapping`, and silently ignored.** `es_compat.rs:13495` never reads the key (grepped: zero hits in the function body); `types.rs:280` stays at its `true` default; `index.rs:17334` has no schema parameter to consult. Live-proven in `EVIDENCE-doc-values-on-text.md`: a `keyword, doc_values:false` field still produces a column and still aggregates. **An accepted-but-ignored mapping option is a correctness bug, not a footprint footnote** — the user made an explicit storage decision and got neither the saving nor an error. Fixed by R3.

**B3 — `index: false` is also silently ignored, same root cause.** `types.rs:403 is_searchable()` returns `options.indexed`, never populated. `_field_caps` reports `"searchable": true` where ES reports false. Fixed by R3.

**B4 — `semantic_text` declared in an index template silently loses all full-text search after flush.** `engine/crates/xerj-engine/src/engine.rs:2123` `es_type_to_field_type` has **no `semantic_text` arm** — I read the full match block; it falls to `_ => FieldType::Object`. Used by `apply_index_template` (`engine.rs:777`). `es_compat.rs:13670-13678` documents this exact failure being found and fixed for the *other* converter (`es_type_to_native`) and never applied here: *"It used to fall through to `Object`, which FTS-indexes the whole value as ONE keyword token — match/BM25 then found the doc pre-flush (memtable scan) but ZERO hits post-flush."* Silent, total loss of lexical recall. The same block is also missing `match_only_text`, `annotated_text` and `search_as_you_type`, which are all accepted mapping types (`es_compat.rs:1088-1104`).

**B5 — A fully-dead merge batch is re-selected every 5 seconds forever.** `index.rs:8319` returns `None` when `live_doc_count == 0`; the driver's `Ok(None) => {}` arm drops it; `select_merges` is stateless over the snapshot. Each pass re-opens every input, zstd-decodes the whole stored section and JSON-parses every doc, then discards it. On any overwrite- or delete-heavy index: unbounded disk (inputs never retired) plus permanent background CPU. `force_merge` cannot rescue it (`index.rs:7796-7802` breaks on `n == 0`).

### Not release-blocking, but should land in rc.12

**B6 — `DISK_SIZE_2026-07-09.md:107` marks "Merge-path zstd 3→19" **DONE** with a measured −13.3% and a 1360/0/3 sign-off. The code does not exist in any ref.** `grep -rn 'MERGE_ZSTD_LEVEL|set_zstd_level|DV_MERGE'` over `engine/crates` returns nothing; `git log --all -S'MERGE_ZSTD_LEVEL'` matches only docs commits; all three levels are still 3. Most likely lost in the 2026-07-09 OOM session crash. This is a live honest-claims violation — a published measured number HEAD cannot reproduce — and it was the premise handed to the disk audit. Correct the doc to NOT LANDED.

**B7 — `stored_slices_retained_upper_bound` can never return a bound for a real segment.** `stored_codec.rs:634` reads the zstd frame content-size field and returns `Ok(None)` when absent, but the production encoder uses `zstd::encode_all` (`stored_codec.rs:515`), a streaming encoder that does not set it. Verified empirically by the zstd audit with `zstd --list -v` on a real segment. The tests miss it because the fixture uses `zstd::bulk::compress`, which *does* write it — production/test divergence. Also: the function has **zero production callers**, while its doc comment describes live behaviour that does not exist.

**B8 — The `[compression]` config block is entirely inert.** `xerj-common/src/config.rs:657-693` constructs `CompressionConfig { enabled, level, block_size_docs }`; nothing reads it. `engine/xerj.default.toml:173-180` tells operators `level = "best"` selects "Zstandard level 19. Maximum ratio (~5-6x)". Silently ignored. Same silent-fake class as B2. It is also the natural home for R6's knob.

**B9 — `execute_phrase` returns `Ok(Vec::new())` for a positions-less field** (`xerj-fts/src/search.rs:661`), not an error, and the caller latches `fts_handled` and skips the stored fallback — so a phrase query would silently return 0 hits with HTTP 200. Unreachable today (only non-`Text` fields lack positions, and only `Text` fields route to `FtsQuery::Phrase`), but it is a loaded gun for anyone implementing `index_options`. Change to `Err` as belt-and-braces.

**B10 — `FtsMemtable::remove` is O(distinct terms + doc count) under the shard write lock** (`memtable.rs:2285-2344`). Early-returns on the auto-id path, which is why the benchmark never sees it — but every explicit-`_id` `_bulk`, `PUT /_doc/{id}` overwrite and update pays it. This is the production-shaped ES workload. The ghost accounting it already maintains is the Lucene tombstone model; the fix is to mark the ordinal dead in an alive-bitset (tantivy `index_writer.rs:165 alive_bitset.intersect_update`) rather than physically unlink.

**B11 — `sweep_retired_segments` requires the *global* `read_leases` counter to hit exactly 0** (`index_store.rs:1512-1520`). Under sustained overlapping reads a zero-lease instant may be rare, so retired segments sit in the graveyard far longer than the comment claims. Inflates measured on-disk footprint under read load — directly relevant to §5's disk measurement.

**B12 — stale/incorrect docs that would mislead the next implementer:** `postings.rs:31-40` claims a skip table cannot be serialised because the blob is zstd-wrapped — false, `xerj-fts/src/index.rs:1038-1046` decompresses into an owned `Vec` and slices it by intra-blob offset, so offsets are already the addressing mechanism (this wrong comment is the stated reason skip-list acceleration was never built). `merge.rs:56-57` documents squared tiers where `tier_for` implements `log2`. Six stale "Zstd-19" comments in `xerj-fts/src/index.rs`. `postings.rs:150-152` describes a zero-length positions block the encoder does not write.

---

## 4. PR and issue dispositions

| Ref | Disposition | Reason |
|---|---|---|
| **PR#186** fmt cleanliness | **MERGE NOW** | `main` CI+Release are red at 8a6fa38; this is the unblocker for the entire release. |
| **PR#184** multibyte CREATE TABLE guard | **MERGE** after CI | The `autoindex` UTF-8 panic fix (`panic=abort` core-dumped whole runs on `'ا'`/`'设'`). Real crash fix, narrow. |
| **PR#185** reference-coding docs | **MERGE** after CI | Docs only, no engine surface. |
| **PR#156** refuse unsupported corpus reconciliation | **MERGE-AFTER-CHANGES** | Real data-integrity fix, but `--fresh` stops being an escape hatch (`state.rs:272` parses the journal before `state.rs:347` can delete it) while the error text still advertises it; unbounded refusal string (~7.3 MB projected); `xerj brain` takes the wrong branch on the exact case its new message exists for; PR body contradicts shipped code. 22 commits behind, CONFLICTING. **Not rc.12 work** — and it adds startup cost (journal parsed twice). |
| **PR#162** generation-based reconciliation | **NEEDS-AUTHOR-WORK** | Reverts #160's merged `Option<usize>` fix and will not compile after rebase; turns **every** existing non-empty legacy `--no-graph` state dir into a hard failure with no `--fresh` escape — including the reference corpora that `xc-index.sh` maintains. Moves `--no-graph` crash/resume coverage off the path it rewrites. Not rc.12. |
| **PR#166** PDF parse-once spool | **NEEDS-AUTHOR-WORK** | Mechanism and tests are genuinely strong, but the branch no longer compiles against main (`RawRecord` gained `origin`; `FileScan.sketches` became `Vec<GroupSketch>` inside the conflicted hunk), and the headline 17-21% was measured at `6c8ada8`, which is **not on the branch**. Re-measure at the rebased head. Not rc.12 — it *increases* transient disk by up to 384 MiB and is Linux-only. |
| **PR#167** autoindex map metadata drift | **MERGE-AFTER-CHANGES** | Two real agent-facing bugs (`bytes: 0` on unchanged resume; `run.started` = summary time), correct re-derive-from-durable-commits fix, strong tests. Needs rebase — main's #160 added a `/v1/embedding/identity` branch to the `handle()` chain this PR refactors — plus the `bytes` semantic change in CHANGELOG. Orthogonal to rc.12. |
| **ISSUE#173** 407-index fragmentation | **NEEDS-AUTHOR-WORK; fix A is rc.12-adjacent** | Fragmentation is real and reproduces (405 datasets from 2,182 files); root cause is one bug — `flatten_object` prefixes every leaf with the file's single root key (`extract/json.rs:70` → `extract/mod.rs:183`), so 426 valkey command files get pairwise-disjoint field sets. All three suggested directions are wrong or moot. The "1.2% semantic" and "singleton indices 0" figures are stale (pre-#180) and must be corrected in the body before they reach rc.12 notes. Fix B (an incapable index must contribute zero hits, not 400 the whole wildcard — ES parity, `KnnVectorQueryBuilder.java:516-517`) is a genuine parity fix worth taking. |
| **ISSUE#174** passage retrieval | **NEEDS-AUTHOR-WORK — re-scope first** | Three premises are wrong: `_passage` with byte offsets already ships and works; chunk offsets are retained, not discarded; `fragment_size` is not capped. The real defect is narrow — `_passage` is populated from exactly one call site (the kNN executor), so a lexical query asking for it gets a silent no-op. Retitle to "`_passage` is not populated on the lexical query path". Explicitly reject option (3) on the record: per-chunk vectors permanently force semantic search off HNSW onto brute force (`index.rs:9714`, `9850`). |

**None of the four triaged autoindex PRs advances either rc.12 axis.** Land them on their own merits; do not count them toward the release.

---

## 5. Measurement plan — the gate

The release may not claim a number this plan cannot verify.

### Preconditions

1. `main` green (R0 merged). A red baseline makes every A/B meaningless.
2. `f953b6e` moved off `main` to a branch + PR, CI green, then merged.
3. `engine/target/release/xerj` rebuilt — the current binary (10:38) predates `f953b6e` (12:01). Scoped: `cd engine && cargo build --release -j 32 -p xerj-engine -p xerj-server`.

### Both axes, one command

```sh
demo/playbooks/rc12/measure_rc12.sh <label> 500000
```

Run it once per landed lever, same doc count, same seeded corpus (`random.Random(20261207)`), so the A/B is byte-identical. Compare `results-<label>.json` against **`results-baseline-nocache.json`** — *never* `results-baseline-main.json`, which is the cache-mirage artifact.

Required labels, in order: `fts-reader-cache` (R2) → `dv-off-text` (R3) → `bool-hybrid` (R4) → `fnscore` (R5) → `zstd-merge-l6` (R6).

### Disk axis — what must be asserted

- `index_total_bytes` and `index_over_raw` from the JSON.
- Per-artifact `by_artifact` breakdown.
- **Per-field `.dv` attribution.** The harness does not do this and it is the number R3 turns on. Parse the `DV01` envelope directly (format at `doc_values.rs:629-671`: `u32` magic `0x44563031`, `u32` count, then per column `u8` kind | `u32` name_len | name | `u64` payload_len | payload). My script is at `/tmp/claude-1001/-home-claude-ai-xerj/03fcf4af-6568-40d2-b823-7e48dc0413b8/scratchpad/dvattr.py`; it should be promoted into `demo/playbooks/rc12/` and called by the harness.
- **Success condition for R3:** the `body` column is absent, and an index mapping `body` with an explicit `"doc_values": true` still has it.
- Force-merge to one segment first (the harness does), and note B11: retired segments may not be swept while reads are in flight, so measure with no concurrent load.

### Latency axis — what must be asserted

- Engine `took`, not wall-clock, for anything above ~1 ms; wall-clock only as the transport sanity check.
- `XERJ_DISABLE_QUERY_CACHE=1` — already in the harness at line 61. **Verify it is present in the running config before trusting any number.** Add an assertion to the harness that fails loudly if all 25 shapes report `took: 0`, so mirage #3 cannot happen.
- **`agg_terms_on_text` and `sort_on_text` must become `error` entries after R3.** An error is the success condition, not a regression. The harness comment already says so; make it an assertion.
- Report **per shape**. Do not publish a sum or mean over the 25 shapes — an unweighted sum of p50s over a hand-picked family list is not a workload.

### Additional runs the harness does not cover

- **Force-merge time.** MEASURED 116.52 s (baseline-main) vs 139.13 s (baseline-nocache) on the *same corpus* — **19% run-to-run variance**. Any claim about merge cost needs ≥3 repeats and a median. This is the metric R6 could damage.
- **Ingest.** 33.0 s / 31.86 s (≈15.2k docs/s). Guard against regression; not an improvement target.
- **Mixed read-under-write p99.** `SCORECARD.md:97-100`'s four loss cells (13.57/13.45/10.27/10.74 vs ES 3.45/6.76/3.68/3.57) were generated 2026-07-28 by `9d50fbb`, **before `collection_publication` landed 2026-08-02 (`825f8b3`)** put a whole-index reader/writer seqlock into `Index::search`. That headline is quoted in `AGENTS.md`, `README`, `llms.txt` and on xerj.org. **Re-run `demo/playbooks/bench-matrix.mjs` mixed phase before rc.12 is cut.** The rc.12 harness does not cover this — it force-merges and runs with no concurrent writer.
- **ES-YAML gate: 1360 passed / 0 failed / 3 skipped**, twice, on the final binary. Run `terms_text_docvalues.yml` explicitly before the full suite when R3 lands.

---

## 6. Sequencing and interactions

```
R0 fmt/CI green ──┬─→ R2 measure FTS cache ──┬─→ R4 bool hybrid ──→ R5 boosting/fnscore ──→ R7 phrase_prefix
                  │                          │        (share candidate generation — same worktree, in order)
                  ├─→ R1 correct BASELINE.md │
                  │                          │
                  └─→ R3 dv-off-text ────────┴─→ R6 zstd merge L6
                       (parallel worktree)        (AFTER R3 — see below)
```

**Must land in order**

- **R0 before everything.** Red CI invalidates every A/B.
- **R2 before R4/R5/R7.** The FTS reader open is a fixed cost under every FTS shape. Measure it first or you will attribute its removal to whichever lever happens to land next.
- **R4 before R5.** Both remove a query family from the brute stored scan and both need FTS candidate generation for a non-trivially-projectable query. R5 built on R4's machinery is much smaller than R5 built alone.
- **R3 before R6.** R6's win is a percentage of the bytes that remain. Applied before R3 it would compress 55 MB of doc-values we are about to delete, inflating the apparent win and wasting merge CPU. **The two do not stack additively** — R3 first, then measure R6 against the reduced index.

**Genuine conflicts**

- **Merge-time compression vs merge cost vs read-under-write p99.** This is the one real three-way tension. Force-merge is already 116.5 s and merges run on a `nice(15)` pool of `max(2, ncores/8)` with `XERJ_MERGE_PARALLELISM=1`. At **L6** the projected cost is +0.6 s (0.5%) and I judge it safe. At **L9** the streaming-decode window doubles 2.00 → 4.00 MiB, which is unbudgeted heap on a project with a 112 GB OOM in its history. At **L19** encode collapses to ~2 MB/s. Ship L6; make L9/L19 unreachable by config clamp. And because `MIXED_READ_UNDER_WRITE_FINDING_2026-07-08.md` is still open, attach a mixed-p99 re-measure to R6 specifically — not just a disk diff.
- **R3 vs `agg_terms_on_text`/`sort_on_text`.** Deliberate and disclosed: 4,887 ms (22.8% of suite engine time) becomes two errors. This is ES parity and the harness already declares it the success condition — but it *is* a behaviour change and belongs in CHANGELOG under a BREAKING heading, with the `doc_values: true` escape hatch documented.
- **R4/R5 vs the `_score` contract.** #177 established that a highlight block must not change `_score` or hit order. Any change to how a `bool` or `function_score` generates candidates must preserve scores exactly. A/B the hit `_id` set and `_score` against the current stored-scan path before the suite, not after.
- **R3 vs `rewrite_unqueryable_exists`.** Do not share the `es_compat.rs:3801-3809` default table. Widening it changes `exists` semantics for `semantic_text`/`object`/`nested`.

**Safe in parallel worktrees**

- R3 (`es_compat.rs`, `engine.rs`, `doc_values` builder) and R4/R5 (`query_node_to_fts`, `is_doc_scan_query`) touch disjoint code. They collide only in `index.rs` line numbers, not semantics.
- B1 (one line, `index.rs:27667`), B4 (`engine.rs:2123`), B5 (`index.rs:8319`), B6/B12 (docs) are all independent and can go in parallel with anything.
- The four autoindex PRs (#156/#162/#166/#167) all conflict *with each other* in `xerj-autoindex/src/lib.rs`. Whichever lands first forces the rest to rebase; #166 is the largest and should not land last.

---

## 7. What I could not determine

1. **What `f953b6e` actually buys.** The binary predates it and building is prohibited this phase. Every latency number here is a *pre-cache* baseline. This is the single largest gap and R2 exists to close it. In particular I cannot say how much of `match_text_common`'s 167 ms is the `.post`/`.meta` inflate versus the term scan itself.
2. **Where `bool_must_filter`'s 2,758 ms actually goes.** I proved the *mechanism* (the `?` at `index.rs:31724` demotes the whole bool; `Term`/`Range` project to `None`) and I have the two endpoints measured (167 ms vs 2,758 ms). I did not profile the 2,758 ms into scan-vs-parse-vs-filter, so R4's landing point (~170 ms) is an inference from the `match`-alone shape, not a measurement.
3. **Whether the ES-YAML gate actually holds for R3.** The audit said it breaks (1357/3/3); the verifier said it holds; I traced the code and side with the verifier. **Neither of us ran the suite.** Settle it by running `terms_text_docvalues.yml` before writing the rest of the lever.
4. **`.dv` share on a production-shaped corpus.** I measured 37.15% on `rc12bench` and the verifier measured 6.97% on the `xc-*` code corpora. Both are real; the truth is corpus-dependent and I cannot say which is more representative of a customer. The release must state the range, not a point.
5. **`.ids` at 3,057,388 B (2.0% of the index, 3.1% post-R3), described in BASELINE.md as "stored raw".** Nobody audited it. Possibly a cheap win; possibly load-bearing for point lookups. Unassessed.
6. **Mixed read-under-write p99 on current HEAD.** No measurement exists post-`825f8b3`. The published 55W/26T/4L headline is stale by a seqlock. I could not run it (memory).
7. **`doc_id` as a keyword doc-values column** (1,335,785 B, 2.33% of `.dv`) alongside a separate 3,057,388 B `.ids` file. Looks like double storage of the external id. Not investigated.
8. **Force-merge variance.** 116.52 s vs 139.13 s on identical input, two runs. I do not know the source — GC, page cache, or the neighbouring 41 GB process. Any merge-cost claim needs repeats.
9. **`xc.py` retrieval was unavailable.** The server was down and booting it against 9,362 indices with swap at 141/148 GB risked OOM. I fell back to direct `grep`/`Read` over `~/.xerj-code/corpora`, which the mandate permits and which is what the task brief prescribes once you have file:line targets. All citations below were verified byte-exact in the corpus checkout: Lucene `IndexOrDocValuesQuery.java:28-45,173-183`, `BooleanScorerSupplier.java:137`, `TextField.java:35-43`, `FieldType.java:44`, `KeywordField.java:56-59`, `SortedSetDocValuesField.java:49`, `IndexingChain.java:1499-1508` (all Apache-2.0); tantivy `src/query/intersection.rs:59-70` (MIT). **No Elasticsearch, sonic, or TurboPFor source was read or adapted** — the only ES-semantics claim in this plan (that modern ES supports `doc_values: true` on `text`) is sourced from XERJ's own in-tree conformance fixture `tests/es-compat-yaml/yaml/aggregations/terms_text_docvalues.yml`, not from ES code.