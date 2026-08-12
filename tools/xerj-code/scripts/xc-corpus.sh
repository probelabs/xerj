#!/usr/bin/env bash
# Clone reference repositories into a named corpus.
#
# Two ways in:
#
#   xc-corpus.sh <name> <git-url>...   build a new corpus from URLs, and write
#                                      a manifest that describes it
#   xc-corpus.sh --from <manifest>     rebuild the corpus that manifest
#                                      describes, at the exact commits it pins
#
# The second form is what makes a corpus shareable. `corpus.json` is a few
# hundred bytes of URLs, SHAs and licences — no source, so no redistribution
# question — and the same manifest on two machines produces the same bytes on
# disk, which is what makes a shared prompt or a shared measurement comparable.
#
# Shallow clones only — history is dead weight for retrieval, and a full clone of
# a large project can be 10x the checkout. Licence is recorded per repo because
# copying from an incompatible one is a real problem, not a formality.
set -euo pipefail

ROOT="${XERJ_CODE_HOME:-$HOME/.xerj-code}"
CORPORA="$ROOT/corpora"

usage() {
  cat >&2 <<'EOF'
usage: xc-corpus.sh <corpus-name> <git-url> [git-url...]
       xc-corpus.sh --from <manifest.json> [--as <corpus-name>]

Groups repositories by PROBLEM DOMAIN, not by language. A corpus that contains
everything retrieves like a search engine with no query — keep them sharp.

  xc-corpus.sh async-rust https://github.com/tokio-rs/tokio \
                          https://github.com/hyperium/hyper

Rebuild a corpus someone else defined, pinned to the same commits:

  xc-corpus.sh --from hub/xerj-storage.json && xc-index.sh xerj-storage

--from is exact: each repo is fetched at the manifest's SHA, and an existing
clone is MOVED to that SHA (local edits and untracked files in it are
discarded). Vetted manifests live in this toolkit's hub/ directory.
EOF
  exit 2
}

# find the licence without guessing: read the file the project actually ships.
# Dual-licensed crates (very common in Rust) ship LICENSE-MIT + LICENSE-APACHE
# and no plain LICENSE, so collect every candidate rather than taking the first.
detect_licence() {
  local repo="$1" f found=()
  for f in "$repo"/LICENSE* "$repo"/LICENCE* "$repo"/COPYING*; do
    [ -f "$f" ] || continue
    # LICENSE-3rdparty.csv (quickwit) is an inventory of DEPENDENCY licences,
    # not this project's licence. Classifying it produced a spurious "UNKNOWN"
    # arm on an otherwise plain Apache-2.0 repo.
    case "$(basename "$f" | tr '[:upper:]' '[:lower:]')" in
      *3rdparty*|*third-party*|*thirdparty*) continue ;;
    esac
    found+=("$(classify_one "$f")")
  done
  if [ ${#found[@]} -eq 0 ]; then echo "NONE-FOUND"; return; fi
  # de-duplicate, join with "/" — "Apache-2.0/MIT" is the honest answer for a
  # dual-licensed project; collapsing it to one licence would be a false record.
  printf '%s\n' "${found[@]}" | sort -u | paste -sd/ -
}

# Classify a single licence file by its text. Order matters: check the most
# RESTRICTIVE phrases first, and keep "bsd" last because Apache and MIT texts
# can both mention BSD in passing.
#
# The restrictive-first ordering is not stylistic. Elasticsearch's LICENSE.txt
# is a triple licence (AGPL-3.0 / SSPL-1.0 / Elastic-2.0) whose text contains
# the phrase «an "Apache License 2.0" compatible license». With Apache checked
# first, the repo was recorded as "Apache-2.0" — the most permissive possible
# reading of the most restrictive licence in the corpus, on the one repo where
# being wrong matters most. A misclassification here is a legal problem, so
# when in doubt this must over-report restriction, never under-report it.
#
# Read the TITLE first, and only then the body. A licence file names itself in
# its opening lines, while its body may cite other licences: MPL-2.0 defines
# "Secondary License" as «the GNU General Public License, Version 2.0» inside
# its own text, so a body-only scan with restrictive-first ordering read
# sonic's plain MPL-2.0 file as GPL.
#
# Two lines, not one, because titles wrap ("Apache License" / "Version 2.0,
# January 2004"). Elasticsearch's triple licence wraps mid-name across exactly
# those two lines and still resolves to AGPL, so the safe direction is
# preserved where it matters most. When the title says nothing recognisable
# (a "# License" heading, a bare copyright line) this falls back to the body
# scan, which is the previous behaviour.
classify_one() {
  local title out
  # `|| true`: grep exits 1 on an empty licence file, and pipefail would
  # otherwise take the whole script down with it.
  title="$(grep -av '^[[:space:]]*$' "$1" 2>/dev/null | head -n 2 || true)"
  out="$(classify_text "$title")"
  [ "$out" = "UNKNOWN" ] && out="$(classify_text "$(head -c 4000 "$1")")"
  echo "$out"
}

classify_text() {
  local body
  body="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr '\n' ' ')"
  case "$body" in
    *"affero general public license"*)   echo "AGPL" ;;
    *"server side public license"*)      echo "SSPL" ;;
    *"elastic license"*)                 echo "Elastic" ;;
    *"business source license"*)         echo "BUSL" ;;
    *"gnu lesser general"*)              echo "LGPL" ;;
    *"gnu general public license"*)      echo "GPL" ;;
    *"mozilla public license"*)          echo "MPL-2.0" ;;
    *"apache license"*)                  echo "Apache-2.0" ;;
    *"mit license"*|*"permission is hereby granted, free of charge"*)
                                         echo "MIT" ;;
    *"redistribution and use in source and binary forms"*)
                                         echo "BSD" ;;
    *)                                   echo "UNKNOWN" ;;
  esac
}

