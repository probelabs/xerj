#!/usr/bin/env python3
"""Validate a shareable corpus manifest.

A manifest published under the project's name is a claim about other people's
code: these URLs, at these commits, under these licences. This checks the parts
a machine can check, so review can spend its attention on the licence column.

usage: validate_manifest.py <manifest.json>...
       validate_manifest.py --hub <manifest.json>...   # also require review{}
"""

import json
import re
import sys

FULL_SHA = re.compile(r"[0-9a-f]{40}")
# `repo` is used as a directory name under the corpus root, which xc-corpus.sh
# then force-checks-out and cleans. A name containing '/' or '..' escapes the
# corpus and destroys whatever it lands on, so a manifest carrying one must
# never pass review — least of all one published under the project's name.
REPO_NAME = re.compile(r"[A-Za-z0-9_][A-Za-z0-9._-]*\Z")
USES = {"adapt-with-attribution", "approach-only", "mixed"}


def check(path: str, hub: bool) -> list[str]:
    errs: list[str] = []
    try:
        with open(path) as fh:
            m = json.load(fh)
    except (OSError, json.JSONDecodeError) as e:
        return [f"unreadable: {e}"]

    corpus = m.get("corpus")
    if not corpus:
        errs.append("no 'corpus' name")
    repos = m.get("repos")
    if not isinstance(repos, list) or not repos:
        return errs + ["'repos' must be a non-empty array"]

    if hub:
        stem = path.rsplit("/", 1)[-1].removesuffix(".json")
        if corpus != stem:
            errs.append(f"corpus '{corpus}' does not match filename '{stem}'")

    seen = set()
    for r in repos:
        name = r.get("repo") or "<unnamed>"
        if name in seen:
            errs.append(f"{name}: duplicate entry")
        seen.add(name)
        for k in ("repo", "url", "licence", "sha"):
            if not r.get(k):
                errs.append(f"{name}: missing '{k}'")
        if not REPO_NAME.match(str(r.get("repo") or "")):
            errs.append(
                f"{name}: repo name is not a plain directory name "
                "(letters, digits, '.', '_', '-'; not starting with '.' or '-')"
            )
        sha = r.get("sha") or ""
        # A short sha is not fetchable from a remote, so it cannot pin a
        # rebuild — the whole point of sharing the definition.
        if not FULL_SHA.fullmatch(sha):
            errs.append(f"{name}: sha '{sha}' is not a full 40-character sha")
        if not hub:
            continue
        if not (r.get("url") or "").startswith("https://"):
            errs.append(f"{name}: url must be https")
        rev = r.get("review") or {}
        if not rev.get("spdx"):
            errs.append(f"{name}: no human-reviewed 'review.spdx'")
        if rev.get("use") not in USES:
            errs.append(f"{name}: review.use must be one of {sorted(USES)}")
        if not (rev.get("by") and rev.get("at")):
            errs.append(f"{name}: review needs 'by' and 'at'")
    return errs


def main() -> int:
    args = sys.argv[1:]
    hub = "--hub" in args
    paths = [a for a in args if a != "--hub"]
    if not paths:
        print(__doc__, file=sys.stderr)
        return 2
    bad = 0
    for p in paths:
        errs = check(p, hub)
        if errs:
            bad += 1
            for e in errs:
                print(f"{p}: {e}", file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
