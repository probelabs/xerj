# Server Retrieval Uplift — Battle Scorecard

**bare vs xc.1 (old server) vs xc.2 (new server)** — same tool (`xc.py`, unchanged), same
corpus (13-library `battle/` reflib: 32 files, 3 inferred datasets). **Only the server binary
changed.** xc.1 = `/tmp/xc-battle/bin/xerj-xc1`, xc.2 = `xerj-xc2` (both `xerj v1.0.0-rc.11`,
booted `--insecure --embed-mode lexical`, health green throughout). Corpus indexed fresh into
both instances via the shipped `xc-index.sh`.

---

## What changed in the server (branch `feat/server-retrieval-uplift`, commit `ee25292`)

- **`engine/crates/xerj-autoindex/src/extract/code.rs`** — the core enrichment. Per captured
  symbol, bake into the BM25 `defs` field: (1) the symbol's **signature** (enclosing
  declaration's first line, e.g. `pub fn furnish(width: u32, depth: u32) -> Sift`), and (2) an
  **identifier sub-word decomposition** (snake/camel/acronym/letter-digit split, e.g.
  `getHTTPResponse` → `get http response`, `id_to_fieldnorm` → `id to fieldnorm`). Also add a
  `signature` member to each `symbols[]` entry, broaden symbol capture (Rust
  const/static/union/enum-variant; Go const/var), and widen dedup to `(kind, name, signature)`.
- **`engine/crates/xerj-fts/src/analyzer.rs`** — additive building block: a position-preserving
  `WordDelimiterFilter`, a built-in `code` analyzer, and ES `word_delimiter` /
  `word_delimiter_graph` filter-name resolution. Purely additive; every existing analyzer stays
  byte-identical (regression-guarded).
- **`engine/crates/xerj-autoindex/src/lib.rs`** — test-fixture fix (#178 clustering-key test now
  uses a genuinely symbol-less file, since `const` is now captured).

**Why this should help retrieval:** a behavioural query (`parse timestamp string`, `retry
backoff`) previously had no token surface to match against an opaque `snake_case`/`camelCase`
identifier blob, because the ingest path's standard analyzer never splits those. Folding
signatures (parameter names + types) and sub-words into `defs` gives BM25 real tokens to match on
*what a function does*, not just its bare name.

**Scope caveat (verified during implement):** the engine's memtable ingest applies a single
analyzer to all fields and does **not** honor per-field mapping analyzers. So the win is delivered
extractor-side (sub-words baked into `defs`, which the standard analyzer then tokenizes), not via
a per-field `code` analyzer. The `code`/`word_delimiter` analyzer ships as a tested, additive
block for query/segment/`_analyze` paths, not as the autoindex mechanism.

---

## Retrieval scorecard

Same 16 queries, `xc.py -k 10`, both servers. Rank = position of the expected library.

| Slice | n | xc.1 top-1 | xc.2 top-1 | xc.1 top-3 | xc.2 top-3 |
|---|---|---|---|---|---|
| EASY (keyword)   | 8  | 8/8  | 8/8  | 8/8  | 8/8  |
| BEHAVIOURAL      | 8  | 7/8  | 7/8  | 8/8  | 8/8  |
| **ALL**          | 16 | **15/16** | **15/16** | **16/16** | **16/16** |

**Per-query rank changes (xc.1 → xc.2): none.** Ranks are byte-for-byte identical on all 16
queries. The single non-top-1 is the same on both instances: the *arena* behavioural query
(`C allocator ... dangling handle after free using a version counter`) returns **grove #1, arena
#2** — a true corpus ambiguity (grove is a Rust generational arena with a near-identical contract),
not a server defect. No query moved up **or** down.

- **Median end-to-end `xc.py` latency:** xc.1 = **60.4 ms**, xc.2 = **63.6 ms** (both dominated by
  Python startup + RRF fusion, not server query time; the 3.2 ms delta is noise).