# Copyleft and source-available licences are read-the-approach only. Warn on
# every one of them, not just GPL — the corpora deliberately include AGPL/SSPL
# (Elasticsearch) and BUSL (Meilisearch EE) sources.
warn_licence() {
  case "$1" in
    *AGPL*|*SSPL*|*Elastic*|*BUSL*|*GPL*|*LGPL*|*UNKNOWN*|*NONE-FOUND*)
      echo "          ^ $1: adapt the APPROACH, do not copy the code" >&2 ;;
  esac
}

# Read a manifest into "repo<TAB>url<TAB>sha<TAB>licence" lines. python3 is
# already required by xc.py, so this adds no new dependency — and a real JSON
# parser beats a regex on a file that carries licence data.
read_manifest() {
  python3 - "$1" <<'PY'
import json, re, sys
path = sys.argv[1]
try:
    with open(path) as fh:
        m = json.load(fh)
except FileNotFoundError:
    sys.exit(f"xc-corpus: no such manifest: {path}")
except json.JSONDecodeError as e:
    sys.exit(f"xc-corpus: {path} is not valid JSON: {e}")

repos = m.get("repos")
if not isinstance(repos, list) or not repos:
    sys.exit(f"xc-corpus: {path} has no 'repos' array")

for r in repos:
    name = r.get("repo") or ""
    url, sha = r.get("url") or "", r.get("sha") or ""
    if not name or not url:
        sys.exit(f"xc-corpus: {path}: an entry is missing 'repo' or 'url'")
    # A short sha cannot be fetched from a remote — `git fetch --depth 1 origin
    # e449d17` fails with "couldn't find remote ref". Manifests written before
    # full SHAs were recorded would otherwise rebuild silently at the tip,
    # which is the opposite of a pin. Fail loudly instead.
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        sys.exit(
            f"xc-corpus: {path}: repo '{name}' has sha '{sha}', which is not a "
            "full 40-character sha. A short sha is not fetchable from a remote, "
            "so this manifest cannot pin a rebuild. Regenerate it with "
            "`xc-corpus.sh <name> <url>...` on a machine that has the corpus."
        )
    print(f"{name}\t{url}\t{sha}\t{r.get('licence') or 'UNKNOWN'}")

print(m.get("corpus") or "", end="", file=sys.stderr)
PY
}

# Put `target` on exactly `sha`, fetching only what that needs.
#
# Fetch-by-full-SHA at depth 1 is cheap and is what github.com serves; the
# progressively deeper fallbacks cover remotes that refuse an arbitrary-SHA
# want (uploadpack.allowAnySHA1InWant off) or a commit that is not reachable
# from the default tip.
checkout_at_sha() {
  local target="$1" url="$2" sha="$3"

  if [ ! -d "$target/.git" ]; then
    mkdir -p "$target"
    git -C "$target" init -q
    git -C "$target" remote add origin "$url"
  else
    git -C "$target" remote set-url origin "$url" 2>/dev/null \
      || git -C "$target" remote add origin "$url"
  fi

  if ! git -C "$target" cat-file -e "$sha^{commit}" 2>/dev/null; then
    git -C "$target" fetch --depth 1 --quiet origin "$sha" 2>/dev/null \
      || git -C "$target" fetch --quiet origin "$sha" 2>/dev/null \
      || git -C "$target" fetch --quiet --tags origin 2>/dev/null \
      || true
  fi

  if ! git -C "$target" cat-file -e "$sha^{commit}" 2>/dev/null; then
    echo "  [FAIL] $(basename "$target") — remote has no commit $sha" >&2
    return 1
  fi

  # --force discards tracked edits; clean removes files left over from a
  # different commit. "Same manifest in, same bytes out" has to mean it.
  git -C "$target" checkout --quiet --force --detach "$sha"
  git -C "$target" clean -qfd
}

