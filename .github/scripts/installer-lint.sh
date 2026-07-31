#!/usr/bin/env bash
# Syntax gate for the two install scripts served from xerj.org — `curl | sh`
# and `irm | iex` are the first thing a new user runs, so a parse error there
# is a broken front door that no other job would catch (neither file is Rust,
# neither is executed by any test).
#
# Deliberately cheap and tool-optional: the parse checks below are the floor
# and always run; shellcheck (POSIX sh) and pwsh (PowerShell AST) sharpen them
# when the runner happens to have them, and are skipped — never failed — when
# it does not.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SH="$REPO/landing/get"
PS="$REPO/landing/get.ps1"
ERR="$(mktemp)"
trap 'rm -f "$ERR"' EXIT

FAIL=0
note() { echo "  $*"; }
bad()  { echo "::error::$*"; FAIL=1; }

echo "── installer lint ──"

for f in "$SH" "$PS"; do
  [ -f "$f" ] || bad "installer missing: $f"
done
[ "$FAIL" = 0 ] || exit 1

# ── landing/get — POSIX sh ────────────────────────────────────────────────
if sh -n "$SH" 2>"$ERR"; then
  note "PASS: sh -n landing/get"
else
  bad "landing/get is not valid POSIX sh"
  sed 's/^/    /' "$ERR"
fi

# The script is piped into `sh`, so a bashism that a bash-as-sh runner happily
# accepts still breaks users on dash/ash. Those are SC3xxx, reported at warning
# severity — hence -S warning rather than errors-only. Style notes stay advisory.
if command -v shellcheck >/dev/null 2>&1; then
  if shellcheck -s sh -S warning "$SH"; then
    note "PASS: shellcheck -s sh (warnings and up)"
  else
    bad "shellcheck reported errors in landing/get"
  fi
else
  note "SKIP: shellcheck not installed"
fi

# ── landing/get.ps1 — PowerShell ──────────────────────────────────────────
# The path travels in the environment: `pwsh -Command` appends trailing argv
# to the command text rather than binding it to $args.
if command -v pwsh >/dev/null 2>&1; then
  if XERJ_PS_FILE="$PS" pwsh -NoProfile -NonInteractive -Command '
      $errors = $null
      [void][System.Management.Automation.Language.Parser]::ParseFile(
        $env:XERJ_PS_FILE, [ref]$null, [ref]$errors)
      if ($errors.Count -gt 0) { $errors | ForEach-Object { $_.ToString() }; exit 1 }
      exit 0'; then
    note "PASS: PowerShell parser accepts landing/get.ps1"
  else
    bad "landing/get.ps1 has PowerShell parse errors"
  fi
else
  note "SKIP: pwsh not installed"
fi

echo "──"
if [ "$FAIL" = 0 ]; then
  echo "INSTALLER LINT PASSED"
else
  echo "INSTALLER LINT FAILED"
  exit 1
fi
