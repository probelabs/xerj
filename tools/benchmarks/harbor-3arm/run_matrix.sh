#!/usr/bin/env bash
# One benchmark x model x arm cell:  run_matrix.sh <benchmark> <model> <arm> [harbor args...]
#   benchmark: frontier-bench | deep-swe | senior-swe | terminal-bench | swe-bench-verified
#   arm:       native | xerj | xerj-tuned
# Requires: harbor (pip install harbor), docker + buildx + compose-v2,
#   CLAUDE_CODE_OAUTH_TOKEN (claude setup-token), XERJ_BIN (musl release binary).
# Re-running a cell RESUMES its harbor job (quota pauses are expected on subscription).
set -euo pipefail
BENCH="${1:?benchmark}"; MODEL="${2:?model}"; ARM="${3:?arm}"; shift 3
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARBOR="${HARBOR:-harbor}"
JOBS_ROOT="${JOBS_ROOT:-$HERE/jobs}"
[ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ] || { echo "run: claude setup-token; export CLAUDE_CODE_OAUTH_TOKEN" >&2; exit 2; }
case "$BENCH" in
  # Frontier-Bench v0.1 lives on Harbor Hub under the terminal-bench slug
  # (74 tasks, verified name-identical to github.com/harbor-framework/frontier-bench).
  frontier-bench)     DATASET=(-d terminal-bench/terminal-bench) ;;
  deep-swe)           DATASET=(-d datacurve/deep-swe) ;;
  senior-swe)         DATASET=(--repo snorkel-ai/senior-swe-bench-v2026.06) ;;
  terminal-bench)     DATASET=(-d terminal-bench/terminal-bench-2-1) ;;
  swe-bench-verified) DATASET=(-d swe-bench/swe-bench-verified) ;;
  *) echo "unknown benchmark: $BENCH" >&2; exit 2 ;;
esac
case "$ARM" in
  native)     AGENT="claude-code" ;;
  xerj)       AGENT="xerj_agents:ClaudeCodeXerj" ;;
  xerj-tuned) AGENT="xerj_agents:ClaudeCodeXerjTuned" ;;
  *) echo "unknown arm: $ARM" >&2; exit 2 ;;
esac
CELL="$BENCH.$MODEL.$ARM"; JOB_DIR="$JOBS_ROOT/$CELL"; mkdir -p "$JOBS_ROOT"
export PYTHONPATH="$HERE${PYTHONPATH:+:$PYTHONPATH}"
AUTH_ARGS=(--ae "CLAUDE_CODE_OAUTH_TOKEN=$CLAUDE_CODE_OAUTH_TOKEN" --ae "CLAUDE_FORCE_OAUTH=true")
[ -n "${ANTHROPIC_API_KEY:-}" ] && AUTH_ARGS+=(--ae "ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY")
if [ -d "$JOB_DIR" ]; then echo "== resuming $CELL"; exec "$HARBOR" job resume "$JOB_DIR" "$@"; fi
echo "== starting $CELL (agent=$AGENT)"
exec "$HARBOR" run "${DATASET[@]}" -a "$AGENT" -m "$MODEL" \
  --jobs-dir "$JOBS_ROOT" --job-name "$CELL" -n "${CONCURRENT:-4}" -y \
  "${AUTH_ARGS[@]}" "$@"
