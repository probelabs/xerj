#!/usr/bin/env bash
# Per-platform regression guard for the autoindex "Too many open files" crash.
#
# The crash: `xerj autoindex` on a large repo infers hundreds of datasets =>
# hundreds of indices. Before the fix each index pinned one WAL file descriptor
# PER ingest shard (a count that scales with CPU cores, ~8-16), so a few hundred
# datasets held thousands of fds open and aborted with EMFILE — fatal on macOS,
# whose default soft limit is 256.
#
# This gate constrains the descriptor budget so the regression is caught on ANY
# runner regardless of its `kern.maxfilesperproc`:
#   soft 256  — the macOS default; the server MUST raise it (raise_nofile_limit)
#   hard 4096 — between the pre-fix need (~6,400 for 400 datasets) and the
#               post-fix need (~600). Pre-fix => EMFILE; post-fix => fits.
# So the job fails if EITHER the limit-raise OR the per-index WAL-fd reduction
# regresses. Runs under bash on ubuntu / macOS / windows(git-bash).
set -uo pipefail

# Best-effort FD budget (no-op on Windows, which uses a different handle model).
ulimit -Hn 4096 2>/dev/null || true
ulimit -Sn 256  2>/dev/null || true
echo "platform: $(uname -s)   ulimit -Sn: $(ulimit -Sn 2>/dev/null || echo n/a)   -Hn: $(ulimit -Hn 2>/dev/null || echo n/a)"

# Overridable so CI can point at a cheaper profile than fat-LTO release;
# defaults to the release path for local use.
BIN="${XERJ_BIN:-engine/target/release/xerj}"
[ -f "$BIN.exe" ] && BIN="$BIN.exe" # windows

# ── generate 400 distinct-schema datasets (one index each) ─────────────────
ROOT="$(mktemp -d)"
CORPUS="$ROOT/corpus"
mkdir -p "$CORPUS"
for i in $(seq 1 400); do
  d="$CORPUS/ds_$i"
  mkdir -p "$d"
  # Unique column names per folder => a distinct inferred dataset per folder.
  printf 'k_%d_id,k_%d_name,k_%d_val\n1,alpha,10\n2,beta,20\n3,gamma,30\n' "$i" "$i" "$i" > "$d/data.csv"
done
echo "generated $(find "$CORPUS" -name '*.csv' | wc -l | tr -d ' ') dataset files"

# ── boot the server (inherits the constrained fd budget) ───────────────────
DATA="$ROOT/data"
mkdir -p "$DATA"
"$BIN" --insecure --data-dir "$DATA" > "$DATA/server.log" 2>&1 &
SPID=$!
up=0
for _ in $(seq 1 160); do
  if curl -fs -m1 localhost:9200/_cluster/health >/dev/null 2>&1; then up=1; break; fi
  kill -0 "$SPID" 2>/dev/null || { echo "server exited during boot"; cat "$DATA/server.log"; exit 1; }
  sleep 0.5
done
[ "$up" = 1 ] || { echo "server never became healthy"; cat "$DATA/server.log"; exit 1; }

# ── write/read round-trip on the platform's own filesystem ─────────────────
# Index creation runs the durable-publish chain (write tmp → fsync → rename →
# fsync parent dir), which is where per-platform filesystem semantics bite:
# on Windows `fsync_dir` used to fail with ERROR_ACCESS_DENIED for every call,
# so the server aborted at boot and no index could ever be created. Autoindex
# below would catch that too, but only as a vague "server exited" — assert the
# round-trip explicitly so the failure names itself.
curl -fs -m10 -XPUT localhost:9200/smoke-crud -H 'content-type: application/json' \
  -d '{"settings":{"number_of_shards":1}}' > "$DATA/crud.log" 2>&1 \
  || { echo "::error::create index failed on $(uname -s)"; cat "$DATA/crud.log"; cat "$DATA/server.log"; exit 1; }
curl -fs -m10 -XPOST 'localhost:9200/smoke-crud/_doc/1?refresh=true' -H 'content-type: application/json' \
  -d '{"title":"windows durability round trip"}' >> "$DATA/crud.log" 2>&1 \
  || { echo "::error::index doc failed on $(uname -s)"; cat "$DATA/crud.log"; cat "$DATA/server.log"; exit 1; }
HITS="$(curl -s -m10 'localhost:9200/smoke-crud/_search?q=durability' | tr -d ' \n' | sed -n 's/.*"value":\([0-9]*\).*/\1/p' | head -1)"
[ "${HITS:-0}" = "1" ] \
  || { echo "::error::search round-trip returned '${HITS:-}' hits, expected 1"; cat "$DATA/server.log"; exit 1; }
echo "write/read round-trip OK on $(uname -s)"

# ── autoindex the corpus ───────────────────────────────────────────────────
"$BIN" autoindex "$CORPUS" > "$DATA/ax.log" 2>&1
RC=$?
echo "autoindex exit=$RC"
tail -20 "$DATA/ax.log" || true

# ── assertions ─────────────────────────────────────────────────────────────
FAIL=0
if grep -qiE 'too many open files|os error 24|emfile' "$DATA/ax.log" "$DATA/server.log"; then
  echo "::error::FD-exhaustion regression — autoindex hit 'Too many open files'"
  FAIL=1
fi
# autoindex exit codes: 0 complete, 3 completed-with-junk (both fine).
if [ "$RC" != "0" ] && [ "$RC" != "3" ]; then
  echo "::error::autoindex exited $RC (expected 0 or 3)"
  FAIL=1
fi
CREATED="$(curl -s 'localhost:9200/_cat/indices?h=index' 2>/dev/null | grep -c 'ax-' || true)"
echo "ax-* indices created: $CREATED"
if [ "${CREATED:-0}" -lt 300 ]; then
  echo "::error::expected >=300 datasets indexed, got ${CREATED:-0}"
  FAIL=1
fi

kill "$SPID" 2>/dev/null || true
if [ "$FAIL" = 0 ]; then
  echo "AUTOINDEX FD SMOKE PASSED on $(uname -s) ($CREATED datasets, no EMFILE)"
else
  echo "AUTOINDEX FD SMOKE FAILED on $(uname -s)"
  exit 1
fi
