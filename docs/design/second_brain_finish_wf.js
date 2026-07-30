export const meta = {
  name: 'second-brain-finish',
  description: 'Recover the unfinished second-brain use case, verify it against its contract, redesign the UX from fresh research, implement, and prove it live end to end',
  phases: [
    { title: 'Recover', detail: 'build + test + spec-conformance ground truth — what actually works today' },
    { title: 'Research', detail: 'fresh landscape, agent-memory, UX and honest-differentiation research in parallel' },
    { title: 'Design', detail: 'synthesize research into one concrete design decision set' },
    { title: 'Implement', detail: 'close the gaps and build the redesigned UX + demo corpus' },
    { title: 'Verify', detail: 'live end-to-end xerj brain run + UX render + spec conformance' },
    { title: 'Ship', detail: 'case study + website page from what was actually proven' },
  ],
}

const REPO = '/home/claude/ai/xerj'
const SPEC = `${REPO}/docs/design/SECOND_BRAIN_SPEC.md`
const SCRATCH = '/tmp/claude-1001/-home-claude-ai-xerj/e36b8c4c-3a94-4a76-80f1-2119380319c6/scratchpad'

const GROUND = `
## Where this work stopped (recovered state — verified, do not re-derive)

The XERJ "second brain" use case was built across three parallel streams and then
ABANDONED MID-VERIFICATION when an earlier workflow's integrate agent died on an API
500. Recovery finding: **integration and shipping already completed in earlier
sessions; the VERIFY phase never ran.** So the code exists but has never been proven
to build, run, or match its contract.

- Branch: \`feat/second-brain\` in the MAIN repo at ${REPO} (already checked out there).
  14 commits ahead of main, **42 files, +10,528 lines**. Do NOT switch branches.
- The CONTRACT (authoritative, 1,217 lines): ${SPEC}. It is normative — exact index
  mappings, exact edge_id computation, exact endpoint shapes, exact UX panel ids.
  Read it before touching anything. Where code and spec disagree, the spec wins
  unless the spec is demonstrably wrong (say so loudly if it is).
- What exists on the branch (line counts verified):
  * \`engine/crates/xerj-engine/src/graph.rs\` (641) — graph traversal/expansion
  * \`engine/crates/xerj-api/src/graph_api.rs\` (1,662) — the HTTP half:
    POST /_graph/{brain}/link, DELETE .../link/{edge_id}, GET .../ego, GET .../overview
  * \`engine/crates/xerj-server/src/brain.rs\` (690) — the \`xerj brain <folder>\` one-command CLI
  * \`engine/crates/xerj-autoindex/src/detect/\` — link detectors (wikilink, mdlink, href, samedir, sequence, e2e)
  * \`xerj-ux/src/ux/ego-ledger.js\` (871), \`xerj-ux/src/data/second-brain-api.js\` (698),
    \`xerj-ux/src/dashboards/second-brain.js\` (64)
  * tests: \`engine/crates/xerj-engine/tests/graph_expand.rs\` (494),
    \`engine/crates/xerj-server/tests/brain_cli.rs\` (334)
- No TODO/unimplemented markers remain. The UX is written to hit the real API and
  explicitly NEVER falls back to mock data.
- Unrelated: \`git stash@{0}\` holds a prior session's debug PROBE. LEAVE IT ALONE.

## What the feature is (the story it must tell)

\`xerj brain <folder>\` points one binary at a folder of notes and turns it into a
queryable, **bi-temporal, provenance-carrying knowledge graph** — then a console
dashboard shows what your notes believe. The three claims that make it different:
  1. **Every link has a quote.** An edge is not an assertion, it is evidence — the
     detector records the exact text that justified the link.
  2. **Replayable at any moment.** Bi-temporal (\`valid_at\` / \`as_of\`): you can ask
     what the brain believed last Tuesday. Retired links are invalidated, never deleted.
  3. **One binary, zero config.** No graph database, no ETL, no embedding service.
LANGUAGE RULE (binding, from the spec): every human-visible surface says
"link / believed since / retired / what taught this" — NEVER schema vocabulary
(\`edge\`, \`src\`, \`dst\`, \`valid_at\`, \`as_of\`) in the UI.
HONESTY RULE (binding): XERJ is **not** a graph database and must never be presented
as one. It is a search engine with a graph-shaped index over its own documents.

## Build & run discipline (hard rules — violating these is a failed task)

- Cargo workspace is ${REPO}/engine. Build ONLY scoped:
  \`cargo build --release -j 32 -p <crate>\`. NEVER workspace-wide. NEVER \`cargo clean\`.
  The chain is xerj-engine → xerj-api → xerj-autoindex → xerj-console-api → xerj-server.
- \`cargo fmt\` + \`cargo clippy -D warnings\` clean on every crate you touch.
- Booting the server in this sandbox: foreground boots are REAPED (exit 144). Use the
  Bash tool's \`run_in_background: true\`, then poll readiness with
  \`until curl -s -o /dev/null -w '%{http_code}' localhost:PORT/ | grep -q 200; do sleep 2; done\`.
  Config is a TOML with \`[server] es_compat_port / rest_port / data_dir\`.
  **USE PORTS 9330-9345** and data dirs under ${SCRATCH} — other work may hold 9200/9310.
- Commits authored \`xerj-org <git@xerj.org>\`. NEVER a Claude co-author trailer. Never
  mention ctrl-frk. Do NOT push. Do NOT commit to main. Commit on \`feat/second-brain\`.

## Honesty rules (this becomes a public case study)

- Every claim traces to something you RAN. If you did not run it, say "not measured".
- If the build is broken, that is the headline — fix it, and say it was broken.
- "The feature does not work yet" is a valid, valuable finding. Never paper over it.
- Do not invent benchmark numbers. Measure both sides or omit the comparison.
`

