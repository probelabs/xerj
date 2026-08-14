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

# Recorded before the run so a failure can tell "this run wrote records" apart
# from "an earlier run's records are still lying around". Salvaging the latter
# would date stale data to now, which is the failure mode SKILL.md calls worse
# than no index at all.
docs_before="$(curl -fsS -m 5 "$URL/xc-$name*/_count" 2>/dev/null \
               | sed 's/.*"count":\([0-9]*\).*/\1/' || echo "")"
[ -n "$docs_before" ] || docs_before=0

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

# How many records actually landed. This is the ground truth about whether the
# corpus is usable, and it is the check SKILL.md already tells people to run by
# hand ("Always verify: curl -s "$URL/xc-<corpus>*/_count" must be > 0").
count_records() {
  curl -fsS -m 5 "$URL/xc-$name*/_count" 2>/dev/null \
    | sed 's/.*"count":\([0-9]*\).*/\1/' || echo ""
}

# Exit 3 is "completed, some files were unparseable" — expected on any real
# repository (binaries, fixtures, vendored blobs). Treat it as success.
#
# Any other non-zero code is NOT taken at face value, because autoindex can
# abort in finalisation *after* every document has been written — a whole-corpus
# read-back or generation-cutover reconciliation failure leaves a complete,
# queryable index behind and still exits 1 (see issue #367; observed on
# apache/lucene on both embedding modes, 6,032 java records live). Discarding
# that corpus sends the user back to a step that will fail again, so ask the
# engine what is actually there before deciding. An empty or unreachable index
# still fails, and a salvaged run is reported loudly rather than silently.
docs=""
salvaged=false
case $rc in
  0) echo "indexed cleanly" ;;
  3) echo "indexed (exit 3: some files skipped as junk — normal for real repos)" ;;
  *)
    docs="$(count_records)"
    if [ -n "$docs" ] && [ "$docs" -gt "$docs_before" ] 2>/dev/null; then
      salvaged=true
      echo "xc-index: WARNING — autoindex exited $rc, but this run wrote records" >&2
      echo "xc-index: ($docs_before -> $docs). The corpus is queryable and is being" >&2
      echo "xc-index: recorded as indexed, with autoindex_exit=$rc in its state file." >&2
      echo "xc-index: That exit usually means finalisation failed AFTER the writes." >&2
      echo "xc-index: Coverage is not guaranteed — please report the error above." >&2
    elif [ -n "$docs" ] && [ "$docs" -gt 0 ] 2>/dev/null; then
      echo "xc-index: autoindex failed with exit $rc and wrote no new records" >&2
      echo "xc-index: ($docs already present from an earlier run — not treating" >&2
      echo "xc-index: those as this run's output; a stale corpus dated to now is" >&2
      echo "xc-index: worse than none)." >&2
      exit $rc
    else
      echo "xc-index: autoindex failed with exit $rc and left no records" >&2
      exit $rc
    fi
    ;;
esac

mkdir -p "$ROOT/state"
cat > "$ROOT/state/$name.json" <<EOF
{"corpus":"$name","indexed_at":"$(date -u +%Y-%m-%dT%H:%M:%SZ)","prefix":"xc-$name","url":"$URL","autoindex_exit":$rc,"salvaged":$salvaged}
EOF

[ -n "$docs" ] || docs="$(count_records)"
[ -n "$docs" ] || docs="?"
echo "corpus '$name' searchable: $docs records"
echo "next: xc.py $name \"<what you need>\""
