#!/usr/bin/env bash
# CI gate for the use-case harnesses under demo/usecases/.
#
# Those harnesses are the only executable proof that the shipped use cases
# still work — the brain HTTP surface, the MCP tool surface an agent sees, and
# autoindex's zero-config discovery. Until this script existed every one of
# them was run by hand, so a regression in any of the three was caught by
# nothing.
#
# It drives the EXISTING harnesses rather than restating them: gen-corpus.sh →
# boot-and-brain.sh → transcript.sh → mcp-smoke.sh, then run-eval.sh over a
# small generated corpus plus a search round-trip. Everything they need is
# passed as an env override, so this stays free of machine-specific paths.
#
# Deliberately self-contained: no browser, no model, no network, nothing
# outside the repo — only the release binaries, curl and jq.
#
# Env overrides:
#   XERJ_BIN                xerj binary        (default <repo>/engine/target/release/xerj)
#   XERJ_MCP_BIN            xerj-mcp binary    (default <repo>/engine/target/release/xerj-mcp)
#   XERJ_USECASE_ROOT       scratch root       (default a fresh mktemp -d)
#   XERJ_BRAIN_DEMO_PORT    brain es-compat    (default 9331; also takes +1/+2)
#   XERJ_AUTOINDEX_PORT     autoindex es-compat(default 9341; also takes +100/+101)
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SB="$REPO/demo/usecases/second-brain"

XERJ_BIN="${XERJ_BIN:-$REPO/engine/target/release/xerj}"
XERJ_MCP_BIN="${XERJ_MCP_BIN:-$REPO/engine/target/release/xerj-mcp}"
[ -f "$XERJ_BIN.exe" ] && XERJ_BIN="$XERJ_BIN.exe"
[ -f "$XERJ_MCP_BIN.exe" ] && XERJ_MCP_BIN="$XERJ_MCP_BIN.exe"

ROOT="${XERJ_USECASE_ROOT:-$(mktemp -d)}"
BRAIN_PORT="${XERJ_BRAIN_DEMO_PORT:-9331}"
AX_PORT="${XERJ_AUTOINDEX_PORT:-9341}"
BRAIN_ROOT="$ROOT/second-brain"
AX_WORK="$ROOT/autoindex"
AX_URL="http://localhost:$AX_PORT"
mkdir -p "$BRAIN_ROOT" "$AX_WORK"

FAILED=0
phase() { echo; echo "════ $* ════"; }
bad()   { echo "::error::$*"; FAILED=$((FAILED + 1)); }
ok()    { echo "  PASS: $*"; }

