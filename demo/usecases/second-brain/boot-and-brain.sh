#!/usr/bin/env bash
# boot-and-brain.sh — one command from the demo vault to a running brain.
#
# Runs `xerj brain` on the generated corpus. That single command boots (or
# attaches to) a local xerj server, indexes the vault with link detection on,
# and prints the console URL — this script only stages the inputs and then
# verifies the server actually answers.
#
# Idempotent: a second run attaches to the already-running server and the
# indexing converges (same edge_ids — detector valid_at comes from pinned
# mtimes). Ports: default 9331 (the booted server also takes +1/+2 for
# rest/grpc — i.e. 9331-9333, inside the 9330-9345 demo range; never
# 9200/9310).
#
# Env overrides:
#   XERJ_BRAIN_DEMO_ROOT  working root   (default ${TMPDIR:-/tmp}/xerj-second-brain-demo)
#   XERJ_BRAIN_DEMO_PORT  es-compat port (default 9331)
#   XERJ_BIN              xerj binary    (default <repo>/engine/target/release/xerj)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"

DEMO_ROOT="${XERJ_BRAIN_DEMO_ROOT:-${TMPDIR:-/tmp}/xerj-second-brain-demo}"
PORT="${XERJ_BRAIN_DEMO_PORT:-9331}"
XERJ_BIN="${XERJ_BIN:-$REPO/engine/target/release/xerj}"
BRAIN="vault"
URL="http://localhost:$PORT"
CORPUS="$DEMO_ROOT/vault"
DATA_DIR="$DEMO_ROOT/data"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -x "$XERJ_BIN" ] || fail "xerj binary not found at $XERJ_BIN — build it: (cd $REPO/engine && cargo build --release -j 32 -p xerj-server)"

# Corpus: regenerate only when absent, so mtimes (= edge valid_at) stay put
# across re-runs of this script.
if [ ! -d "$CORPUS" ]; then
  "$HERE/gen-corpus.sh" "$CORPUS"
fi

echo "── xerj brain $CORPUS (brain '$BRAIN', $URL) ──"
"$XERJ_BIN" brain "$CORPUS" --brain "$BRAIN" --url "$URL" \
  --data-dir "$DATA_DIR" --no-open

# `xerj brain` already waited for readiness; verify independently anyway.
for _ in $(seq 1 30); do
  code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/health/ready" || true)
  [ "$code" = "200" ] && break
  sleep 2
done
[ "$code" = "200" ] || fail "server at $URL not ready (last probe HTTP $code)"

KEY_FILE="$DATA_DIR/admin.key"
[ -s "$KEY_FILE" ] || echo "note: no $KEY_FILE — server may be running unauthenticated or was booted elsewhere"

echo "PASS: brain '$BRAIN' live at $URL (api key: $KEY_FILE, data: $DATA_DIR)"
echo "next: ./transcript.sh   # the API proof"
echo "      ./mcp-smoke.sh    # the agent surface"
echo "stop: kill \$(cat $DATA_DIR/server.pid)"