phase('Recover')

const RECOVER_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['builds', 'build_errors_fixed', 'tests', 'spec_conformance', 'works_end_to_end', 'gaps', 'summary'],
  properties: {
    builds: { type: 'boolean' },
    build_errors_fixed: { type: 'string', description: 'what was broken and what you changed; "none" if it built clean' },
    tests: { type: 'string', description: 'exact test results per crate' },
    spec_conformance: {
      type: 'array',
      items: {
        type: 'object', additionalProperties: false,
        required: ['area', 'conforms', 'detail'],
        properties: {
          area: { type: 'string', description: 'e.g. edge schema, edge_id, POST link, GET ego, GET overview, recall blend, detectors, UX panels' },
          conforms: { type: 'string', enum: ['yes', 'partial', 'no', 'untested'] },
          detail: { type: 'string' },
        },
      },
    },
    works_end_to_end: { type: 'string', description: 'did `xerj brain <folder>` actually run and produce a queryable graph? exact evidence' },
    gaps: { type: 'array', items: { type: 'string' } },
    summary: { type: 'string' },
  },
}

const recovered = await agent(
`PHASE 1 — RECOVER. Establish the ground truth: does this feature actually work?

${GROUND}

An earlier build of the crate chain was already kicked off; assume nothing about it —
verify yourself.

Do this, in order:
1. **Build** the chain scoped: xerj-engine, xerj-api, xerj-autoindex, xerj-console-api,
   xerj-server. If it does not compile, FIX IT (minimally — the goal is a working
   baseline, not a redesign) and report exactly what was broken.
2. **Test**: run the second-brain tests — \`cargo test --release -j 32 -p xerj-engine --test graph_expand\`
   and \`-p xerj-server --test brain_cli\`, plus the crates' lib tests. Report real numbers.
3. **Spec-conformance audit**: read ${SPEC} and check the implementation against it —
   the edge index mapping (§2.1), the stored-document type discipline (§2.2), edge_id
   computation (§2.3), each endpoint's exact shape (§4.1-4.4), the recall graph modes
   (§5), the detector emission rules (§6.5), and the UX panel contract (§7.1). For each
   area say conforms yes/partial/no/untested with the evidence.
4. **End-to-end**: actually run it. Build a small notes folder (or use ${REPO}/docs —
   real markdown with real wikilinks/relative links), boot the server on port 9330 with
   a data dir under ${SCRATCH}, run \`xerj brain <folder>\`, then curl the graph endpoints
   (\`/_graph/{brain}/overview\`, \`/_graph/{brain}/ego?...\`) and show real JSON output.
   Does it produce a real graph with real quotes on the edges?
5. If the UX can be rendered/exercised headlessly (it is plain JS + a console API), check
   whether the dashboard's data contract matches what the API actually returns.

Return the structured result. Be blunt: if this thing is broken or hollow, say so with
evidence. That is the most valuable outcome of this phase.`,
  { label: 'recover-and-verify', phase: 'Recover', model: 'fable', effort: 'high', schema: RECOVER_SCHEMA }
)

