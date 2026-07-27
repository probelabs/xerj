# Per-file zero-day sweep (multi-agent workflow)

After the structural passes reported core "clean," a **per-file** review was run —
a multi-agent workflow over the 883 security-relevant files, each reviewer primed
to *assume bugs were missed and look harder*, with **adversarial verification** of
every candidate. This is the honest test of whether the structural approach had
blind spots. **It did — it found a real authz gap the graph missed.**

## The workflow
- **Scout** → the 883-file work-list (from the XERJ indices).
- **Review** → 177 agents (5 files each), reading real code + XERJ facts, hunting
  logic bugs / missing-insufficient-broken(`==`)-authz / second-order / TOCTOU /
  deser / type-confusion — reporting only concrete source→sink or broken-check
  candidates.
- **Verify** → an independent agent per candidate, tasked to **refute** it
  (reachable? guarded? dead code? receiver-type FP? deploy-only?).
- **Synthesize** → only survivors.

Scale: **222 agents, ~10.8M tokens, ~31 min**, 44 candidates → **5 confirmed**
after adversarial verification. (8 verify agents hit a usage limit near the end;
those candidates are unverified, not counted.)

## Confirmed findings

| # | file:line | class | severity | note |
|---|---|---|---|---|
| 1 | `wp-admin/user-new.php:100` | role injection (inconsistent `wp_ensure_editable_role`) | Medium (multisite + filtered roles) | **NEW — the structural graph missed it.** See FINDINGS.md #4 |
| 2 | `wp-includes/http.php:598` | SSRF `169.254` gap | Medium | independent re-discovery of FINDINGS.md #1 (validates the sweep) |
| 3 | `akismet/class.akismet-admin.php:1212` | CSRF (missing nonce on a GET view action) | Low | plugin, needs a logged-in admin + forged GET |
| 4 | `ID3/module.audio-video.flv.php:716` | resource-exhaustion DoS | Low | author+ uploads a crafted `.flv`; loop over attacker-set count |
| 5 | `build/pages/font-library/page.php:308` | missing capability check | Low | low-priv access to the font-library admin page |

Findings 3–5 are **at the workflow's adversarial-verifier confidence**; only #1
(and #2, already known) were re-read by the lead. #3–#5 are low-severity and
plugin/edge — reported honestly as candidates pending a lead read, not asserted.

## The honest takeaway — why the structural pass missed #1
My authz graph asked *"is there a capability check?"* — `user-new.php` has one
(`promote_user`), so it passed. The real bug is **inconsistency**: a second,
role-specific guard (`wp_ensure_editable_role`, added 6.8.0) is applied to 2 of 3
sibling role-sinks and **omitted on the third**. A "presence of a check" model
cannot see a *missing sibling guard*; a per-file read that compares the three
branches can. **Lesson: structural coverage and per-file reading are complementary
— the graph narrows and proves coverage; the read catches inconsistency and logic
gaps the facts don't encode.** This sweep is now part of the method.

## Cost
~10.8M tokens for the exhaustive per-file pass is the *thorough* end of the dial —
appropriate when you suspect misses. The cheap structural passes (~26k tokens for
the whole earlier audit) are the daily driver; this deep sweep is the periodic
"did we miss anything" backstop. Both belong in the workflow.

## Reproduce
The workflow script is under the session's `workflows/scripts/`; re-run points the
same reviewer/verifier prompts at any file list (`review_files.json`).