- **Index stats:** both instances indexed the identical corpus — 32 records, 3 datasets, ~0.1 s
  build each; identical `_source` field set (title, language, defs, symbols). The **only** real
  difference is `defs` **content** + the new `signature` member. Example (`sift/src/lib.rs`):
  xc.1 `defs` carries bare `kind name` lines only; xc.2 additionally carries a signature line and a
  sub-word line per symbol (e.g. `function furnish / pub fn furnish(width: u32, depth: u32) -> Sift
  / furnish pub fn width u32 32 depth sift`). Enrichment is **verifiably present** in the xc.2
  index; it simply did not change any rank on this corpus.

**Why null here (honest root cause):** every library file ships a rich module-level doc-comment
describing its behaviour in prose (`least recently used`, `banker's rounding`, `append-only … seal
before replay`). The flat body/defs/title BM25 already saturates at top-1 through that prose, so
there is no headroom for the `defs` enrichment to move a rank. The enrichment pays off only on
files with **opaque identifiers and little/no prose** — a condition this doc-comment-heavy reflib
corpus does not exercise.

---

## Codegen scorecard

Subset of 3 tasks (`trellis-order`, `weft-scan`, `tally-round`), `csrun.py --trials 1 --rounds 2`.
**n = 1 per cell.** bare and xc.1 came from one run (server 9402); xc.2 from a second run (9412).

| Arm | Solved | Output tokens | Cost (USD) |
|---|---|---|---|
| bare              | **0/3** | 84,331 | $4.5516 |
| xc.1 (retrieval)  | **3/3** | 2,318  | $0.4477 |
| xc.2 (retrieval)  | **3/3** | 2,143  | $0.2263 |

Per-task output tokens (trellis / weft / tally): bare 27,441 / 20,935 / 35,955; xc.1 680 / 699 /
939; xc.2 730 / 558 / 855.

**Caveat (n=1):** the only robust signal is **bare (0/3) ≪ retrieval (3/3)** — bare burned ~84k
output tokens / $4.55 flailing on these unfamiliar custom-API contracts, hitting the 2-round budget
on every task. **xc.1 vs xc.2 is within noise:** identical solve rate (3/3 vs 3/3), near-equal
per-task tokens (mixed direction — weft down, trellis up), and the headline cost gap ($0.4477 vs
$0.2263) is driven by cache/input-token pricing variance across two separate runs, **not** by fewer
generations. Do not read it as an xc.2 efficiency win.

---

## Verdict

**The server-only change produced no measurable retrieval or downstream-codegen uplift over xc.1
on this corpus — and no regression.** Retrieval ranks are byte-identical across all 16 queries
(both 15/16 top-1, 16/16 top-3); latency is unchanged within noise; codegen solve rate is 3/3 on
both. The xc.2 `defs` enrichment (signatures + identifier sub-words) is real and verifiably present
in the index, but the doc-comment-heavy reflib corpus already saturates BM25 at top-1 through
prose, leaving no headroom for the enrichment to move a rank. It is built to help exactly the case
this corpus lacks — opaque identifiers with little prose — so the honest read is **"shipped,
behaviourally safe, ES conformance and non-code indexes unaffected, but unproven here."** The large,
robust win remains **retrieval vs no-retrieval** (0/3 → 3/3 codegen), which both server versions
deliver equally. To actually measure the xc.2 lever, re-run against an opaque-identifier /
prose-poor corpus.

**Scorecard file:** `/home/claude/ai/xerj/.claude/skills/xerj-code/measure/SERVER_UPLIFT_SCORECARD.md`

---

# Addendum — the decisive re-run on a prose-poor corpus (2026-08-06)

The verdict above said the null was because the reflib corpus is doc-comment-rich, and named the
fix: re-run on opaque-identifier / prose-poor code. Done. Built `battle-terse` = the same 13
libraries with **all comments and doc-strings stripped** (identifiers survive, prose gone;
verified: sift keeps `furnish` but loses `conservative`/`count-min`). Indexed into both binaries,
same shipped `xc.py`.

