#!/usr/bin/env bash
# Stage 1: ALIGNED subset — the first 15 frontier-bench tasks alphabetically
# (stage1-tasks.txt; rule fixed before any result was seen) x 3 models x 3 arms.
# Complete matrix first; extend to all 74 by re-running run_matrix without -i.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INC=(); while read -r t; do INC+=(-i "$t"); done < "$HERE/stage1-tasks.txt"
for MODEL in ${MODELS:-claude-opus-5 claude-opus-4-8 claude-opus-4-6}; do
  for ARM in ${ARMS:-native xerj xerj-tuned}; do
    echo; echo "######## stage1: frontier-bench / $MODEL / $ARM ########"
    "$HERE/run_matrix.sh" frontier-bench "$MODEL" "$ARM" "${INC[@]}" || \
      echo "!! cell $MODEL.$ARM non-zero — continuing" >&2
  done
done
echo "stage-1 pass complete — run ./collect_results.py"
