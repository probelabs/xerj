#!/usr/bin/env bash
# Index a corpus into the local XERJ instance.
set -euo pipefail

ROOT="${XERJ_CODE_HOME:-$HOME/.xerj-code}"
CORPORA="$ROOT/corpora"
URL="${XERJ_URL:-http://localhost:9200}"
XERJ_BIN="${XERJ_BIN:-xerj}"

usage() { echo "usage: xc-index.sh <corpus-name> [--fresh]" >&2; exit 2; }

[ $# -ge 1 ] || usage
name="$1"; shift

# Reject what we do not understand rather than dropping it. The previous form
# was `[ "${1:-}" = "--fresh" ] && fresh="--fresh"`, which silently ignored a
# mistyped or extra flag: `xc-index.sh <corpus> --frsh` ran an INCREMENTAL
# index and exited 0. That is the worst possible shape for this particular
# flag — autoindex keeps incremental state in ~/.xerj/autoindex/, so a run that
# should have been fresh skips every file as "already indexed" and leaves a
# corpus with 0 documents that retrieves nothing, with no error anywhere.
fresh=""
while [ $# -gt 0 ]; do
  case "$1" in
    --fresh) fresh="--fresh"; shift ;;
    *) echo "xc-index: unknown argument '$1'" >&2; usage ;;
  esac
done

dir="$CORPORA/$name"
[ -d "$dir" ] || { echo "xc-index: no corpus '$name' — run xc-corpus.sh first" >&2; exit 2; }

command -v "$XERJ_BIN" >/dev/null 2>&1 || {
  echo "xc-index: '$XERJ_BIN' not on PATH. Set XERJ_BIN to the binary path." >&2; exit 2; }

curl -fsS -m 5 "$URL/_cluster/health" >/dev/null 2>&1 || {
  echo "xc-index: no XERJ at $URL. Start it, or set XERJ_URL." >&2; exit 2; }

echo "indexing corpus '$name' from $dir"

# --no-graph: reference code needs ranked passages, not a relationship map. The
# graph detectors cost real time on a large tree and nothing here consumes edges.
set +e
"$XERJ_BIN" autoindex "$dir" \
  --url "$URL" \
  --prefix "xc-$name" \
  --no-graph \
  $fresh
rc=$?
set -e

# Exit 3 is "completed, some files were unparseable" — expected on any real
# repository (binaries, fixtures, vendored blobs). Treat it as success.
case $rc in
  0) echo "indexed cleanly" ;;
  3) echo "indexed (exit 3: some files skipped as junk — normal for real repos)" ;;
  *) echo "xc-index: autoindex failed with exit $rc" >&2; exit $rc ;;
esac

mkdir -p "$ROOT/state"
cat > "$ROOT/state/$name.json" <<EOF
{"corpus":"$name","indexed_at":"$(date -u +%Y-%m-%dT%H:%M:%SZ)","prefix":"xc-$name","url":"$URL","autoindex_exit":$rc}
EOF

docs=$(curl -fsS -m 5 "$URL/xc-$name*/_count" 2>/dev/null \
       | sed 's/.*"count":\([0-9]*\).*/\1/' || echo "?")
echo "corpus '$name' searchable: $docs records"
echo "next: xc.py $name \"<what you need>\""