# ---------------------------------------------------------------------------
# argument parsing
# ---------------------------------------------------------------------------
manifest_in=""
name=""

if [ "${1:-}" = "--from" ]; then
  shift
  [ $# -ge 1 ] || usage
  manifest_in="$1"; shift
  while [ $# -gt 0 ]; do
    case "$1" in
      --as) shift; [ $# -ge 1 ] || usage; name="$1"; shift ;;
      *)    usage ;;
    esac
  done
else
  [ $# -ge 2 ] || usage
  name="$1"; shift
fi

entries=()

if [ -n "$manifest_in" ]; then
  # stderr of read_manifest carries the manifest's own corpus name; on failure
  # it carries the error, so keep both and let the exit status decide.
  meta_err="$(mktemp)"; trap 'rm -f "$meta_err"' EXIT
  if ! rows="$(read_manifest "$manifest_in" 2>"$meta_err")"; then
    cat "$meta_err" >&2
    exit 2
  fi
  [ -n "$name" ] || name="$(cat "$meta_err")"
  [ -n "$name" ] || { echo "xc-corpus: manifest has no 'corpus' name; pass --as <name>" >&2; exit 2; }
fi

case "$name" in
  */*|.*|"") echo "xc-corpus: bad corpus name '$name'" >&2; exit 2 ;;
esac

dest="$CORPORA/$name"
mkdir -p "$dest"
manifest="$dest/corpus.json"

# ---------------------------------------------------------------------------
# fetch
# ---------------------------------------------------------------------------
record() {
  local repo_name="$1" url="$2" target="$3" declared="${4:-}"
  local lic sha bytes files
  lic="$(detect_licence "$target")"
  sha="$(git -C "$target" rev-parse HEAD 2>/dev/null || echo unknown)"
  bytes="$(du -sb "$target" 2>/dev/null | cut -f1 || echo 0)"
  files="$(find "$target" -type f -not -path '*/.git/*' 2>/dev/null | wc -l)"
  echo "          licence=$lic sha=${sha:0:12} files=$files"
  warn_licence "$lic"
  # The detector is text-matching heuristics and has been wrong before. When a
  # manifest disagrees with the checkout, say so rather than overwrite quietly.
  if [ -n "$declared" ] && [ "$declared" != "$lic" ]; then
    echo "          ! manifest says licence=$declared, checkout reads $lic — verify before copying" >&2
  fi
  entries+=("{\"repo\":\"$repo_name\",\"url\":\"$url\",\"licence\":\"$lic\",\"sha\":\"$sha\",\"files\":$files,\"bytes\":$bytes}")
}

if [ -n "$manifest_in" ]; then
  echo "rebuilding corpus '$name' from $manifest_in"
  while IFS=$'\t' read -r repo_name url sha declared; do
    [ -n "$repo_name" ] || continue
    target="$dest/$repo_name"
    if [ -d "$target/.git" ] && [ "$(git -C "$target" rev-parse HEAD 2>/dev/null)" = "$sha" ]; then
      echo "  [ok] $repo_name already at ${sha:0:12}"
    else
      echo "  [pin] $repo_name -> ${sha:0:12}"
      checkout_at_sha "$target" "$url" "$sha" || continue
    fi
    record "$repo_name" "$url" "$target" "$declared"
  done <<< "$rows"
else
  for url in "$@"; do
    repo_name="$(basename "$url" .git)"
    target="$dest/$repo_name"

    if [ -d "$target/.git" ]; then
      echo "  [skip] $repo_name already cloned"
    else
      echo "  [clone] $repo_name"
      if ! git clone --depth 1 --quiet "$url" "$target"; then
        echo "  [FAIL] $repo_name — could not clone; continuing" >&2
        continue
      fi
    fi
    record "$repo_name" "$url" "$target"
  done
fi

if [ ${#entries[@]} -eq 0 ]; then
  echo "xc-corpus: nothing cloned" >&2
  exit 1
fi

# The manifest is regenerated from what is actually on disk — never copied
# through from the input — so it always describes this checkout.
{
  printf '{"corpus":"%s","cloned_at":"%s","repos":[\n' \
    "$name" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '  %s' "${entries[0]}"
  for e in "${entries[@]:1}"; do printf ',\n  %s' "$e"; done
  printf '\n]}\n'
} > "$manifest"

echo
echo "corpus '$name': ${#entries[@]} repos at $dest"
echo "share it: $manifest  (rebuild with xc-corpus.sh --from <that file>)"
echo "next: xc-index.sh $name"
