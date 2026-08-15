#!/usr/bin/env bash
# End-to-end proof of the single-node WAL tap (issue #320), with two real xerj
# nodes rather than a stub: documents written to the source must appear on the
# target within a poll interval, filtered by the allowlist, with deletes
# propagating and system indices never leaving the node.
#
# The unit and integration coverage lives in
# engine/crates/xerj-storage/src/wal.rs (tail/cursor/gap) and
# engine/crates/xerj-engine/tests/wal_tap_push.rs (push semantics). This script
# is the one that proves the whole path over real HTTP.
#
# WHAT THIS SCRIPT DOES NOT PROVE: convergence under redelivery.
# The tap's at-least-once story rests on `version_type: external` — a
# redelivered action must be a no-op at the target. xerj's own `_bulk` IGNORES
# per-action `version` / `version_type` (only the single-doc path honours them,
# es_compat.rs:2990), so a xerj target is precisely the target that does not
# implement the mechanism, and running two xerj nodes cannot exercise it.
# That claim is covered instead by
# `redelivery_converges_at_a_target_that_honours_external_versioning` and
# `a_target_that_rejects_every_action_is_reported_unhealthy_with_real_lag` in
# wal_tap_push.rs, against a stub that implements the ES rule. Against an
# Elasticsearch or OpenSearch target the mechanism is live; against a xerj
# target the tap degrades to last-write-wins by arrival, as
# `xerj-engine/src/wal_tap.rs` documents.
#
#   cargo build --release -p xerj-server
#   scripts/verify-wal-tap.sh engine/target/release/xerj
set -uo pipefail

BIN="${1:?usage: verify-wal-tap.sh <path-to-xerj-binary>}"
ROOT="$(mktemp -d /tmp/i320-verify.XXXXXX)"
SRC=9350
DST=9360
FAIL=0

cleanup() {
  [[ -n "${SRC_PID:-}" ]] && kill "$SRC_PID" 2>/dev/null
  [[ -n "${DST_PID:-}" ]] && kill "$DST_PID" 2>/dev/null
  wait 2>/dev/null
}
trap cleanup EXIT

check() { # check <label> <expected> <actual>
  if [[ "$2" == "$3" ]]; then echo "   PASS  $1"; else
    echo "   FAIL  $1: expected [$2], got [$3]"; FAIL=1
  fi
}

mkdir -p "$ROOT/source/data" "$ROOT/target/data"
cat > "$ROOT/target/xerj.toml" <<EOF
[server]
es_compat_port = $DST
rest_port = 9361
grpc_port = 9362
data_dir = "$ROOT/target/data"
bind_address = "127.0.0.1"
[auth]
enabled = false
EOF
cat > "$ROOT/source/xerj.toml" <<EOF
[server]
es_compat_port = $SRC
rest_port = 9351
grpc_port = 9352
data_dir = "$ROOT/source/data"
bind_address = "127.0.0.1"
[auth]
enabled = false

[wal_tap]
enabled          = true
target_url       = "http://127.0.0.1:$DST"
indices          = ["edge-*"]
poll_interval_ms = 200
EOF

"$BIN" --config "$ROOT/target/xerj.toml" > "$ROOT/target.log" 2>&1 &
DST_PID=$!
"$BIN" --config "$ROOT/source/xerj.toml" > "$ROOT/source.log" 2>&1 &
SRC_PID=$!
for port in $SRC $DST; do
  for _ in $(seq 1 60); do
    curl -sf -m 2 "http://127.0.0.1:$port/_cluster/health" >/dev/null && break
    sleep 1
  done
done
echo "== source :$SRC (tap on, allowlist edge-*) -> target :$DST | logs in $ROOT"

count() { curl -s "http://127.0.0.1:$1/$2/_count" | sed -n 's/.*"count":\([0-9]*\).*/\1/p'; }

# ── 1. an allowlisted index flows ───────────────────────────────────────────
for i in 1 2 3; do
  curl -s -XPUT "http://127.0.0.1:$SRC/edge-logs/_doc/$i" \
    -H 'Content-Type: application/json' -d "{\"msg\":\"hello $i\"}" >/dev/null
done
# ── 2. a non-allowlisted index must not ─────────────────────────────────────
curl -s -XPUT "http://127.0.0.1:$SRC/orders/_doc/o1" \
  -H 'Content-Type: application/json' -d '{"total":42}' >/dev/null

sleep 3
curl -s -XPOST "http://127.0.0.1:$DST/edge-logs/_refresh" >/dev/null
echo
echo "-- push"
check "3 docs reached the target" "3" "$(count $DST edge-logs)"
check "a non-allowlisted index did not" "" "$(count $DST orders)"

# ── 3. deletes travel ───────────────────────────────────────────────────────
curl -s -XDELETE "http://127.0.0.1:$SRC/edge-logs/_doc/2" >/dev/null
sleep 3
curl -s -XPOST "http://127.0.0.1:$DST/edge-logs/_refresh" >/dev/null
check "a delete propagated" "2" "$(count $DST edge-logs)"

# ── 4. system indices never leave ───────────────────────────────────────────
echo
echo "-- system indices"
sys_on_target=$(curl -s "http://127.0.0.1:$DST/_cat/indices?h=index" | grep -c '^\.xerj' || true)
sys_bootstrapped=$(curl -s "http://127.0.0.1:$DST/_cat/indices?h=index" | grep '^\.xerj' | wc -l)
echo "   (the target bootstraps its own $sys_bootstrapped system indices; the test is \
whether the SOURCE's docs are in them)"
check "no source doc landed in a target system index" "0" \
  "$(curl -s "http://127.0.0.1:$DST/.xerj_users/_count" | sed -n 's/.*"count":\([0-9]*\).*/\1/p')"

# ── 5. the runtime REST surface ─────────────────────────────────────────────
echo
echo "-- REST surface"
cfg=$(curl -s "http://127.0.0.1:$SRC/_xerj/wal_tap")
echo "   GET /_xerj/wal_tap -> $cfg"
check "the credential is never echoed back" "0" "$(grep -c 'target_auth"' <<<"$cfg")"

curl -s -XPUT "http://127.0.0.1:$SRC/_xerj/wal_tap" -H 'Content-Type: application/json' \
  -d '{"indices":["edge-*","metrics"]}' >/dev/null
curl -s -XPUT "http://127.0.0.1:$SRC/metrics/_doc/m1" \
  -H 'Content-Type: application/json' -d '{"cpu":0.5}' >/dev/null
sleep 3
curl -s -XPOST "http://127.0.0.1:$DST/metrics/_refresh" >/dev/null
check "an index added at runtime starts shipping" "1" "$(count $DST metrics)"

bad=$(curl -s -o /dev/null -w "%{http_code}" -XPUT "http://127.0.0.1:$SRC/_xerj/wal_tap" \
  -H 'Content-Type: application/json' -d '{"indices":[".xerj_users"]}')
check "a system-index pattern is refused" "400" "$bad"

echo
echo "-- stats"
curl -s "http://127.0.0.1:$SRC/_xerj/wal_tap/_stats" | head -c 900; echo

gaps=$(curl -s "http://127.0.0.1:$SRC/_xerj/wal_tap/_stats" | grep -o '"gaps":[0-9]*' | \
  awk -F: '{s+=$2} END {print s+0}')
check "no gaps under a keeping-up tap" "0" "$gaps"

echo
[[ $FAIL -eq 0 ]] && echo "== ALL CHECKS PASSED" || echo "== FAILURES PRESENT"
exit $FAIL
