export const meta = {
  name: 'xerj-whitebox-methodology',
  description: 'Five-stage instruction/data whitebox audit of XERJ with vs without XERJ, adversarially verified, with a measured comparison and issue drafts',
  phases: [
    { title: 'Taxonomy', detail: 'instruction/data dangerous-sink taxonomy + per-category XERJ query and grep baseline' },
    { title: 'Lenses', detail: 'sinks, taint-trace, privilege/filters, filter-bypass, concurrency — each WITH and WITHOUT XERJ' },
    { title: 'Verify', detail: 'adversarial refutation of every finding, empirical where possible' },
    { title: 'Synthesize', detail: 'with/without comparison, findings report, case-study section, issue drafts' },
  ],
}

const WT = '/home/claude/ai/xerj-rust-audit-wt'
const SCRATCH = '/tmp/claude-1001/-home-claude-ai-xerj/e36b8c4c-3a94-4a76-80f1-2119380319c6/scratchpad'
const XERJ = 'http://127.0.0.1:9310'
const VENV = `${SCRATCH}/astvenv/bin/python`

const GROUND = `
## The mission (the user's own methodology — follow it precisely)

A single packed WHITEBOX security methodology, run on the XERJ Rust engine, measured
WITH XERJ (the AST/call-graph substrate) vs WITHOUT (grep + read the raw tree). The
organizing principle is the user's, and it is sharper than a category list:

  A function is DANGEROUS when INSTRUCTION and DATA share one channel with no separator.
  - SQL built by string concatenation/format! (instruction+data mixed) vs a prepared
    statement with bound params (separated). Dangerous = the mixed one.
  - A filesystem open/join where '/' and '..' in the DATA are interpreted as path
    INSTRUCTIONS to leave the intended directory.
  - A shell/process call where argument DATA can carry option INSTRUCTIONS
    (e.g. a value "-o/tmp/exploit" becomes a flag), even if each arg is shell-escaped —
    shell-escaping quotes metacharacters but does NOT stop argument/option injection.
  - Deserialization where DATA bytes carry type/'construction INSTRUCTIONS.
  A filter/sanitizer is only real if it removes the INSTRUCTION power from the DATA
  channel. "It calls escape()" is not enough — escape() for shell metacharacters does
  nothing against option injection.

The five stages (each is a lens below):
  1. Flag all dangerous END functions (sinks) by category, by the instruction/data test.
  2. Trace each dangerous sink back to user input / taint sources (the call graph).
  3. Map the privilege model + flows: which roles/checks/filters apply, and WHERE.
  4. Validate every filter/protection for BYPASSES (the shell-escape-vs-option-injection
     class, path-canonicalization gaps, order-of-check bugs, partial escaping).
  5. State machines: race conditions (TOCTOU), transaction/lock deadlocks, lock-across-await.

## The live substrate (USE IT — this is the WITH-XERJ path)

XERJ is running at ${XERJ} (ES-compatible wire). Three indices over CURRENT main:
  rustfns    — one doc per function (5,098 docs). Queryable fields:
     crate, file, line_start, line_end, fn_name, module_path, owner, trait_impl,
     is_test, is_async, is_unsafe_fn, is_pub, is_handler_shaped, extractors[],
     sinks[] (fs_read fs_write fs_delete fs_path_join fs_rename process deserialize
              net_egress spawn sql_ish log_secret),
     validators[] (explicit_validate typed_name checked_arith saturating try_convert
                   containment bounds auth),
     concurrency[] (lock_acquire relaxed_atomic spawn_blocking block_on dashmap),
     lock_across_await, guard_after_destructive_op, path_join_from_param, path_join_args[],
     alloc_product, alloc_from_param_count, alloc_args_all[], panic_ops[], has_narrowing_cast,
     calls_self, has_depth_guard, reads_config_limit, body (the source, up to 12k chars),
     body_truncated, params, signature, return_type.
  rustcalls  — caller->callee edges (87,150). Fields: caller_id, caller, callee,
     callee_path, file, line, kind (call|macro), is_method, resolvable.
     NOTE: method-call edges (is_method:true) are unresolved by name; free/path calls
     (resolvable:true) are the reliable graph.
  rustroutes — axum route registrations (477). Fields: method, path, handler, handler_path, file, line.
Plus *_pre69 mirror indices over the PRE-#69 tree (contains 6 known-fixed bugs — your
calibration/recall set).

Query examples (agents run these with curl via Bash):
  curl -s "${XERJ}/rustfns/_search" -H 'Content-Type: application/json' -d '{"query":{"bool":{"filter":[{"term":{"sinks":"process"}},{"term":{"is_test":false}}]}},"_source":["file","fn_name","line_start","body"],"size":50}'
  curl -s "${XERJ}/rustcalls/_search" -d '{"query":{"term":{"callee":"process_bulk_with_opts"}},"_source":["caller","file","line"],"size":100}'   (who calls X → walk toward handlers)
Boolean terms work as true/false (the old string-only quirk is fixed).

## The WITHOUT-XERJ baseline (measure it honestly, same lens)

For each lens ALSO run the grep an auditor would use on the raw tree at ${WT}/engine, e.g.
  grep -rn --include='*.rs' -E 'Command::new|process::Command' ${WT}/engine/crates
Record: how many match LINES, how many distinct FILES (a grep hit forces reading the whole
file), and whether the true finding is reachable that way at all. The AST hit is a FUNCTION;
the grep hit is a FILE — that unit difference is the core of the comparison.
Token measurement helper (use it for both sides): ${VENV} -c "import tiktoken;print(len(tiktoken.get_encoding('cl100k_base').encode(open('F').read())))"

## Environment / how to work

- Repo (read the real code here): ${WT}   (branch feat/rust-self-audit; do NOT switch branches)
- tree-sitter venv python: ${VENV}
- Existing tooling you may reuse: ${WT}/docs/examples/rust-ast-audit/{rust_ast_index.py,find_recursion_cycles.py,detection_quality.py,QUERY_COOKBOOK.md}
- You have Bash, Read, Grep. Use curl for XERJ, Read for source, grep for the baseline.
- To PROVE an exploit you may boot a throwaway server: write a toml with a unique
  es_compat_port in 9314..9330 and data_dir under ${SCRATCH}, boot it with the Bash
  tool's run_in_background (NEVER foreground — it gets reaped), poll readiness with
  \`until curl -s -o /dev/null -w '%{http_code}' localhost:PORT/ | grep -q 200; do sleep 2; done\`,
  hit it, then \`pkill -f 'es_compat_port = PORT'\`-style cleanup by data_dir path. The
  server binary is ${WT}/engine/target/release/xerj. Exit 134/139 on a crash IS the proof.

## Known calibration set (do NOT re-report these as new — use them to sanity-check your queries)

PR #69 fixed 6 bugs; the /_sql WHERE-parser stack-overflow (sql.rs) was found by THIS
project and is already FIXED on this branch. Anything you report must be NEW (present on
current main) or a genuinely missed sibling. If a lens finds nothing new, "nothing new,
here is what I swept" is the correct, honest result.

## Honesty rules (public dogfooding case study — truth over headline)

- Every finding cites a real file:line and quotes real code you READ. No invented lines.
- Distinguish unauthenticated-network vs authenticated vs operator-config vs internal-only.
  Severity without reachability is noise.
- A filter-bypass claim must name the concrete bypass input (e.g. an arg value "-oFILE").
- If you cannot reach a sink from user input, say so — an unreachable dangerous sink is
  informational, not a vulnerability.
- Measure both sides of every comparison or omit the claim.
`

