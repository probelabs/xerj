#!/usr/bin/env bash
# mcp-smoke.sh — smoke-test the agent surface: the xerj-mcp stdio proxy
# pointed (via XERJ_URL) at the demo brain boot-and-brain.sh started.
#
# Checks that the four brain tools are listed (xerj_brain_ego, xerj_brain_link,
# xerj_brain_unlink, xerj_brain_overview) and that a real tools/call round-trip
# against the live brain succeeds. If the brain tools are not present, that is
# reported as a FAIL — never papered over.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"

DEMO_ROOT="${XERJ_BRAIN_DEMO_ROOT:-${TMPDIR:-/tmp}/xerj-second-brain-demo}"
PORT="${XERJ_BRAIN_DEMO_PORT:-9331}"
MCP_BIN="${XERJ_MCP_BIN:-$REPO/engine/target/release/xerj-mcp}"
BRAIN="vault"
KEY_FILE="$DEMO_ROOT/data/admin.key"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ ! -x "$MCP_BIN" ]; then
  echo "FAIL: xerj-mcp binary not found at $MCP_BIN"
  echo "      build it: (cd $REPO/engine && cargo build --release -j 32 -p xerj-mcp)"
  exit 2
fi

export XERJ_URL="http://localhost:$PORT"
[ -s "$KEY_FILE" ] && export XERJ_AUTH="ApiKey $(cat "$KEY_FILE")"

PASSED=0 FAILED=0
ok()  { echo "  PASS: $*"; PASSED=$((PASSED + 1)); }
bad() { echo "  FAIL: $*"; FAILED=$((FAILED + 1)); }

echo "── xerj-mcp smoke · XERJ_URL=$XERJ_URL ──"

# Line-delimited JSON-RPC over stdio: initialize → tools/list → one real
# tools/call against the live brain.
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"mcp-smoke","version":"0"}}}'
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
  printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"xerj_brain_overview\",\"arguments\":{\"brain\":\"$BRAIN\"}}}"
  sleep 2
} | timeout 30 "$MCP_BIN" > "$WORK/out.jsonl" 2> "$WORK/err.log"

if [ ! -s "$WORK/out.jsonl" ]; then
  echo "FAIL: xerj-mcp produced no output; stderr:"
  sed 's/^/  /' "$WORK/err.log"
  exit 1
fi

jq -s '.[] | select(.id == 1)' "$WORK/out.jsonl" > "$WORK/init.json"
jq -s '.[] | select(.id == 2)' "$WORK/out.jsonl" > "$WORK/tools.json"
jq -s '.[] | select(.id == 3)' "$WORK/out.jsonl" > "$WORK/call.json"

jq -e '.result.serverInfo' "$WORK/init.json" >/dev/null 2>&1 \
  && ok "initialize handshake" || bad "initialize handshake"

TOOLS=$(jq -r '.result.tools[].name' "$WORK/tools.json" 2>/dev/null | sort)
echo "  tools listed: $(echo "$TOOLS" | tr '\n' ' ')"
for t in xerj_brain_ego xerj_brain_link xerj_brain_unlink xerj_brain_overview; do
  echo "$TOOLS" | grep -qx "$t" && ok "tool $t listed" || bad "tool $t listed"
done

# Honesty strings the design mandates in every brain-tool description.
jq -e '[.result.tools[] | select(.name | startswith("xerj_brain_"))] as $b
       | ($b | length) > 0 and ($b | all(.description | test("not a graph database")))' \
   "$WORK/tools.json" >/dev/null 2>&1 \
  && ok "brain tools carry the not-a-graph-database honesty string" \
  || bad "brain tools carry the not-a-graph-database honesty string"

if jq -e '.result and (.result.isError != true)' "$WORK/call.json" >/dev/null 2>&1; then
  ok "tools/call xerj_brain_overview succeeded"
  jq -e '.result.content[0].text | fromjson | .exists == true and .edges.total >= 1' \
     "$WORK/call.json" >/dev/null 2>&1 \
    && ok "overview payload: exists:true with live edge counts" \
    || bad "overview payload: exists:true with live edge counts"
else
  bad "tools/call xerj_brain_overview succeeded"
fi

echo "──"
echo "mcp-smoke: $PASSED passed, $FAILED failed"
[ "$FAILED" -eq 0 ] && echo "RESULT: PASS" || echo "RESULT: FAIL"
exit $((FAILED > 0))