phase('Research')

const RESEARCH = [
  {
    key: 'landscape',
    title: 'The second-brain / PKM landscape and what actually makes one valuable',
    brief: `Research the personal-knowledge-management and "second brain" landscape as it
stands now: Obsidian (+ its graph view and Bases), Logseq, Roam, Reflect, mem.ai, Notion
AI, Tana, Capacities, and the note-graph research tradition. Use WebSearch/WebFetch for
current facts — do not rely on memory. Answer concretely:
- What do users actually get value from, and what is famously *pretty but useless*
  (the "graph view is a hairball nobody uses" critique is well documented — get the real
  substance of it, and what the successful alternatives to a hairball are).
- Which of these tools carry PROVENANCE (why does this link exist?) and which carry
  TIME (what did I believe last month)? Be specific about who does and does not.
- What is the actual job-to-be-done: retrieval? rediscovery? writing? agent grounding?
Return findings with sources (URLs), and a blunt list of design traps to avoid.`,
  },
  {
    key: 'agent-memory',
    title: 'Agent-native memory: how AI agents actually consume a knowledge graph',
    brief: `XERJ's stated positioning is AI-native — the agent is the primary customer.
Research how AI agents actually use long-term memory and knowledge graphs today: MCP
(Model Context Protocol) memory servers, GraphRAG and its critiques, agentic-memory
patterns (episodic vs semantic), temporal knowledge graphs (e.g. Zep/Graphiti), and
retrieval-augmented approaches that use structure rather than pure embeddings. Use
WebSearch/WebFetch for current facts.
Answer: what interface does an agent WANT (tools? a query language? an ego query?), what
does bi-temporality buy an agent concretely, and what does provenance-per-edge buy it
(citation, trust, contradiction detection)? Note that this repo already has an MCP crate
(\`engine/crates/xerj-mcp\`) — read it and say whether the second brain should expose
itself through MCP, and exactly what tools it should expose.`,
  },
  {
    key: 'ux-design',
    title: 'Designing a knowledge-graph UX that is genuinely useful, not a hairball',
    brief: `Research and then DESIGN the visual/interaction language for this dashboard.
Read the existing implementation first — \`xerj-ux/src/ux/ego-ledger.js\` (871 lines),
\`xerj-ux/src/dashboards/second-brain.js\`, \`xerj-ux/src/data/second-brain-api.js\` — and
${SPEC} §7. Then research what actually works for graph/temporal/provenance UI: ego-network
views vs force-directed hairballs, timeline/time-travel controls, evidence-first
("show me the quote") interfaces, and dense-information dashboard patterns.
Also read the repo's own design language: ${REPO}/xerj-ux/assets/base.css and an existing
dashboard module for the house style, and ${REPO}/docs/case-studies/wordpress-security-audit/README.md
for the honesty tone.
Deliver a concrete design: what panels, what interactions, what the ego-ledger should
look and feel like, how time-travel is expressed, how a quote is surfaced. Be specific
enough to implement. Call out anything in the current implementation that should CHANGE.`,
  },
  {
    key: 'differentiation',
    title: 'Honest differentiation: what XERJ genuinely does that others do not',
    brief: `Adversarial, honesty-first analysis. Read ${SPEC} and the implementation, then
determine what this second brain ACTUALLY offers that a user cannot get elsewhere — and
what it does not. Use WebSearch to check the competition's real current capabilities.
Specifically pressure-test the three claims: (1) every link has a quote/provenance,
(2) bi-temporal replay, (3) one binary zero config. For each: is it true in the code, is
it genuinely rare in the market, and does a user actually care?
Then be brutal about the weaknesses: the detectors are lexical (wikilinks, relative
links, same-dir, sequence) — that is *structural* link extraction, NOT semantic
understanding. Does that undercut the "second brain" claim? What is the honest framing?
Also: XERJ is NOT a graph database (a binding rule) — so what happens at scale, and what
queries can it genuinely not answer? Return the honest positioning + the claims we must
NOT make.`,
  },
]

const research = await parallel(RESEARCH.map((r) => () =>
  agent(
`RESEARCH — ${r.title}

${GROUND}

## Recovered ground truth (what actually works today)
${JSON.stringify(recovered).slice(0, 6000)}

## Your assignment
${r.brief}

Use WebSearch and WebFetch for anything about the outside world — this must be current,
not recalled. Cite URLs. Read the real code in ${REPO} for anything about XERJ itself.
Be concrete and opinionated; a list of vague "considerations" is a failed result. Return
a thorough written report.`,
    { label: `research:${r.key}`, phase: 'Research', model: 'fable', effort: 'high' }
  )
))