phase('Taxonomy')

const TAX_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['categories', 'notes'],
  properties: {
    notes: { type: 'string' },
    categories: {
      type: 'array',
      items: {
        type: 'object', additionalProperties: false,
        required: ['category', 'instruction_data_rationale', 'xerj_query', 'xerj_candidate_count', 'grep_baseline', 'grep_file_count', 'separated_safe_form'],
        properties: {
          category: { type: 'string' },
          instruction_data_rationale: { type: 'string', description: 'why mixing instruction+data here is dangerous, and what the separated safe form is' },
          separated_safe_form: { type: 'string' },
          xerj_query: { type: 'string', description: 'the exact curl/query body used' },
          xerj_candidate_count: { type: 'integer' },
          grep_baseline: { type: 'string' },
          grep_file_count: { type: 'integer' },
        },
      },
    },
  },
}

const taxonomy = await agent(
`STAGE 1 — DANGEROUS-SINK TAXONOMY by the instruction/data principle.

${GROUND}

Build the taxonomy that the five lenses will use. Enumerate the categories of dangerous
END function in THIS codebase where instruction and data share a channel:
  - injection: SQL/DSL built by string concat/format! (vs prepared/bound) — the /_sql and
    query DSL paths; also any query built into another system.
  - path: filesystem open/join/create/delete where '..' or '/' in data is a path instruction.
  - process/argv: Command::new + args where data can carry option instructions (the
    shell-escape-doesn't-stop-option-injection class).
  - deserialization: bytes carrying type/construction instructions (serde_json::from_*,
    bincode, rmp, etc).
  - egress/SSRF: a URL/host from data steering a network instruction.
  - format/log injection if present.
For EACH category: state the instruction/data rationale and the separated-safe form;
give the XERJ query that enumerates that category's sinks (run it, record the live
candidate count); give the equivalent grep on ${WT}/engine and its file count. This is
also the first with/without data point.

Run every query for real against ${XERJ} and grep for real against the tree. Return the
structured taxonomy.`,
  { label: 'taxonomy', phase: 'Taxonomy', schema: TAX_SCHEMA }
)

