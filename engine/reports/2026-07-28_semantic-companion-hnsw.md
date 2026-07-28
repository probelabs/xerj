# Semantic companion HNSW publication

## Outcome

`semantic_text` mappings now register their derived embedding target as an
internal native `Vector` field. The engine can therefore pin that target as
the index HNSW field, insert every eligible semantic document, persist the
graph, restore it after restart, and serve plain semantic queries through HNSW
with exact rescoring.

This is a correctness/path-selection fix, not a throughput benchmark. No
speedup claim is made here.

## Root cause

The API mapping converter attached `EmbeddingConfig` to the public text field
but did not add its derived target to the engine schema. HNSW intentionally
pins only native `FieldType::Vector` mappings so arbitrary numeric arrays do
not become graphs. Consequently semantic embeddings were written into
`_source`, but HNSW skipped them and semantic queries always used the exact
document scan.

Separately, the semantic executor called the brute-force function directly.
Even a correctly populated graph could not serve a `semantic` query.

## Change

- Add an internal vector companion for an implicit or custom semantic target,
  preserving dimensions and similarity.
- Never overwrite an explicit companion.
- Keep the raw public mapping as the mapping round-trip; the implicit
  companion is engine state, not a second user-declared field.
- Route eligible plain semantic queries through the existing HNSW gate and
  exact-rescore tail. Filtered queries, aggregation queries, non-pinned fields,
  incomplete/stale graphs, non-cosine mappings, small indices, and all other
  ineligible shapes retain exact fallback.
- Thread the request deadline through graph admission and candidate hydration.
  Expired work falls back to the cooperative exact timeout path and never
  publishes partial ANN candidates or caches a timed-out result.
- Add atomic multi-field schema publication so a semantic source and companion
  cannot become partially visible.
- Plan the merged raw mapping and schema additions for every index selected by
  a wildcard/comma-list `PUT _mapping` before publishing the first target.
  Deterministic candidate failures, such as a collision on a later target,
  therefore leave all earlier schemas and raw mapping round-trips unchanged.

## Mapping policy and pre-existing inconsistencies now rejected

The flagship fix exposed mapping states that previously returned success while
raw mapping and engine schema diverged. This change rejects them before
mutation:

- a companion explicitly mapped to a non-vector type;
- explicit target dimensions or similarity that differ from the semantic
  producer;
- multiple semantic fields sharing one target;
- changing an existing semantic field's target, dimensions, or similarity via
  `PUT _mapping`;
- nested `semantic_text` fields, which the ingest collector does not support;
- a custom dotted target resolved only through nested properties, or an
  ambiguous mapping containing both literal and nested definitions for the
  same logical path.

A literal top-level dotted target such as `"semantic.embedding"` remains
supported and has an API bulk/query regression test.

## Focused proof

The engine fixture indexes 1,024 documents with two semantic fields and proves:

- the first companion is pinned with 1,024/1,024 graph coverage;
- the body semantic query takes ANN (zero exact-scan checkpoints);
- the fixed exact winner is `s777`, with the same exact-rescored score as the
  brute oracle;
- measured recall@5 is at least 0.8 on the fixed repetitive fixture (the
  unique winner is stable; remaining ties intentionally do not promise
  ordering);
- `hits.total.value` remains `k`;
- expired queries return timed-out partial semantics and are not cached;
- filtered and aggregation semantic requests stay exact;
- a second, non-pinned semantic field stays exact instead of mixing fields;
- cached IDs/scores are stable and a write invalidates the cached answer;
- the graph remains covered and serves ANN after flush and fresh-engine
  restart.

The API fixture uses real create-index and bulk routes for 1,024 documents and
proves full HNSW coverage, semantic query success, public mapping round-trip,
flush/restart restoration, and literal dotted custom-target operation.
Existing phase-attribution coverage also proves successful semantic bulk
publication emits ordered Embed/WAL/Memtable/HNSW records with HNSW
`outcome=ok` and `vectors=1`.

## Current-main integration

The production changes were replayed onto current-main commit
`afcb3a76359945c4f1dfe2fd920f8307bc12f261` in the dedicated worktree
`/workspace/xerj-current-main-semantic-companion-hnsw`.

