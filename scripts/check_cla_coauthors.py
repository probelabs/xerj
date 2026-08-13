#!/usr/bin/env python3
"""CLA check for Co-authored-by trailers (issue #269).

cla-bot resolves signatures by matching commit *authors* against
.contributors. When a maintainer re-lands a fork PR — squashing the
contributor's commits onto an xerj-org branch and crediting them with a
Co-authored-by trailer — every commit author is xerj-org, so the gate goes
green even though the person whose work it is has never signed (live
instance: PR #248, the re-land of buger's #166). This check closes that
hole: every Co-authored-by trailer on a PR must resolve to a login listed
in .contributors.

An email resolves to a login by, in order:

 1. GitHub noreply address — ``[id+]login@users.noreply.github.com``.
 2. This repo's own commit history — ``GET /repos/{repo}/commits?author=<email>``
    returns the login GitHub itself attributes that email to. The CLA
    signature flow ("open a PR adding your username to .contributors from
    your own account") lands a commit authored with the signer's email, so
    signing makes the signer resolvable as a side effect.

Anything unresolvable FAILS the gate, deliberately: an identity the gate
cannot attribute is exactly what it exists to flag.

Usage (CI): check_cla_coauthors.py --pr <number>
  Needs `gh` and GH_TOKEN; repo comes from GITHUB_REPOSITORY or --repo.

Tests: scripts/test_cla_coauthors.py
"""

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import urllib.parse

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent

# git trailer keys are case-insensitive; GitHub emits "Co-authored-by".
_COAUTHOR_RE = re.compile(
    r"^\s*co-authored-by:\s*(?P<name>[^<]*?)\s*<(?P<email>[^>]+)>\s*$",
    re.IGNORECASE | re.MULTILINE,
)

_NOREPLY_RE = re.compile(
    r"^(?:\d+\+)?(?P<login>[^@+]+)@users\.noreply\.github\.com$", re.IGNORECASE
)


def parse_coauthors(message):
    """All Co-authored-by trailers in a commit message as (name, email)."""
    return [
        (m.group("name"), m.group("email").strip())
        for m in _COAUTHOR_RE.finditer(message)
    ]


def noreply_login(email):
    """Login embedded in a GitHub noreply address, or None."""
    m = _NOREPLY_RE.match(email.strip())
    return m.group("login") if m else None


def check_commits(commits, contributors, resolve):
    """Return a list of problem strings; empty means the gate passes.

    commits: [{"sha": ..., "message": ...}]
    contributors: logins from .contributors
    resolve: email -> login-or-None (network lookup; injectable for tests)
    """
    signed = {c.lower() for c in contributors}
    resolved = {}  # email -> login or None, so each email is looked up once
    problems = []
    for commit in commits:
        sha = commit["sha"][:8]
        for name, email in parse_coauthors(commit["message"]):
            if email not in resolved:
                resolved[email] = noreply_login(email) or resolve(email)
            login = resolved[email]
            if login is None:
                problems.append(
                    f"{sha}: cannot attribute Co-authored-by '{name} <{email}>' "
                    "to any GitHub login (not a noreply address, and no commit "
                    "in this repo is authored with it). Ask them to sign per "
                    "CLA.md — the signature PR itself makes their email "
                    "resolvable — or use their @users.noreply.github.com "
                    "address in the trailer."
                )
            elif login.lower() not in signed:
                problems.append(
                    f"{sha}: Co-authored-by '{name} <{email}>' is GitHub user "
                    f"'{login}', who has not signed the CLA (not in "
                    ".contributors). See CLA.md for the one-time signature "
                    "flow."
                )
    return problems


def _gh_api(path):
    out = subprocess.run(
        ["gh", "api", "--paginate", path],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    # --paginate concatenates JSON arrays; parse them all.
    items = []
    dec = json.JSONDecoder()
    i = 0
    while i < len(out):
        chunk, end = dec.raw_decode(out, i)
        items.extend(chunk if isinstance(chunk, list) else [chunk])
        i = end
        while i < len(out) and out[i].isspace():
            i += 1
    return items


def make_repo_resolver(repo):
    """email -> login via the repo's own commit history."""

    def resolve(email):
        q = urllib.parse.quote(email)
        commits = _gh_api(f"repos/{repo}/commits?author={q}&per_page=1")
        for c in commits:
            author = c.get("author") or {}
            if author.get("login"):
                return author["login"]
        return None

    return resolve


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--pr", required=True, type=int, help="pull request number")
    ap.add_argument(
        "--repo",
        default=os.environ.get("GITHUB_REPOSITORY", "xerj-org/xerj"),
        help="owner/name (default: $GITHUB_REPOSITORY or xerj-org/xerj)",
    )
    args = ap.parse_args()

    contributors = json.loads((REPO_ROOT / ".contributors").read_text())
    raw = _gh_api(f"repos/{args.repo}/pulls/{args.pr}/commits?per_page=100")
    commits = [{"sha": c["sha"], "message": c["commit"]["message"]} for c in raw]

    problems = check_commits(commits, contributors, make_repo_resolver(args.repo))
    if problems:
        print(f"CLA co-author check FAILED for PR #{args.pr}:", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1
    trailers = sum(len(parse_coauthors(c["message"])) for c in commits)
    print(
        f"CLA co-author check passed for PR #{args.pr}: "
        f"{len(commits)} commit(s), {trailers} Co-authored-by trailer(s), "
        "all attributable to signed contributors."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