## Behavioural queries (concept words), terse corpus, n=12

| | xc.1 | xc.2 |
|---|---|---|
| top-1 | 8/12 | 8/12 |
| top-3 | **10/12** | **9/12** |

Two ranks moved, both **down** for xc.2 (spool #4→#5, quill #2→#4). **No improvement; a slight
regression.** Root cause: these queries use *concept* words (`cache`, `circular buffer`, `signed
integer`) that appear in the terse code neither as prose (stripped) nor as identifiers — so no
server can match them — while the signatures+sub-words xc.2 folds into `defs` add field length that
mildly **dilutes** BM25.

## Narrow queries the lever actually targets (identifier sub-words), n=4

| query | expected | xc.1 | xc.2 |
|---|---|---|---|
| "per tick refill" | cadence | **miss** | **#1** |
| "split mix hashing" | sift | miss | miss |
| "width depth constructor" | sift | #1 | #1 |
| "stop key threshold" | sift | #3 | #3 |

The enrichment **works where it is designed to**: `per_tick` → "per tick" took cadence from *absent*
to *#1*. But it fires only when a query term is a sub-word that is *not otherwise present* — 1 of 4
here; the rest already matched via whole identifiers in the code body.

## Final verdict (complete)

- **The server change is real, safe, and correct** — 272 engine tests pass, ES conformance and every
  non-code index byte-identical, `defs` verifiably enriched with signatures + identifier sub-words.
- **It does not improve realistic reference-coding retrieval**, and on prose-poor code it mildly
  regresses it, because the sub-words go into the *same* `defs` field and dilute BM25. It helps only
  a narrow query class (term = an identifier sub-word absent elsewhere), demonstrated once
  (cadence miss→#1).
- **The robust, corpus-independent result is unchanged and large: retrieval-vs-none** — bare 0/3
  ($4.55, ~84k output tokens) vs xc.1/xc.2 3/3 (~$0.3, ~2.2k tokens). Both server versions deliver
  that equally.

## The actionable next step (why it diluted, and the real fix)

Put the sub-word/signature tokens in a **separate, lower-weight field** (e.g. `defs_expanded`)
searched only as a recall fallback, instead of concatenating them into the primary `defs`. That
keeps the precision of the bare-identifier match while adding the sub-word recall that took cadence
miss→#1 — capturing the win without the dilution that cost spool/quill a rank. That is a one-field
mapping change on the same branch, and the right thing to measure next.

---

# Addendum 2 — the fix (defs_expanded), and it clears the bar (2026-08-06)

Applied the recommended fix: the signatures + identifier sub-words now go into a
SEPARATE `defs_expanded` field (verified: 14/14 code docs carry it on the new
server, 0 on the old), and the shipped tool searches it at a low boost
(`defs_expanded^0.5`). `defs` itself is back to the clean "kind name" list.

Fair comparison — each server with its matching tool (xc.1's tool cannot search a
field its index lacks; XERJ `multi_match` returns zero hits for a wholly-absent
field, so the new field must be paired with the new tool):

| slice | xc.1 | xc.2′ |
|---|---|---|
| behavioural (concept words), n=12 | top-1 8/12, top-3 9/12 | **8/12, 9/12 — identical** |
| sub-word / signature (the lever), n=4 | top-1 1/4, top-3 2/4 | **top-1 2/4, top-3 3/4** |

**The dilution regression is gone** (behavioural is now byte-identical to xc.1;
the first attempt had dropped spool #4→#5 and quill #2→#4) **and the recall win
survives** (`per tick refill` → cadence went from *absent* to *#1*). Net: xc.2′ is
strictly ≥ xc.1 — zero regressions, one gain. Small, but a real regression-free
improvement, plus richer symbol capture (const/static/variants) that costs nothing.
272 engine tests pass; ES conformance and non-code indexes unaffected. **Merged.**
