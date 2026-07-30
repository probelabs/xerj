# Give your agent a second brain it can cite

**Use case:** your agent works against a folder of notes — a wiki, a docs tree, an
Obsidian vault — and needs more than flat retrieval: *why are these two notes
connected*, *what did the notes claim last month*, and *let me record a connection
I just discovered, with the sentence that proves it*. That is usually a
Neo4j-plus-LLM stack. XERJ ships it in the binary you already run.

One command turns the folder into a **bi-temporal, evidence-carrying link index**,
and four MCP tools give your agent the whole loop: orient → inspect → assert →
retire, with every past moment replayable.

> XERJ doesn't understand your notes — it makes the structure you wrote
> queryable, timestamped, and evidence-backed, deterministically. It is a search
> engine with a graph-shaped index over its own documents, **not** a graph
> database and not an LLM pipeline.

Everything below was run end-to-end against a live XERJ. No graph database, no
embedding service, no config file.

---

## 1. One command from folder to brain

```console
$ xerj brain ~/notes
graph: brain 'notes' → .xerj-memory-notes-edges; 4 structural edges, 0 prior edges invalidated (5 detectors live)
phase B: indexing 7 files with 8 workers → http://localhost:9337

✓ your second brain is ready — 7 files, 8 links, 0.2s
  → http://localhost:9337/_xerj-console/#/second-brain?brain=notes
  agents: XERJ_URL=http://localhost:9337 xerj-mcp
```

That last line is the agent hookup. The detectors are **deterministic and
lexical** — `[[wikilinks]]`, relative markdown/html links, files that sit in the
same folder, section order. Same folder in, byte-identical brain out. Links the
author wrote (`wikilink`/`mdlink`/`href`) record the exact quote and byte offset
that taught them; structural links (`samedir`/`sequence`) record their rationale
in the same evidence slot (`a.md and b.md share directory notes`).

## 2. Wire the MCP server

`xerj-mcp` is a thin stdio proxy: whatever the engine says (including refusals)
reaches the agent verbatim. Point it at the URL the CLI printed:

```json
{
  "mcpServers": {
    "xerj": {
      "command": "xerj-mcp",
      "env": { "XERJ_URL": "http://localhost:9337" }
    }
  }
}
```

Secured server? Add `"XERJ_AUTH": "ApiKey <key>"` (the key a CLI-booted server
uses lives at `<data-dir>/admin.key`).

The brain surface is four tools on top of XERJ's six search/memory tools:

| MCP tool | Endpoint | The verb |
|---|---|---|
| `xerj_brain_overview` | `GET /_graph/{brain}/overview` | orient: counts, hubs, what taught it |
| `xerj_brain_ego` | `GET /_graph/{brain}/ego` | inspect one node's evidence-backed neighborhood |
| `xerj_brain_link` | `POST /_graph/{brain}/link` | assert a link, with your evidence |
| `xerj_brain_unlink` | `DELETE /_graph/{brain}/link/{edge_id}` | retire — never delete |

## 3. The loop, as the agent runs it

**Orient.** `xerj_brain_overview {"brain": "notes"}`:

```json
{"exists": true,
 "edges": {"total": 8, "live": 8, "invalidated": 0},
 "detectors": [{"detector": "samedir@1", "live": 4}, {"detector": "wikilink@1", "live": 4}],
 "hubs": {"out": [{"id": "0efdb5a0167606d3f7e0e1da85fd8527", "live_edges": 2}, "…"]}}
```

**Inspect.** `xerj_brain_ego {"brain": "notes", "node": "0efdb5a0…"}` returns
every link with direction, hop, type, confidence — and the evidence:

```json
{"edge_id": "…", "src": "0efdb5a0…", "dst": "…", "type": "wikilink",
 "evidence": {"quote": "The engine. See [[roadmap]] for what is next, and [[benchmarks]] for the numbers.",
              "source": "xerj.md", "offset": 58}}
```

Node previews are hydrated by default, and `not_shown` accounts for anything
clipped — the response tells you what it did *not* show.

**Assert.** The agent is a first-class writer into the same bi-temporal store.
Pass the exact quote you are relying on:

```json
xerj_brain_link {"brain": "notes", "src": "0efdb5a0…", "dst": "b9a299c5…",
                 "type": "contradicts",
                 "evidence": {"quote": "Reads: zero losses.", "source": "benchmarks.md", "offset": 17},
                 "confidence": 0.7}
→ {"created": true, "edge_id": "c54fd9779976994bb33cea0b8c8761ad", "…": "…"}
```

`edge_id` is deterministic over `(src, type, dst, valid_at)`: re-asserting the
same fact answers `created: false` with the same id. `valid_at` defaults to
server-now, so pass it explicitly when a retry must dedupe. Agent-asserted
links keep `detector: "manual@1"` forever — they never masquerade as detected.

**Retire, never delete.**

```json
xerj_brain_unlink {"brain": "notes", "edge_id": "c54fd977…"}
→ {"invalidated": true, "invalid_at": 1785402581729, "expired_at": 1785402581729}
```

Retiring twice is idempotent (`already_invalid_at`). And the link is *not gone*:

```json
xerj_brain_ego {"brain": "notes", "node": "0efdb5a0…", "as_of": 1785402581724}
→ the retired link is present — this is what the brain believed at that moment

xerj_brain_ego {"brain": "notes", "node": "0efdb5a0…", "include_expired": true}
→ present with its invalid_at — visible history, not a tombstone
```

Two clocks per link: `valid_at`/`invalid_at` say when the fact was true;
`created_at`/`expired_at` say when the brain learned it.

## 4. Graph-aware memory recall

The same coupling reaches `/_memory` recall at zero extra tools: give
`xerj_memory_recall` a `graph` argument and recall is restricted to (or blended
with) what the namespace's links can reach:

```json
xerj_memory_recall {"namespace": "gr", "query": "deploy",
                    "graph": {"mode": "restrict", "seeds": ["m1"], "hops": 1}}
→ {"hits": ["…"], "graph": {"mode": "restrict", "reachable": 2}}
```

A namespace with no links yet degrades gracefully (`no_edges_index: true`) —
graph-less recall stays bit-identical.

## 5. What this honestly is (and is not)

- **Not a graph database.** No query language, no shortest-path, no PageRank.
  Hops cap at 2 by design; ask for 3 and the engine refuses in plain words, and
  the proxy hands you that refusal verbatim:
  `hops is capped at 2: XERJ's second brain is a relationship layer over
  documents, not a graph database …` — iterate from `reachable` instead.
- **Not neural.** Detection is deterministic and lexical; links the author
  wrote (`wikilink`/`mdlink`/`href`) plus structural priors (`samedir`,
  `sequence`). It re-indexes structure; it does not discover hidden meaning.
- **Evidence shows what taught the link — not always a quote.** Author-written
  links carry the exact quote and byte offset; structural links carry a stated
  rationale (offset 0); a manual link without evidence is shown as asserted,
  not detected.
- **Replay resolution = indexing cadence.** `xerj brain` is a batch run, not a
  watcher; detected links take `valid_at` from file mtimes, which are only as
  trustworthy as your filesystem's history.

Related: [agentic memory](./agentic-memory.md) ·
[zero-config indexing](./zero-config-indexing.md)