phase('Lenses')

const FINDING_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['lens', 'with_xerj', 'without_xerj', 'swept', 'findings'],
  properties: {
    lens: { type: 'string' },
    swept: { type: 'string', description: 'what surface was covered and what came back clean' },
    with_xerj: {
      type: 'object', additionalProperties: false,
      required: ['queries', 'candidate_count', 'functions_read', 'approx_tokens'],
      properties: {
        queries: { type: 'array', items: { type: 'string' } },
        candidate_count: { type: 'integer' },
        functions_read: { type: 'integer' },
        approx_tokens: { type: 'integer', description: 'tokens actually needed to reach the findings via XERJ' },
      },
    },
    without_xerj: {
      type: 'object', additionalProperties: false,
      required: ['greps', 'match_files', 'approx_tokens', 'reachable'],
      properties: {
        greps: { type: 'array', items: { type: 'string' } },
        match_files: { type: 'integer' },
        approx_tokens: { type: 'integer', description: 'tokens to triage the grep hits (whole files)' },
        reachable: { type: 'string', description: 'could the no-XERJ path find these findings, and at what cost / not at all' },
      },
    },
    findings: {
      type: 'array',
      items: {
        type: 'object', additionalProperties: false,
        required: ['id', 'title', 'severity', 'file', 'line', 'code_quote', 'instruction_data_explanation', 'taint_path', 'reachability', 'attacker_input', 'existing_filter', 'bypass', 'impact', 'suggested_fix'],
        properties: {
          id: { type: 'string' },
          title: { type: 'string' },
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low', 'info'] },
          file: { type: 'string' },
          line: { type: 'integer' },
          code_quote: { type: 'string' },
          instruction_data_explanation: { type: 'string', description: 'exactly how instruction and data mix here' },
          taint_path: { type: 'string', description: 'source -> ... -> sink, with the call-graph hops' },
          reachability: { type: 'string', enum: ['unauthenticated-network', 'authenticated-network', 'operator-config', 'internal-only', 'unclear'] },
          attacker_input: { type: 'string', description: 'the concrete malicious input' },
          existing_filter: { type: 'string', description: 'what validation exists on the path, if any' },
          bypass: { type: 'string', description: 'for a filter-bypass finding, the concrete bypassing input; else "n/a"' },
          impact: { type: 'string' },
          suggested_fix: { type: 'string' },
        },
      },
    },
  },
}

