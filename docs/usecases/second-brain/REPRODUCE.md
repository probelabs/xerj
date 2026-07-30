# Reproduce the second-brain use case, exactly

Every command below was run on branch `feat/second-brain`. The corpus is this
repository's own `docs/` folder, so the whole thing reproduces from a clone.
Numbers (0.7 s, 364 links, …) are from one machine and one corpus; yours will
differ in the details, and the invariants (idempotence, evidence, replay)
should not.

Throughout, `$REPO` is the repository root and `$DATA` is a fresh, empty
directory for the brain's server data.

## 0. Build (scoped — never workspace-wide)

```sh
cd $REPO/engine
cargo build --release -j 32 -p xerj-engine -p xerj-api -p xerj-autoindex \
  -p xerj-console-api -p xerj-server -p xerj-mcp
```

## 1. Run the brain

```sh
DATA=$(mktemp -d)
$REPO/engine/target/release/xerj brain $REPO/docs \
  --url http://localhost:9340 --data-dir "$DATA" --no-open
```

Expected shape (values vary with the current state of `docs/`):

```
brain 'docs': 121 files (5 MB) under …/docs
booted xerj server (pid …) — data: …/data
phase A: 7 datasets inferred, 14 junk/skipped files
phase B: indexing 106 files with 8 workers → http://localhost:9340
✓ your second brain is ready — 121 files, 364 links, 0.7s
  → http://localhost:9340/_xerj-console/#/second-brain?brain=docs
```

Exit codes: `0` ready · `3` ready-with-junk (unreadable files recorded, never
fatal — expected on this corpus, which contains images and binaries) · `1`
nothing indexable / server failure · `2` usage.

**Idempotence check** — run the identical command again. It must attach to the
already-running server, converge fast (0.14 s here), exit `0`, and the link
count must not grow.

The booted server keeps its own admin key at `$DATA/admin.key`; every direct
HTTP call below needs it:

```sh
AUTH="Authorization: ApiKey $(cat "$DATA/admin.key")"
```

## 2. Claim 1 — evidence on every link

Pick a hub node from the overview, then ask for its neighborhood:

```sh
curl -s -H "$AUTH" 'http://localhost:9340/_graph/docs/overview' | python3 -m json.tool
# take an id from .hubs.in[0].id (call it $NODE)

curl -s -H "$AUTH" \
  "http://localhost:9340/_graph/docs/ego?node=$NODE&include_nodes=true" \
  | python3 -m json.tool
```

Verify one quote against the source file (this is the whole point):

```sh
# for an authored edge (detector mdlink@1 / wikilink@1 / href@1):
# evidence.quote must be verbatim text from evidence.source
grep -F "<paste evidence.quote here>" $REPO/docs/<evidence.source>
```

Notes on what you will see:
- `evidence.offset` is the byte offset **within the section** the link was
  found in, not within the whole file (contract §6.5).
- Structural edges (`samedir@1`, `sequence@1`) carry generated rationales
  ("a.md and b.md share directory …"), offset 0 — not extracted text.

## 3. Claim 2 — retire, then replay

```sh
# capture a moment before the retirement
T_BEFORE=$(date +%s%3N)

# assert a manual link (src/dst are node ids from step 2)
curl -s -H "$AUTH" -H 'content-type: application/json' \
  -X POST http://localhost:9340/_graph/docs/link \
  -d '{"src":"<id1>","dst":"<id2>","type":"manual"}'
# → 201 {"edge_id":"…", …}

# retire it
curl -s -H "$AUTH" -X DELETE http://localhost:9340/_graph/docs/link/<edge_id>
# → {"invalidated":true,"invalid_at":<ms>}

# now: the link is gone, and the response SAYS one link was excluded
curl -s -H "$AUTH" "http://localhost:9340/_graph/docs/ego?node=<id1>&types=manual"
# → 0 edges, "not_shown":{"expired_excluded":1,…}

# replay the moment before the retirement: the link comes back,
# with its later invalid_at visible
curl -s -H "$AUTH" \
  "http://localhost:9340/_graph/docs/ego?node=<id1>&types=manual&as_of=$T_BEFORE"

# or ask now with tombstones included
curl -s -H "$AUTH" \
  "http://localhost:9340/_graph/docs/ego?node=<id1>&types=manual&include_expired=true"
```

Also check that the 2-hop cap refuses honestly:

```sh
curl -s -H "$AUTH" "http://localhost:9340/_graph/docs/ego?node=<id1>&hops=3"
# → 400, "…not a graph database…"
```

## 4. The dashboard

Open `http://localhost:9340/_xerj-console/#/second-brain?brain=docs` in a
browser (the console handles its own login using the same bootstrap
credentials). What to look for: the ego ledger with links grouped by what
taught them, evidence on hover/click, the belief-time scrubber narrating
appeared/retired counts, and the AUTHORED vs STRUCTURAL split.

Automated equivalent (what our verification ran — the real data layer plus
the real renderers for all nine panels, in Node, against the live server;
47/47 checks; needs Node ≥ 20):

```sh
XERJ_URL=http://localhost:9340 XERJ_BRAIN=docs \
XERJ_ADMIN_KEY_FILE="$DATA/admin.key" \
  node $REPO/docs/usecases/second-brain/contract-check.mjs
```

Note: this proves the rendering logic and data contract, not real-browser
pixels/interactions — those were not machine-verified.

## 5. The agent surface (MCP)

```sh
XERJ_URL=http://localhost:9340 \
XERJ_AUTH="ApiKey $(cat "$DATA/admin.key")" \
  $REPO/engine/target/release/xerj-mcp
```

Then over stdio: `tools/list` must include `xerj_brain_ego`,
`xerj_brain_link`, `xerj_brain_unlink`, `xerj_brain_overview`; a
`tools/call` of `xerj_brain_overview` with `{"brain":"docs"}` must round-trip
with `"contract": "xerj-second-brain/1"` and edge counts that match step 2.

## 6. Test suites

```sh
cd $REPO/engine
cargo test --release -p xerj-engine --test graph_expand   # 5
cargo test --release -p xerj-server --test brain_cli      # 3
cargo test --release -p xerj-autoindex                    # 109
cargo test --release -p xerj-console-api                  # 76
cargo test --release -p xerj-mcp                          # 25
cargo test --release -p xerj-engine --lib                 # 241
cargo test --release -p xerj-api --lib                    # 84 pass, 1 fail
```

The one xerj-api failure (`reindex_pages_past_10k_via_keyset`) is
pre-existing on `main` (its commit is an ancestor of `main`) and unrelated to
the graph feature; it is listed so you are not surprised by it.

## 7. Cleanup

```sh
kill <server pid printed at boot>   # or: pkill -f 'xerj.*9340'
rm -rf "$DATA"
```

## Environment used for the published numbers

Linux x86-64, release build, 121 files / 5.7 MB corpus, fresh data dir, port
9340. Token economics and large-corpus scaling were not measured; treat any
extrapolation as yours, not ours.
