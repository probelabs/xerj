#!/usr/bin/env python3
"""Lint the shell that ships inside docs/recipes/*.md and its published copy.

Two properties, each of which has already been violated in this repo:

1.  No block chains an integrity check into the action it is supposed to gate
    with ``&&`` while relying on ``set -e``.  ``set -e`` does not stop a command
    that is part of an ``&&`` list unless it is the last one, so::

        set -eu
        sha256sum -c "$ASSET.sha256" \\
          && tar -xzf "$ASSET"

    exits 0 on a bad digest and lets the caller continue into the next section.
    Writing each step on its own line is what makes errexit load-bearing.
    Only integrity verifiers are flagged: ``[ "$n" -eq 0 ] && break`` is
    ordinary flow control, not a gate that must fail closed.

2.  Every shell block in a recipe appears verbatim on the published page.  The
    two files are edited by hand and have drifted before: one copy of the
    air-gapped model block was guarded and the other was not, so a reader of
    xerj.org and a reader of the repo ran different code.  Line wrapping and
    HTML entities are normalised away; the sequence of commands is not.  The
    page may carry *extra* blocks — sample output is also in ``<pre>`` — so this
    is containment, not equality.

``bash -n`` is used to tell shell from sample output on the published page,
which has no language marker.  Blocks in the markdown are trusted to be shell
because the author wrote ```` ```bash ```` and are parsed as such.
"""

from __future__ import annotations

import html
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
MD_DIR = REPO / "docs" / "recipes"
HTML_DIR = REPO / "landing" / "docs" / "recipes"

MD_BLOCK = re.compile(r"^```(?:bash|sh)\n(.*?)^```$", re.S | re.M)
HTML_BLOCK = re.compile(r'<pre class="code">(.*?)</pre>', re.S)

# Commands whose whole purpose is to refuse the next step.
VERIFIERS = ("sha256sum", "sha512sum", "shasum", "sha256", "gpg", "gpgv", "cosign", "cmp")

# Recipes whose published page must carry every command in the markdown.
#
# This is not yet the whole tree.  Several pages legitimately predate their
# recipe or were edited independently and are missing blocks outright; making
# them match is editorial work, not a lint fix, and doing it silently here would
# hide how much of the tree is unchecked.  The recipes listed below are the ones
# where a divergence is a *security* difference rather than a formatting one, so
# they are held in lockstep now and the rest are reported as uncovered.
LOCKSTEP = frozenset({"air-gapped-deployment"})


def parses(text: str) -> tuple[bool, str]:
    proc = subprocess.run(
        ["bash", "-n"], input=text, text=True, capture_output=True, check=False
    )
    return proc.returncode == 0, proc.stderr.strip()


def normalise(text: str) -> list[str]:
    """Commands, stripped of the formatting the two copies may differ in."""
    joined = re.sub(r"\\\n\s*", " ", text)
    return [" ".join(line.split()) for line in joined.splitlines() if line.strip()]


def check_gate_chaining(text: str, where: str, errors: list[str]) -> None:
    if not re.search(r"^\s*set -[a-z]*e", text, re.M):
        return
    for stmt in re.sub(r"\\\n\s*", " ", text).splitlines():
        if "&&" not in stmt:
            continue
        head = stmt.strip().lstrip("( ").split()
        if head and head[0] in VERIFIERS:
            errors.append(
                f"{where}: `{head[0]}` gates the rest of an && list inside a "
                f"`set -e` block, which exempts it from errexit — put each step "
                f"on its own line\n      {stmt.strip()}"
            )


def main() -> int:
    errors: list[str] = []
    md_checked = 0
    paired = 0
    uncovered: list[str] = []

    for md_path in sorted(MD_DIR.glob("*.md")):
        md_rel = md_path.relative_to(REPO)
        md_blocks = MD_BLOCK.findall(md_path.read_text())

        for i, block in enumerate(md_blocks):
            where = f"{md_rel} block {i + 1}"
            ok, err = parses(block)
            if not ok:
                errors.append(f"{where}: does not parse\n      {err}")
            check_gate_chaining(block, where, errors)
            md_checked += 1

        html_path = HTML_DIR / (md_path.stem + ".html")
        if not html_path.exists():
            continue
        if md_path.stem not in LOCKSTEP:
            uncovered.append(md_path.stem)
            continue

        published = []
        for raw in HTML_BLOCK.findall(html_path.read_text()):
            block = html.unescape(raw)
            if parses(block)[0]:
                published.append(normalise(block))
                check_gate_chaining(block, f"{html_path.relative_to(REPO)}", errors)

        for i, block in enumerate(md_blocks):
            paired += 1
            if normalise(block) not in published:
                errors.append(
                    f"{md_rel} block {i + 1} is not on the published page "
                    f"{html_path.relative_to(REPO)} — the two copies have drifted "
                    f"and readers get different instructions\n"
                    f"      recipe: {normalise(block)}"
                )

    if errors:
        print(f"recipe-shell-lint: {len(errors)} problem(s)\n", file=sys.stderr)
        for e in errors:
            print(f"  {e}\n", file=sys.stderr)
        return 1

    print(
        f"recipe-shell-lint: {md_checked} recipe shell blocks OK, "
        f"{paired} checked against their published copy"
    )
    if uncovered:
        print(
            f"recipe-shell-lint: published copies NOT checked for drift "
            f"({len(uncovered)}): {', '.join(sorted(uncovered))}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