const LENSES = [
  { key: 'sinks-injection', title: 'Stage 1+2 — injection sinks (SQL/DSL mixing) traced to user input',
    brief: `Enumerate injection-class sinks (query/DSL/SQL built by concat/format!, not bound).
Use the taxonomy's injection query as the start. For each, use rustcalls to trace toward a
handler (is_handler_shaped / a rustroutes entry) — the taint path from request to sink. The
/_sql stack-overflow is already fixed; look for OTHER query-building paths that mix data into
an instruction string without separation (e.g. building an ES query, a Lucene string, a regex,
a stored-script, an aggregation script, a routing key). Report the mix and whether request
data reaches it.` },
  { key: 'path-traversal', title: 'Stage 1+2+4 — path sinks + traversal-filter bypasses',
    brief: `Enumerate path sinks (fs_path_join / fs_write / fs_delete / fs_rename / create_dir_all)
where a data component is joined into a path. For each: is the joined component attacker-
influenced (path_join_args, trace via rustcalls)? Is there a containment filter, and is it
LEXICAL (defeatable by symlink or by an absolute component that .join swallows) or canonicalized?
Validate the filter for BYPASS: does trim_start_matches('/') still allow '..'? does starts_with
run BEFORE or AFTER the destructive op (guard_after_destructive_op)? PR #69 fixed index/snapshot
names — find siblings it missed (put_index_mapping, storage backend abs(), autoindex).` },
  { key: 'process-argv', title: 'Stage 1+4 — process/exec sinks + argument/option-injection bypasses',
    brief: `Enumerate process sinks (Command::new / process::Command / spawn of an external binary —
e.g. the PDF worker in xerj-autoindex, any subprocess). For each: what arguments are built, and
can DATA flow into an argument that the child interprets as an OPTION (the "-o/tmp/exploit" /
"--output=" class)? Shell-escaping does NOT stop this. Check for a '--' argument terminator and
for values that start with '-'. Report the concrete injecting input. Also flag any command whose
program name or path is data-derived.` },
  { key: 'privilege-filters', title: 'Stage 3 — privilege model + which checks/filters apply where',
    brief: `Map the auth/privilege surface: query rustfns for validators:auth and the console/api-key/
role paths; cross-reference rustroutes against which handlers sit behind an auth layer. Find:
routes that mutate or read data with NO auth check on the path; auth checks that fail OPEN on
error; timing-unsafe secret comparison (== on tokens vs constant-time); any-valid-key-acts-on-
any-object (IDOR) shapes; secrets in logs/responses (sinks:log_secret). State, per sensitive
route, what check applies and where — and where none does.` },
  { key: 'deser-egress', title: 'Stage 1+2 — deserialization + SSRF/egress sinks traced to input',
    brief: `Enumerate deserialization sinks (serde_json::from_*, bincode, rmp, from_slice/from_reader)
and network-egress sinks (reqwest, hyper client, TcpStream::connect). For deserialization: are
bytes attacker-controlled and is there a size/type bound before the parse (the bulk-amplification
class)? For egress: can a URL/host from request data steer the client to an internal address
(cloud metadata 169.254.169.254, localhost, link-local) — the SSRF the WordPress study found in
wp_http_validate_url? Trace each to its source.` },
  { key: 'concurrency', title: 'Stage 5 — state machines: races, TOCTOU, lock-across-await, deadlock',
    brief: `Query lock_across_await:true, concurrency:relaxed_atomic, concurrency:dashmap, and
guard_after_destructive_op. Find: a lock guard held across an .await (starvation/deadlock under
load); TOCTOU where a check and its use straddle a lock boundary (PR #69 added a re-check-under-
write-lock for the field limit — find where that pattern is MISSING); Ordering::Relaxed guarding
a non-trivial invariant; ack-implies-durable violations; soft-delete paths that leave data
readable; error paths that leave a structure half-updated. Use find_recursion_cycles.py for
mutual-recursion / lock-cycle shapes.` },
]

const lensResults = await pipeline(
  LENSES,
  (lens) => agent(
`WHITEBOX LENS — ${lens.title}

${GROUND}

## Taxonomy from stage 1 (your starting queries)
${JSON.stringify(taxonomy).slice(0, 3500)}

## Your assignment
${lens.brief}

## Method (measured, not optional)
1. WITH XERJ: run the queries against ${XERJ}, narrow to candidates, READ the real code for
   each with Read, trace reachability via rustcalls/rustroutes. Record queries, candidate count,
   functions read, and approx tokens you actually consumed to reach the findings.
2. WITHOUT XERJ: run the equivalent grep on ${WT}/engine, record match files and the approx
   tokens to triage them (whole files), and whether the no-XERJ path could reach the same findings.
3. Every finding: real file:line + quoted code, the instruction/data explanation, the taint path,
   reachability class, the concrete attacker input, the existing filter and its bypass (if any).
4. Prefer FEW solid findings over many speculative ones. This engine was already audited; the
   easy things are fixed. Zero findings + an honest sweep summary is a valid, valuable result.
Return the structured result.`,
    { label: `lens:${lens.key}`, phase: 'Lenses', effort: 'high', schema: FINDING_SCHEMA }
  ),
  (result, lens) => {
    if (!result || !result.findings || result.findings.length === 0) return result
    const capped = result.findings.slice(0, 6)
    return parallel(capped.map((f) => () =>
      agent(
`ADVERSARIAL VERIFIER. Your DEFAULT is that this finding is WRONG. Refute it.

${GROUND}

## The claim (lens: ${lens.title})
${JSON.stringify(f, null, 2)}

## How to refute
1. Open the file at the cited line and confirm the quoted code EXISTS there. A wrong line = refuted.
2. Find the guard the auditor missed: validation earlier in the fn, in the caller (read rustcalls
   to find callers, then Read them), in a middleware layer, in the type system, or a config default.
3. Test the reachability: does the route/handler actually exist and sit OUTSIDE auth? For an
   injection/traversal/argv claim, is the data really attacker-controlled at that point?
4. For a filter-BYPASS claim, the bypass input must actually work — reason it through or, better,
   BOOT A THROWAWAY SERVER (port 9314..9330, background) and try it. A curl that triggers it is
   proof; a curl that fails to is a refutation.
5. Check it is not already fixed on current main (this branch has PR #69 + the /_sql fix).
Refuted=true if wrong, misattributed, guarded, unreachable, already-fixed, or severity is
materially overstated. When genuinely unsure, refute — a false positive in a public case study
costs more than a missed finding.`,
        { label: `verify:${f.id}`, phase: 'Verify', effort: 'high',
          schema: {
            type: 'object', additionalProperties: false,
            required: ['finding_id', 'refuted', 'reason', 'code_at_line_confirmed', 'corrected_severity', 'corrected_reachability', 'empirical_test'],
            properties: {
              finding_id: { type: 'string' },
              refuted: { type: 'boolean' },
              reason: { type: 'string' },
              code_at_line_confirmed: { type: 'boolean' },
              corrected_severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low', 'info', 'not-a-bug'] },
              corrected_reachability: { type: 'string' },
              empirical_test: { type: 'string', description: 'what you ran and the result; "none" if not tested' },
            },
          },
        }
      ).then((v) => ({ finding: f, verdict: v }))
    )).then((verdicts) => ({ ...result, verdicts: verdicts.filter(Boolean) }))
  }
)

