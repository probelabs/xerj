#!/usr/bin/env bash
# Regression tests for xc-corpus.sh — the manifest round-trip (issue #319).
#
# A corpus manifest is only shareable if it can rebuild the corpus it describes.
# These tests pin that contract: full SHAs go in, the same bytes come back out,
# on a machine that has never seen the corpus.
#
# Offline by design. Fixtures are local git repositories served over file://,
# so nothing here touches the network or the real ~/.xerj-code.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
XC_CORPUS="$HERE/../scripts/xc-corpus.sh"

pass=0; fail=0
ok()   { pass=$((pass+1)); echo "  ok   — $1"; }
bad()  { fail=$((fail+1)); echo "  FAIL — $1"; }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Fixture: a repo with two commits, so "pinned to the parent" is distinguishable
# from "whatever HEAD happens to be". uploadpack.allowAnySHA1InWant mirrors what
# github.com already permits — the issue verified fetch-by-full-SHA against it.
make_repo() {
  local dir="$TMP/remote/$1" body="$2"
  mkdir -p "$dir"
  git -C "$dir" init -q -b main
  git -C "$dir" config user.email t@example.invalid
  git -C "$dir" config user.name  test
  git -C "$dir" config uploadpack.allowAnySHA1InWant true
  printf 'MIT License\n\nPermission is hereby granted, free of charge, ...\n' > "$dir/LICENSE"
  printf '%s\n' "$body" > "$dir/lib.rs"
  git -C "$dir" add -A && git -C "$dir" commit -qm one
  printf 'fn added_after_the_pin() {}\n' >> "$dir/lib.rs"
  git -C "$dir" add -A && git -C "$dir" commit -qm two
}

# Every invocation gets its own XERJ_CODE_HOME so tests cannot leak into each
# other, and can never write to the developer's real corpora.
xc() {
  local home="$1"; shift
  XERJ_CODE_HOME="$home" "$XC_CORPUS" "$@" >"$TMP/out.log" 2>&1
}

make_repo alpha 'fn alpha() {}'
make_repo beta  'fn beta() {}'
ALPHA_URL="file://$TMP/remote/alpha"
BETA_URL="file://$TMP/remote/beta"
ALPHA_TIP="$(git -C "$TMP/remote/alpha" rev-parse HEAD)"
ALPHA_PIN="$(git -C "$TMP/remote/alpha" rev-parse HEAD~1)"

echo "test_xc_corpus.sh"

# ---------------------------------------------------------------------------
# 1. The manifest records a FULL 40-char SHA.
#
# `git rev-parse --short HEAD` produced a 7-char abbreviation, which no remote
# will serve: `git fetch --depth 1 origin e449d17` fails with "couldn't find
# remote ref". A manifest that cannot be fetched from cannot rebuild anything.
# ---------------------------------------------------------------------------
H1="$TMP/home1"
if xc "$H1" demo "$ALPHA_URL" "$BETA_URL"; then
  M="$H1/corpora/demo/corpus.json"
  sha="$(grep -o '"repo":"alpha"[^}]*"sha":"[0-9a-f]*"' "$M" | grep -o '"sha":"[0-9a-f]*"' | cut -d'"' -f4)"
  check "manifest records a 40-char sha" "${#sha}" "40"
  check "manifest sha is the cloned commit" "$sha" "$ALPHA_TIP"
else
  bad "clone-by-url failed"; cat "$TMP/out.log"
fi

# ---------------------------------------------------------------------------
# 2. `--from <manifest>` rebuilds the corpus on a fresh machine, at the pinned
#    SHA — not at the remote's current tip. Same manifest in, same bytes out.
# ---------------------------------------------------------------------------
cat > "$TMP/pinned.json" <<EOF
{"corpus":"pinned","cloned_at":"2026-08-12T00:00:00Z","repos":[
  {"repo":"alpha","url":"$ALPHA_URL","licence":"MIT","sha":"$ALPHA_PIN","files":2,"bytes":0},
  {"repo":"beta","url":"$BETA_URL","licence":"MIT","sha":"$(git -C "$TMP/remote/beta" rev-parse HEAD)","files":2,"bytes":0}
]}
EOF