phase('Design')

const design = await agent(
`PHASE 3 — DESIGN. Turn four research streams into ONE decision set an implementer can execute.

${GROUND}

## Recovered ground truth
${JSON.stringify(recovered).slice(0, 5000)}

## Research: landscape / what makes a second brain valuable
${String(research[0] || '').slice(0, 14000)}

## Research: agent-native memory + MCP
${String(research[1] || '').slice(0, 14000)}

## Research: UX design
${String(research[2] || '').slice(0, 14000)}

## Research: honest differentiation
${String(research[3] || '').slice(0, 12000)}

## Your job

Produce **the design**, as decisions, not options. Constraints:
- The working substrate (graph engine, HTTP API, CLI, detectors) is 10.5k lines that
  already exist and largely work. This is an EVOLUTION, not a rewrite. Justify every
  change to existing behavior; prefer additive.
- ${SPEC} is the contract. If your design changes it, write the exact spec amendment and
  say why the original was wrong.
- Everything must be buildable in this session by a handful of implement agents.

Decide and specify precisely:
1. **The use-case narrative** — the single story this feature tells, in the user's words.
   What is the demo? What does someone see in 60 seconds that makes it click?
2. **The UX** — final panel set, the ego-ledger interaction, how time-travel is expressed,
   how provenance/quotes are surfaced. Reference exact files to change and what the new
   render output must look like. Keep the binding LANGUAGE RULE.
3. **Agent surface** — should the second brain expose MCP tools? If yes, exactly which
   tools with which signatures (read \`engine/crates/xerj-mcp\` first). If no, say why not.
4. **Gap closure** — from the recover phase, which gaps MUST be fixed for the demo to be
   honest, ranked.
5. **The honest framing** — the claims we make and the claims we explicitly refuse to make.
6. **A work breakdown** into 4-6 independent implementation tasks with crisp acceptance
   criteria, each touching a disjoint file set (they will run in parallel).

Return the design as a detailed written document. It IS the implementation brief.`,
  { label: 'design', phase: 'Design', model: 'fable', effort: 'high' }
)

phase('Implement')

const TASKS = [
  { key: 'ux-dashboard', title: 'The dashboard + ego-ledger UX', hint: 'xerj-ux/src/dashboards/second-brain.js, xerj-ux/src/ux/ego-ledger.js, xerj-ux/src/data/second-brain-api.js, assets/base.css' },
  { key: 'api-gaps', title: 'Engine/API gap closure', hint: 'engine/crates/xerj-engine/src/graph.rs, engine/crates/xerj-api/src/graph_api.rs, memory recall graph modes' },
  { key: 'cli-demo', title: 'The `xerj brain` CLI path + a real demo corpus', hint: 'engine/crates/xerj-server/src/brain.rs, engine/crates/xerj-autoindex/src/detect/, a demo notes folder' },
  { key: 'agent-surface', title: 'The agent-facing surface (MCP tools / recipes)', hint: 'engine/crates/xerj-mcp, docs/recipes' },
]

const implemented = await pipeline(
  TASKS,
  (t) => agent(
`IMPLEMENT — ${t.title}

${GROUND}

## THE DESIGN (this is your brief — follow it)
${String(design).slice(0, 22000)}

## Recovered ground truth (what already works — do not break it)
${JSON.stringify(recovered).slice(0, 4000)}

## Your slice
${t.title}. Primary files: ${t.hint}. Other agents are working the other slices IN
PARALLEL in the same tree — stay inside your file set, and if you must touch a shared
file make the smallest possible edit.

Rules:
- Implement only what the design specifies for your slice. If the design is silent or
  wrong on a detail, use the spec (${SPEC}), and note the judgement call in your return.
- Rust: scoped release builds only, fmt + clippy -D warnings clean, add/extend tests.
- JS: no build step in this repo — plain ES modules. Match the house style of the
  existing dashboards exactly.
- Do NOT commit (the ship phase commits once, coherently). Do NOT push.
- VERIFY YOUR OWN WORK before returning: build it, test it, and where possible run it
  against a live server (ports 9330-9345, data dir under ${SCRATCH}).

Return: files changed, what you implemented, your build/test evidence, judgement calls,
and anything you could NOT do (honestly).`,
    { label: `impl:${t.key}`, phase: 'Implement', model: 'fable', effort: 'high' }
  ),
  (result, t) => agent(
`REVIEW the implementation slice "${t.title}" — adversarially, then FIX what is wrong.

${GROUND}

## What the implementer claims
${String(result).slice(0, 12000)}

Your job:
1. Verify the claims. Build it. Run the tests. Read the actual diff (\`git diff\` in ${REPO}).
   If a claim is not true, that is your finding.
2. Check it against ${SPEC} and the binding LANGUAGE + HONESTY rules (no schema vocabulary
   in UI text; never call XERJ a graph database).
3. Fix defects you find, minimally and carefully. Do not redesign.
4. Confirm fmt + clippy clean for any Rust touched.
Return: what you verified, what was wrong, what you fixed, and the final state.`,
    { label: `review:${t.key}`, phase: 'Implement', model: 'fable', effort: 'high' }
  )
)