phase('Synthesize')

const clean = lensResults.filter(Boolean)
const confirmed = clean.flatMap((r) => (r.verdicts || []).filter((v) => v.verdict && v.verdict.refuted === false).map((v) => ({ lens: r.lens, ...v })))
const refuted = clean.flatMap((r) => (r.verdicts || []).filter((v) => v.verdict && v.verdict.refuted === true).map((v) => ({ lens: r.lens, id: v.finding.id, title: v.finding.title, why: v.verdict.reason })))
log(`lenses done: ${clean.length} lenses, ${confirmed.length} confirmed, ${refuted.length} refuted`)

const synthesis = await agent(
`STAGE 6 — SYNTHESIS. Turn the audit into the deliverables.

${GROUND}

## Taxonomy
${JSON.stringify(taxonomy).slice(0, 4000)}

## Per-lens results (with/without XERJ measured per lens)
${JSON.stringify(clean.map((r) => ({ lens: r.lens, swept: r.swept, with_xerj: r.with_xerj, without_xerj: r.without_xerj, n_findings: (r.findings || []).length }))).slice(0, 8000)}

## CONFIRMED findings (survived adversarial refutation)
${JSON.stringify(confirmed).slice(0, 14000)}

## REFUTED (report the count + the lesson, not the claims)
${JSON.stringify(refuted).slice(0, 4000)}

## Write these files (in ${WT})
1. docs/case-studies/xerj-self-audit/METHODOLOGY.md — the five-stage instruction/data whitebox
   methodology as a reusable playbook: the principle, the taxonomy, the XERJ query per stage, the
   grep baseline per stage, and the with/without comparison table (recall + tokens per lens,
   summed). Lead with the principle ("dangerous = instruction+data share a channel"). This is the
   dogfooding methodology section the website links to.
2. docs/case-studies/xerj-self-audit/FINDINGS-V2.md — every confirmed finding in full
   (file:line, quoted code, instruction/data explanation, taint path, reachability, attacker input,
   filter+bypass, impact, fix), then the refuted-count + lesson.
3. Append a "Five-stage methodology" section to docs/case-studies/xerj-self-audit/README.md linking
   METHODOLOGY.md and summarizing the with/without result in one table.
4. ISSUES.md (in the same dir) — a GitHub-issue DRAFT per confirmed finding that needs a fix
   (title, severity, reachability, the code, the fix). Do NOT create issues via gh — leave drafts
   for the human to review (public disclosure of an unfixed network-reachable bug is sensitive).

## Rules
- Do not invent numbers; every figure traces to the data above. Label any estimate.
- If confirmed findings are few or zero, that is the honest headline — the METHODOLOGY and the
  measured with/without comparison are the product, exactly as the WordPress study concluded.
- Do NOT commit. Return: files written, the with/without summary table (as text), the confirmed
  count by severity, and how many issue drafts you wrote.`,
  { label: 'synthesize', phase: 'Synthesize', effort: 'high' }
)

return {
  taxonomy_categories: (taxonomy && taxonomy.categories || []).map((c) => ({ category: c.category, xerj: c.xerj_candidate_count, grep_files: c.grep_file_count })),
  lenses: clean.map((r) => ({ lens: r.lens, findings: (r.findings || []).length })),
  confirmed_count: confirmed.length,
  refuted_count: refuted.length,
  confirmed,
  synthesis,
}