H2="$TMP/home2"
if xc "$H2" --from "$TMP/pinned.json"; then
  got="$(git -C "$H2/corpora/pinned/alpha" rev-parse HEAD 2>/dev/null || echo MISSING)"
  check "--from checks out the pinned sha, not the tip" "$got" "$ALPHA_PIN"
  # The pinned commit predates the second commit, so the marker must be absent.
  if grep -q added_after_the_pin "$H2/corpora/pinned/alpha/lib.rs" 2>/dev/null; then
    bad "--from produced tip content at a pinned sha"
  else
    ok "--from produced the pinned working tree"
  fi
  check "--from rebuilt every repo in the manifest" \
        "$(git -C "$H2/corpora/pinned/beta" rev-parse HEAD 2>/dev/null || echo MISSING)" \
        "$(git -C "$TMP/remote/beta" rev-parse HEAD)"
else
  bad "--from <manifest> failed"; sed 's/^/    /' "$TMP/out.log"
fi

# ---------------------------------------------------------------------------
# 3. An existing clone is MOVED to the recorded SHA, not silently skipped.
#
# Old behaviour printed "[skip] alpha already cloned" and left whatever was on
# disk — so re-running against a shared manifest gave two people different code.
# ---------------------------------------------------------------------------
H3="$TMP/home3"
mkdir -p "$H3/corpora/pinned"
git clone -q "$TMP/remote/alpha" "$H3/corpora/pinned/alpha"
if xc "$H3" --from "$TMP/pinned.json"; then
  check "existing clone is moved to the pinned sha" \
        "$(git -C "$H3/corpora/pinned/alpha" rev-parse HEAD)" "$ALPHA_PIN"
else
  bad "--from over an existing clone failed"; sed 's/^/    /' "$TMP/out.log"
fi

# ---------------------------------------------------------------------------
# 4. Round trip: build from URLs, then rebuild elsewhere from the manifest that
#    build produced. This is the actual shareability claim.
# ---------------------------------------------------------------------------
H4="$TMP/home4"
if xc "$H4" --from "$H1/corpora/demo/corpus.json"; then
  check "round-trip reproduces the recorded sha" \
        "$(git -C "$H4/corpora/demo/alpha" rev-parse HEAD 2>/dev/null || echo MISSING)" \
        "$ALPHA_TIP"
else
  bad "round-trip rebuild from a generated manifest failed"; sed 's/^/    /' "$TMP/out.log"
fi

# ---------------------------------------------------------------------------
# 5. A manifest whose SHA is short (every manifest written before this change)
#    must fail loudly rather than silently rebuild at the tip.
# ---------------------------------------------------------------------------
sed "s/$ALPHA_PIN/${ALPHA_PIN:0:7}/" "$TMP/pinned.json" > "$TMP/legacy.json"
H5="$TMP/home5"
if xc "$H5" --from "$TMP/legacy.json"; then
  bad "short-sha manifest was accepted"
else
  if grep -qi 'sha' "$TMP/out.log"; then
    ok "short-sha manifest is rejected with a sha-specific error"
  else
    bad "short-sha manifest rejected, but the error never mentions the sha"
  fi
fi

# ---------------------------------------------------------------------------
# 6. Licence detection. A manifest published under the project's name carries a
#    licence column, so a misread is a legal problem, not a cosmetic one. Both
#    cases below are real corpus repos the detector got wrong.
# ---------------------------------------------------------------------------
# shellcheck disable=SC1090
source <(sed -n '/^detect_licence()/,/^}/p;/^classify_one()/,/^}/p;/^classify_text()/,/^}/p' "$XC_CORPUS")

LIC="$TMP/lic"; mkdir -p "$LIC/mpl" "$LIC/es" "$LIC/qw" "$LIC/dual"

# MPL-2.0 names the GNU GPL inside its own definitions ("Secondary License"),
# so restrictive-first body matching read sonic's plain MPL file as GPL.
cat > "$LIC/mpl/LICENSE.md" <<'EOF'
Mozilla Public License Version 2.0
==================================

1. Definitions
--------------

1.12. "Secondary License"
    means either the GNU General Public License, Version 2.0, the GNU Lesser
    General Public License, Version 2.1, or the GNU Affero General Public
    License, Version 3.0.
EOF
check "MPL-2.0 is not read as GPL" "$(detect_licence "$LIC/mpl")" "MPL-2.0"

# The one that must never regress in the permissive direction.
cat > "$LIC/es/LICENSE.txt" <<'EOF'
Source code in this repository is covered by (i) a triple license under the
"GNU Affero General Public License v3.0 only", "the Server Side Public License,
v 1", and the "Elastic License 2.0" or (ii) an "Apache License 2.0" compatible
license.
EOF
check "triple-licensed source is read as AGPL, not Apache" "$(detect_licence "$LIC/es")" "AGPL"