# The brain server is spawned detached by `xerj brain` and outlives that
# command by design, so it must be stopped explicitly however we exit.
cleanup() {
  for pidfile in "$BRAIN_ROOT/data/server.pid" "$AX_WORK/server.pid"; do
    [ -s "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null
  done
  return 0
}
trap cleanup EXIT

# ── 0. preflight ──────────────────────────────────────────────────────────
phase "0. preflight"
for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 \
    || { echo "::error::$tool is required by the use-case harnesses but is not installed"; exit 1; }
done

# Use the release binaries the job already built; build only the missing
# package if this is being run from a bare checkout.
build_if_missing() {
  local bin="$1" pkg="$2"
  [ -x "$bin" ] && return 0
  echo "  $pkg not built at $bin — building it"
  command -v cargo >/dev/null 2>&1 \
    || { echo "::error::$bin is missing and cargo is not available to build $pkg"; exit 1; }
  (cd "$REPO/engine" && cargo build --release -p "$pkg") \
    || { echo "::error::failed to build $pkg"; exit 1; }
}
build_if_missing "$XERJ_BIN" xerj-server
build_if_missing "$XERJ_MCP_BIN" xerj-mcp
echo "  xerj:     $XERJ_BIN ($("$XERJ_BIN" --version 2>/dev/null | head -1))"
echo "  xerj-mcp: $XERJ_MCP_BIN"
echo "  scratch:  $ROOT"

export XERJ_BIN XERJ_MCP_BIN

# ── 1. second brain: pinned corpus → live brain → API transcript ──────────
phase "1. second brain (corpus, boot, transcript)"
export XERJ_BRAIN_DEMO_ROOT="$BRAIN_ROOT"
export XERJ_BRAIN_DEMO_PORT="$BRAIN_PORT"

if bash "$SB/boot-and-brain.sh"; then
  ok "brain booted and indexed the pinned demo vault"
else
  bad "boot-and-brain.sh failed — no brain to assert against"
  tail -40 "$BRAIN_ROOT/data/server.log" 2>/dev/null
  echo "USE-CASE SMOKE FAILED"
  exit 1
fi

# overview/ego/link/unlink/as_of replay + the normative 400/404s.
if bash "$SB/transcript.sh"; then
  ok "second-brain API transcript"
else
  bad "second-brain API transcript failed (see the FAIL lines above)"
fi

# ── 2. the agent surface: MCP tools listed and callable ───────────────────
phase "2. MCP tool surface"
if bash "$SB/mcp-smoke.sh"; then
  ok "MCP tools listed and tools/call round-tripped"
else
  bad "MCP smoke failed (see the FAIL lines above)"
fi

# ── 3. autoindex: zero-config discovery, then find a value by search ──────
phase "3. autoindex discovery + query"
CORPUS="$AX_WORK/corpus"
mkdir -p "$CORPUS/orders" "$CORPUS/logs" "$CORPUS/people"
# Three shapes, three inferred datasets. The tokens are nonsense words so a
# hit can only come from THIS corpus having been discovered, parsed and made
# searchable — never from anything else on the runner.
cat > "$CORPUS/orders/orders.csv" <<'EOF'
order_id,customer,amount,placed_at
1,zanzibarite,42.50,2026-01-04
2,quovadium,17.25,2026-01-05
3,zanzibarite,99.00,2026-01-06
EOF
cat > "$CORPUS/people/people.jsonl" <<'EOF'
{"person_id":1,"name":"quovadium","city":"lisbon","role":"buyer"}
{"person_id":2,"name":"zanzibarite","city":"porto","role":"buyer"}
EOF
cat > "$CORPUS/logs/app.log" <<'EOF'
2026-01-04T10:00:00Z INFO  checkout completed for zanzibarite
2026-01-04T10:00:01Z WARN  retry scheduled for quovadium
2026-01-04T10:00:02Z ERROR payment declined for quovadium
EOF

export XERJ_AUTOINDEX_WORK="$AX_WORK" XERJ_AUTOINDEX_PORT="$AX_PORT"
if bash "$REPO/demo/usecases/autoindex/run-eval.sh" "$CORPUS" "$XERJ_BIN"; then
  ok "autoindex discovered the corpus and re-ran idempotently"
else
  bad "autoindex eval failed (discovery or idempotency)"
  tail -40 "$AX_WORK/server-$AX_PORT.log" 2>/dev/null
fi

curl -s -o /dev/null -m 30 -XPOST "$AX_URL/ax-*/_refresh" || true

# _cat/indices always emits the full `green open <name> …` row, so match the
# name column rather than anchoring at the start of the line.
DATASETS="$(curl -s -m 30 "$AX_URL/_cat/indices" | grep -cE '(^|[[:space:]])ax-' || true)"
echo "  ax-* datasets: ${DATASETS:-0}"
if [ "${DATASETS:-0}" -ge 3 ]; then
  ok "each of the 3 source shapes became its own dataset"
else
  bad "expected >=3 ax-* datasets from the 3-shape corpus, got ${DATASETS:-0}"
fi

if curl -s -m 30 "$AX_URL/autoindex-catalog/_search?size=0" | jq -e '.hits' >/dev/null 2>&1; then
  ok "autoindex-catalog is queryable"
else
  bad "autoindex-catalog index is missing or unqueryable"
fi

# The discovery claim in full: a value that existed only inside a CSV cell is
# retrievable by search, with no schema and no mapping written by hand.
HITS="$(curl -s -m 30 -G "$AX_URL/ax-*/_search" --data-urlencode 'q=zanzibarite' \
        --data-urlencode 'size=0' | jq -r '.hits.total.value // .hits.total // 0')"
echo "  hits for 'zanzibarite': ${HITS:-0}"
if [ "${HITS:-0}" -ge 3 ]; then
  ok "a CSV/JSONL/log value is searchable straight after discovery"
else
  bad "expected >=3 hits for 'zanzibarite' across the discovered datasets, got ${HITS:-0}"
fi

# ── summary ───────────────────────────────────────────────────────────────
echo
if [ "$FAILED" = 0 ]; then
  echo "USE-CASE SMOKE PASSED (second brain · MCP · autoindex)"
else
  echo "USE-CASE SMOKE FAILED — $FAILED phase(s) broken"
  exit 1
fi
