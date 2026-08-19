# Running Frontier-Bench v0.1 with XERJ as the agent's search tool

**Agent:** Claude Fable 5 (Claude Code) · **Date:** 2026-08-19 · **Task:** run the
recognized agentic-coding benchmarks (Anthropic model-card set) as a 3-arm A/B:
stock Claude Code vs Claude Code + XERJ (website-default) vs XERJ tuned-from-source.

## What worked
Harbor runs `claude-code` natively, so XERJ rides along as a custom agent that
uploads the rc.18 **musl release binary**, boots a server inside each task
container, and lets the agent `xerj autoindex` the repo itself — task, verifier
and scoring stay byte-identical across arms. Subscription OAuth works in-container
(`CLAUDE_CODE_OAUTH_TOKEN` + `CLAUDE_FORCE_OAUTH=true`; Harbor's own key
resolution remaps it into `ANTHROPIC_API_KEY` and breaks — pass it via `--ae`).

## What bit me
The locally-built glibc binary dies in task containers (`GLIBC_2.43 not found`)
— ship musl. Frontier-Bench v0.1 lives on Harbor Hub as
`terminal-bench/terminal-bench` (74 tasks, verified name-identical to the GitHub
repo), not under a frontier-bench slug. Model slug `claude-fable-5` silently
routed to opus-5 — always read `modelUsage` back before trusting a cell.

## Status (in progress — partial, first cell only)
Aligned stage 1 = first 15 tasks alphabetically × {opus-4-6, 4-8, 5} × 3 arms.
Cell 1 (opus-5, stock): **3/7 solved so far** (~3.7M input tokens/task); the
published Opus 5 number is 43.3%. XERJ arms not yet run — no A/B claim is made
here. Full results will land as a follow-up when the 135-trial matrix completes.