# LICENSE-3rdparty.csv is an inventory of DEPENDENCY licences (quickwit); it
# used to add a spurious UNKNOWN arm to a plain Apache-2.0 repo.
printf 'Apache License\nVersion 2.0, January 2004\n' > "$LIC/qw/LICENSE"
printf 'Component,Origin,License\nfoo,https://example.invalid,MIT OR Apache-2.0\n' > "$LIC/qw/LICENSE-3rdparty.csv"
check "third-party inventories are not classified" "$(detect_licence "$LIC/qw")" "Apache-2.0"

# Dual licensing is reported as both, never collapsed to one.
printf 'Apache License\nVersion 2.0, January 2004\n' > "$LIC/dual/LICENSE-APACHE"
printf 'MIT License\n\nPermission is hereby granted, free of charge, ...\n' > "$LIC/dual/LICENSE-MIT"
check "dual licence is reported as both" "$(detect_licence "$LIC/dual")" "Apache-2.0/MIT"

# An empty LICENSE is a degenerate but real case (a placeholder committed before
# the text). grep exits 1 on it, and under `set -eo pipefail` that used to take
# the whole script down mid-corpus instead of recording UNKNOWN.
mkdir -p "$LIC/empty"; : > "$LIC/empty/LICENSE"
check "an empty licence file does not abort the run" "$(detect_licence "$LIC/empty")" "UNKNOWN"