phase('Verify')

const verified = await agent(
`PHASE 5 — LIVE END-TO-END VERIFICATION. Prove the whole thing works, or prove it does not.

${GROUND}

## The design that was implemented
${String(design).slice(0, 8000)}

## Implementation + review results
${JSON.stringify(implemented).slice(0, 14000)}

Do a full, honest, end-to-end run:
1. Build the whole chain scoped and clean. Report any breakage.
2. Run every second-brain test (graph_expand, brain_cli, engine/api/console lib tests) and
   the ES-YAML conformance gate if it is quick — report exact numbers.
3. **The real run**: pick a REAL folder of notes (e.g. ${REPO}/docs — real markdown with
   real links; or build a richer demo corpus if the implement phase made one), boot the
   server on port 9340 with a fresh data dir under ${SCRATCH}, and run
   \`xerj brain <folder>\` exactly as a user would. Capture the real terminal output.
4. Then interrogate the result over HTTP and show REAL JSON: overview stats, an ego query
   on a real note, and — critically — prove the two headline claims:
   * **every link has a quote**: show an edge carrying its evidence text.
   * **replayable at any moment**: assert a link, invalidate it, and show that a
     time-travel query still returns the earlier belief. If this does not work, SAY SO.
5. Check the UX data contract against the live API responses: does what the dashboard
   expects match what the server returns, field for field?
6. Report token/time/size economics only if you actually measured them.

Return a detailed verdict: what is PROVEN working (with the evidence inline), what is
partial, and what is broken. This report gates the case study — do not flatter it.`,
  { label: 'live-verify', phase: 'Verify', model: 'fable', effort: 'high' }
)

phase('Ship')

const shipped = await agent(
`PHASE 6 — SHIP. Write the use case from what was actually proven, and commit.

${GROUND}

## The design
${String(design).slice(0, 10000)}

## LIVE VERIFICATION (this is your source of truth — publish only what it proves)
${String(verified).slice(0, 18000)}

Write:
1. \`${REPO}/docs/usecases/second-brain/README.md\` — the use case: the story, what it does,
   the real terminal output of \`xerj brain <folder>\`, the real API responses, the two
   headline claims with their evidence, and an explicit "what this does NOT do" section
   (the detectors are structural/lexical, XERJ is not a graph database, scale limits).
2. \`${REPO}/docs/usecases/second-brain/REPRODUCE.md\` — every step to reproduce, exactly.
3. A website page \`${REPO}/landing/use-cases/second-brain.html\` — mirror the structure and
   house style of \`${REPO}/landing/use-cases/code-security-audit.html\` (read it first).
   Use the site's existing CSS classes; do not invent new ones. Do NOT add it to any
   subnav/index yet — the human will decide placement.
4. Commit everything from this whole workflow on \`feat/second-brain\`, authored
   \`xerj-org <git@xerj.org>\`, no Claude co-author trailer, ONE coherent commit (or a
   small number of logical ones). Do NOT push.

Rules: if verification showed something is broken or unproven, the docs must say so
plainly — a case study that overclaims is worse than none. No invented numbers.

Return: files written, the commit sha, the headline claims you published, and everything
you deliberately refused to claim.`,
  { label: 'ship', phase: 'Ship', model: 'fable', effort: 'high' }
)

return {
  recovered,
  design_summary: String(design).slice(0, 3000),
  implemented_slices: TASKS.map((t) => t.key),
  verified: String(verified).slice(0, 6000),
  shipped: String(shipped).slice(0, 4000),
}
