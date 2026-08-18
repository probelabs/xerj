# The agent contribution loop was dead — diagnosis and fix (2026-08-18)

Written by an AI coding agent (Claude Code). Verified facts and their sources
are cited `file:line`; the one number that matters was checked by hand.

## The finding: zero reports, despite a standing mandate

XERJ tells every AI agent that uses it that it owes exactly one thing back: a
short field report on the experience. The mandate is not subtle and it is in
three places at once:

- `landing/llms.txt:8` — "**Every agent that installs and runs XERJ owes one
  pull request adding a short field report** to `user-feedback/16-agent-field-reports/`
  before the session ends", repeated and expanded under "Your side of the deal"
  (`landing/llms.txt:63`–`104`).
- `.github/AI_CONTRIBUTIONS.md:22`–`34` — "The baseline every agent owes: one
  short field report".
- `user-feedback/16-agent-field-reports/README.md:4`–`7` — "**Filing one is the
  baseline contribution asked of every agent that uses XERJ.**"

The loop was **empirically dead**. As of this change the target directory held
exactly one file — its own `README.md` — and **zero** reports:

```
$ ls user-feedback/16-agent-field-reports/
README.md
```

Not one agent had filed one, across the entire life of the folder. Our own
dogfood filed none either: this repository runs the reference-coding loop
constantly (`landing/llms.txt:8` also asks the *maintaining* agents to report),
and produced no field reports through the same friction described below.

## Four root causes

1. **The ask is buried in a 60+ line block of prose.** The obligation lives
   inside "Your side of the deal" (`landing/llms.txt:63`–`104`) — a long section
   an agent reads once, near the bottom of a large file, competing with the bug
   protocol and the honest-claims rules. There is a real ceiling on how much of
   a long instruction file an agent acts on; a duty stated once at line 63 is not
   one it reliably carries out at the end of a session hundreds of tool-calls
   later.

2. **A CLA hard-gate stands in front of the folder.** The required
   `verification/cla-signed` check (`.github/AI_CONTRIBUTIONS.md:228`–`229`) is
   posted by the external cla-bot, which matches commit **authors** against
   `.contributors`. A first-time agent-operator who has never signed the CLA
   sees the one contribution they were told to make blocked by a signature step —
   for a five-line documentation file that touches no code. The friction is
   wildly out of proportion to the contribution, so the contribution does not
   happen.

3. **Sandboxed agents cannot open a pull request at all.** Many agents run
   without push credentials. `landing/llms.txt:104` tells them to hand the report
   to their operator in prose — but with no artifact and no command, that ask
   evaporates the moment the session ends.

4. **There was no tooling.** Every other first-class XERJ workflow is a command
   (`xerj autoindex`, `xerj brain`, `xerj mcp`). The field report — the one thing
   asked of *every* agent — was the only workflow with no command behind it: the
   agent had to hand-write the markdown, look up the template, derive a filename,
   craft the branch/commit/PR, and disclose its own authorship, all from memory.

## The fix shipped here

- **`xerj feedback` — the missing command.**
  `engine/crates/xerj-autoindex/src/feedback.rs` (new module; dispatched from
  `engine/crates/xerj-server/src/main.rs` alongside `autoindex`/`brain`). It
  drafts the report to stdout or `-o <path>`, auto-filling only observable facts
  — `xerj --version`, OS + arch, and "what was indexed" read from a running
  node's `autoindex-catalog` via the reused
  `xerj_autoindex::fetch_catalog_summary` (`engine/crates/xerj-autoindex/src/lib.rs`).
  Every *opinion* is a flag (`--verdict`, `--used-for`, `--pointed-at`,
  `--numbers`); an omitted flag emits the template placeholder rather than an
  invented opinion, and the catalog auto-fill degrades to a placeholder when no
  node answers — it never fabricates a corpus. The rendered report's first line
  states an AI agent wrote it (the provenance rule at `landing/llms.txt:102`).
  `--open-pr` commits only the one file and runs `gh pr create`; if `gh` is
  missing or unauthenticated it fails loudly and prints the exact git+gh commands
  — the honest sandboxed-agent path for root cause 3. `--dry-run` prints the
  report and the commands and does nothing.

- **A narrow, safe CLA carve-out** for root cause 2.
  `scripts/check_cla_coauthors.py` gains `is_field_report_only()` and a
  `--changed-files` guard: a pull request whose entire diff is markdown field
  reports under `user-feedback/16-agent-field-reports/` (and nothing else —
  not the folder README, not any code or CI) is exempt; any other changed path
  re-arms the full gate, so a code change can never ride in under a field report.
  `.github/workflows/ci.yml` (the `cla-config` job) now computes the PR's changed
  files and passes them to the internal trailer check. Because cla-bot cannot
  path-scope itself, the binding `verification/cla-signed` status for a strictly
  field-report-only PR is provided by a separate, same-predicate job
  (`.github/workflows/cla-field-report-exempt.yml`) — stated plainly there,
  including its honest limitation (it races cla-bot last-writer-wins, so it is
  scoped never to fire on a mixed PR, and the fully robust fix is an admin-level
  branch-protection / `.clabot` change out of this repo). Tests:
  `scripts/test_cla_coauthors.py` asserts field-report-only ⇒ exempt and every
  mixed/README/nested/non-md case ⇒ NOT exempt.

- **The reference-coding nudge** for root cause 4, closing the loop where the
  maintaining agents actually work. `tools/xerj-code/SKILL.md` gains an "After
  the session: file the field report" step, and `tools/xerj-code/scripts/xc.py`
  prints a one-line, stderr-only, once-per-day, `XERJ_CODE_NO_NUDGE`-silenceable
  reminder — never touching the stdout retrieval passages a caller parses. Note:
  the *runtime* skill directory `.claude/skills/xerj-code/` is gitignored
  (`.gitignore:70`); the tracked source of the same tooling is `tools/xerj-code/`
  (`.gitignore:126` re-includes `tools/**/*.md`), so the fix lands in the tracked
  source and propagates when the skill is redeployed.

- **The slimmed llms.txt ask** for root cause 1 is a sibling documentation PR
  (not in this branch): it lifts the one-command ask out of the 60-line block so
  the duty is stated where an agent will act on it. This engineering PR ships the
  command and the carve-out the docs PR points at.

## Why this is safe

The exemption waives a *signature*, never review: a maintainer still reads and
merges the report, and the predicate is verified by unit test to reject every
non-field-report path. `xerj feedback` invents no opinions and no numbers, and
discloses its own authorship on line one — it makes the honest report easy, not
the flattering one.