# ---------------------------------------------------------------------------
# 7. The vetted manifests shipped in hub/ are well-formed and reviewable.
# ---------------------------------------------------------------------------
for m in "$HERE"/../hub/*.json; do
  [ -f "$m" ] || continue
  n="$(basename "$m")"
  if python3 "$HERE/validate_manifest.py" --hub "$m" 2>"$TMP/hub.err"; then
    ok "hub/$n is a valid, fully pinned, human-reviewed manifest"
  else
    bad "hub/$n: $(tr '\n' ';' < "$TMP/hub.err")"
  fi
done

# The validator itself has to reject what it claims to catch, or the hub check
# above is decoration.
cat > "$TMP/bad.json" <<'EOF'
{"corpus":"bad","repos":[{"repo":"x","url":"https://example.invalid/x","licence":"MIT","sha":"e449d17"}]}
EOF
if python3 "$HERE/validate_manifest.py" "$TMP/bad.json" 2>/dev/null; then
  bad "validate_manifest.py accepted a short sha"
else
  ok "validate_manifest.py rejects a short sha"
fi

# ---------------------------------------------------------------------------
# 8. A manifest is UNTRUSTED INPUT — `--from` exists to run files other people
#    wrote. `repo` becomes a path that is force-checked-out and `clean -fd`ed,
#    so a traversing name must be refused before any of that happens.
#
# Verified reachable before the guard: a manifest with
# repo="../../../victim" moved an unrelated git checkout to the manifest's
# commit, deleted its uncommitted changes and its untracked files, and
# rewrote its `origin` remote.
# ---------------------------------------------------------------------------
VICTIM="$TMP/victim"
mkdir -p "$VICTIM"
git -C "$VICTIM" init -q -b main
git -C "$VICTIM" config user.email t@example.invalid
git -C "$VICTIM" config user.name test
git -C "$VICTIM" remote add origin https://example.invalid/untouched
printf 'committed\n' > "$VICTIM/tracked.txt"
git -C "$VICTIM" add -A && git -C "$VICTIM" commit -qm base
printf 'uncommitted work\n' >> "$VICTIM/tracked.txt"
printf 'untracked\n' > "$VICTIM/untracked.txt"

# home6/corpora/<name> is three levels below $TMP, so ../../../victim lands on it.
cat > "$TMP/traversal.json" <<EOF
{"corpus":"trav","cloned_at":"2026-08-12T00:00:00Z","repos":[
  {"repo":"../../../victim","url":"$ALPHA_URL","licence":"MIT","sha":"$ALPHA_PIN","files":2,"bytes":0}
]}
EOF
H6="$TMP/home6"
if xc "$H6" --from "$TMP/traversal.json"; then
  bad "a traversing repo name was accepted"
else
  ok "a traversing repo name is rejected"
fi
check "traversal did not touch the victim's working tree" \
      "$(cat "$VICTIM/tracked.txt" 2>/dev/null)" "$(printf 'committed\nuncommitted work')"
check "traversal did not delete the victim's untracked files" \
      "$([ -f "$VICTIM/untracked.txt" ] && echo present || echo GONE)" "present"
check "traversal did not rewrite the victim's origin remote" \
      "$(git -C "$VICTIM" remote get-url origin)" "https://example.invalid/untouched"

cat > "$TMP/trav-hub.json" <<'EOF'
{"corpus":"trav-hub","repos":[{"repo":"../evil","url":"https://example.invalid/x","licence":"MIT",
 "sha":"0000000000000000000000000000000000000000",
 "review":{"spdx":"MIT","use":"approach-only","by":"t","at":"2026-08-12"}}]}
EOF
if python3 "$HERE/validate_manifest.py" "$TMP/trav-hub.json" 2>/dev/null; then
  bad "validate_manifest.py accepted a traversing repo name"
else
  ok "validate_manifest.py rejects a traversing repo name"
fi

# ---------------------------------------------------------------------------
# 9. xc-index.sh must not accept an instruction and drop it. `--frsh` used to
#    set no flag and exit 0, running an INCREMENTAL index — and autoindex's
#    incremental state makes that skip every file, leaving a corpus that
#    retrieves nothing with no error anywhere.
# ---------------------------------------------------------------------------
XC_INDEX="$HERE/../scripts/xc-index.sh"
mkdir -p "$TMP/home7/corpora/demo"
if XERJ_CODE_HOME="$TMP/home7" "$XC_INDEX" demo --frsh >"$TMP/idx.log" 2>&1; then
  bad "xc-index.sh accepted an unknown argument"
elif grep -q "unknown argument '--frsh'" "$TMP/idx.log"; then
  ok "xc-index.sh rejects an unknown argument by name"
else
  bad "xc-index.sh rejected '--frsh' but never named it: $(tr '\n' ';' < "$TMP/idx.log")"
fi

# ---------------------------------------------------------------------------
# 10. xc.py warns on every licence xc-corpus.sh warns about. The retrieval tool
#     is where an agent is actually looking at the code, so a gap here is the
#     one that matters. An exact-match ("GPL","LGPL") test was silent on the
#     AGPL, BUSL and (after the detector fix) MPL-2.0 repos the hub ships.
# ---------------------------------------------------------------------------
lic_warn="$(python3 - "$HERE/../scripts/xc.py" <<'PY'
import importlib.util, sys
spec = importlib.util.spec_from_file_location("xc", sys.argv[1])
xc = importlib.util.module_from_spec(spec); spec.loader.exec_module(xc)
warn = lambda l: bool(l) and any(r in l for r in xc.RESTRICTED)
must   = ["AGPL", "SSPL", "Elastic", "BUSL", "GPL", "LGPL", "MPL-2.0",
          "BUSL/MIT", "UNKNOWN", "NONE-FOUND", "Apache-2.0/UNKNOWN"]
mustnt = ["MIT", "Apache-2.0", "BSD", "Apache-2.0/MIT", ""]
bad = [l for l in must if not warn(l)] + [l for l in mustnt if warn(l)]
print("clean" if not bad else "wrong on " + ",".join(repr(b) for b in bad))
PY
)"
check "xc.py warns on every restricted licence and no permissive one" "$lic_warn" "clean"

# ---------------------------------------------------------------------------
# 11. A rebuild that dropped a repository must not exit 0. The documented idiom
#     is `xc-corpus.sh --from hub/<name>.json && xc-index.sh <name>`, so exit 0
#     on a partial corpus indexes something that is not what the manifest
#     describes while the "same manifest in, same bytes out" claim still reads
#     as having held.
# ---------------------------------------------------------------------------
cat > "$TMP/partial.json" <<EOF
{"corpus":"partial","cloned_at":"2026-08-12T00:00:00Z","repos":[
  {"repo":"alpha","url":"$ALPHA_URL","licence":"MIT","sha":"$ALPHA_PIN","files":2,"bytes":0},
  {"repo":"gone","url":"file://$TMP/remote/does-not-exist","licence":"MIT","sha":"$ALPHA_PIN","files":2,"bytes":0}
]}
EOF
H8="$TMP/home8"
if xc "$H8" --from "$TMP/partial.json"; then
  bad "a partial rebuild exited 0"
else
  ok "a partial rebuild exits non-zero"
fi
# The repo that DID land is still checked out, and the manifest still describes
# the real state — only the exit code says the corpus is not complete.
check "the repos that did land are still checked out" \
      "$(git -C "$H8/corpora/partial/alpha" rev-parse HEAD 2>/dev/null || echo MISSING)" "$ALPHA_PIN"
if grep -q 'INCOMPLETE' "$TMP/out.log"; then
  ok "a partial rebuild says which repos are missing"
else
  bad "a partial rebuild failed without naming the gap"
fi

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