Both source commits applied without textual conflicts:

- `9d677073a46fb62059a9b518e524c7ddb95efa3b` supplied companion registration,
  semantic ANN routing, engine/API tests, and this report.
- `cf32c955fb2f9418b870f5a6f986afae50604bfe` supplied deterministic
  all-target prevalidation for multi-index mapping updates.

Applying without a textual conflict is not treated as proof of behavioral
composition. Static inspection specifically confirmed that the replay retains:

- passage-winner metadata and response projection from merged PR 63;
- the four-element scored tuple carrying an optional passage ordinal;
- multi-chunk semantic documents falling back to the passage-aware exact path;
- the static HNSW storage view and reused-external-ID persistence repairs from
  merged PRs 65 and 64.

At this pre-audit stage only `cargo fmt --all -- --check` and
`git diff --check afcb3a7..HEAD` were run. Both passed. No current-main test,
build, conformance, or benchmark result is claimed until an independent source
audit approves compilation.

## Historical source-branch verification

All commands ran in
`/workspace/xerj-fix-semantic-companion-hnsw/engine`.

- `cargo clippy -p xerj-engine -p xerj-api --all-targets -- -D warnings`
  passed.
- `cargo test -p xerj-engine -p xerj-api --lib` passed:
  `237` engine and `74` API tests.
- `cargo test --release -p xerj-engine fully_covered_semantic_companion_uses_ann_with_exact_rescore --lib -- --nocapture`
  passed `1/1`.
- `cargo test --release -p xerj-api semantic_companion --lib -- --nocapture`
  passed `10/10` after the follow-up deterministic multi-index validation
  regression.
- `cargo test -p xerj-api multi_index_put_mapping_validates_every_schema_before_publication --lib -- --nocapture`
  passed `1/1`; it places a clean target first and an engine-schema collision
  second in an explicit comma-list, then proves both schemas and both public
  mappings are unchanged.
- `cargo fmt --all -- --check` and `git diff --check` passed.
- A fresh branch-local
  `cargo build --release -j 32 -p xerj-server -p es-yaml-runner` passed.
  Server SHA-256:
  `3ffccd5e2df47b15375523a74811b89e714d6189be567db8742ccbc89363bcb5`.
  Runner SHA-256:
  `7adb54815499b793b5bdbe7f4340427e81c9e101b34e04a76bc2fd5965d1987b`.
- The built server used fresh data directory
  `/tmp/xerj-semantic-followup.ovrpYc`.
  `./target/release/es-yaml-runner --dir tests/es-compat-yaml/yaml`
  passed `1360`, failed `0`, skipped `3` (`1363` total).

## Excluded attempt

An earlier default-feature server build was interrupted with exit code `130`
after concurrent worktree build provenance was initially misidentified. Its
remaining branch-local compiler process was terminated, no artifact was used,
and no result from that attempt is included above. The recorded hashes and
conformance result come only from the later clean branch-local build after all
competing Cargo/Rust compiler activity had cleared.

## Open engine issue: strict cross-index transactionality

The multi-index follow-up deliberately guarantees validation-before-mutation
for deterministic mapping failures. It does not claim a strict transaction in
the presence of concurrent schema evolution or filesystem failure.

The exact pre-existing source chain is:

1. `xerj-api/src/es_compat.rs::put_mapping` plans several target indices.
2. `Index::add_fields` owns only one index's schema write lock; there is no
   engine-wide schema transaction lock, and dynamic ingest can evolve another
   target between planning and publication.
3. `Engine::put_index_mapping` persists one `es_mapping.json` at a time,
   logs serialization/write failures instead of returning them, and then
   updates `index_mappings` in memory. A later disk failure therefore cannot
   roll back already-published schemas or earlier mapping files.

A separate engine change should introduce a global prepare/persist/publish
primitive: acquire one transaction authority shared by explicit and dynamic
schema evolution, validate all candidate schemas, serialize and stage every
schema/raw-mapping file, atomically publish all in-memory states only after
staging succeeds, and restore staged files/in-memory snapshots on any commit
failure. Until that exists, the supported guarantee is the narrower,
tested deterministic validation boundary above.
