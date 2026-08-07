# ES-YAML conformance gate — run 2026-08-07

Run against a dedicated instance (`:9500`, fresh data dir) built from
`perf/fts-reader-cache` with **both** rc.12 engine changes in:

- the per-segment FTS reader cache
- `doc_values` honoured on the mapping (text/`semantic_text` default off)

```sh
engine/target/release/es-yaml-runner \
  --url http://localhost:9500 \
  --dir tests/es-compat-yaml/yaml
```

## Result

```
ES-COMPAT YAML RUNNER · 200 files · http://localhost:9500
1365 passed · 0 failed · 3 skipped · 1368 total
```

**Zero failures.** The doc-values change touches five flush call sites and the
merge path, so this was the gate that mattered most: had the skip set been keyed
on the resolved `FieldType` instead of the declared `es_type`, or applied to a
field carrying an explicit `"doc_values": true`, the
`aggregations/terms_text_docvalues.yml` cases would have failed here.

## The documented gate number is stale — gate on failures, not on the count

`AGENTS.md:11` states the gate as **"1360 passed / 0 failed / 3 skipped"**. This
run reports **1365 passed / 0 failed / 3 skipped / 1368 total** — five more
passing cases than the documented figure, because cases have been added to the
suite since that line was written. Nothing regressed.

Pinning an exact pass count makes the gate fragile: it goes "red" every time
someone legitimately adds a test, which trains people to edit the number rather
than read the result. **The invariant worth stating is `0 failed`** (plus the 3
known skips), which is also what CI actually enforces. Worth correcting in
`AGENTS.md`.

Note for provenance: PR #156's body also claims "1,365 passed / 0 failed / 3
skipped" and the review of that PR flagged it as unverifiable against the
documented 1360. This run independently confirms 1365 is the current true count.
