#!/usr/bin/env python3
"""Regression tests for the Co-authored-by CLA check (issue #269).

Before this check existed, cla-bot matched only commit *authors* against
.contributors, so a maintainer re-land of a fork PR — commits authored
xerj-org, contributor credited via a Co-authored-by trailer — went green
without the contributor ever signing (live instance: PR #248 re-landing
buger's #166). These tests pin the trailer-aware behaviour.

Run: python3 scripts/test_cla_coauthors.py
"""

import pathlib
import re
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from check_cla_coauthors import check_commits, noreply_login, parse_coauthors

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent

# The shape of PR #248: maintainer-authored re-land, real attribution
# carried only by the trailer.
RELAND_COMMITS = [
    {
        "sha": "c337eb8f",
        "message": "perf(autoindex): reuse the phase-A PDF extraction\n\n"
        "Body text.\n\nCo-authored-by: Leonid Bugaev <leonsbox@gmail.com>",
    },
    {
        "sha": "81c64883",
        "message": "docs(changelog): record the change\n\n"
        "Co-authored-by: Leonid Bugaev <leonsbox@gmail.com>",
    },
]


def no_network(email):
    raise AssertionError(f"unexpected network resolution for {email!r}")


class ParseTests(unittest.TestCase):
    def test_extracts_name_and_email(self):
        got = parse_coauthors(RELAND_COMMITS[0]["message"])
        self.assertEqual(got, [("Leonid Bugaev", "leonsbox@gmail.com")])

    def test_trailer_key_is_case_insensitive(self):
        # git trailers are case-insensitive; GitHub emits "Co-authored-by".
        msg = "subject\n\nCO-AUTHORED-BY: A B <a@b.c>\nco-authored-by: C D <c@d.e>"
        self.assertEqual(
            parse_coauthors(msg), [("A B", "a@b.c"), ("C D", "c@d.e")]
        )

    def test_no_trailers(self):
        self.assertEqual(parse_coauthors("fix: something\n\nplain body"), [])


class NoreplyTests(unittest.TestCase):
    def test_modern_noreply(self):
        self.assertEqual(
            noreply_login("1234567+buger@users.noreply.github.com"), "buger"
        )

    def test_legacy_noreply(self):
        self.assertEqual(noreply_login("buger@users.noreply.github.com"), "buger")

    def test_bot_noreply(self):
        self.assertEqual(
            noreply_login("49699333+dependabot[bot]@users.noreply.github.com"),
            "dependabot[bot]",
        )

    def test_ordinary_email_is_not_a_login(self):
        self.assertIsNone(noreply_login("leonsbox@gmail.com"))


class GateTests(unittest.TestCase):
    def test_unsigned_coauthor_fails_the_gate(self):
        # THE regression for issue #269: every commit author is signed, the
        # co-author is not — the old gate said green here.
        problems = check_commits(
            RELAND_COMMITS,
            contributors=["xerj-org", "dependabot[bot]"],
            resolve=lambda email: "buger",
        )
        self.assertTrue(problems, "gate passed a re-land with an unsigned co-author")
        self.assertTrue(any("buger" in p for p in problems), problems)

    def test_unresolvable_coauthor_fails_the_gate(self):
        # An identity the gate cannot attribute is exactly what it exists to
        # flag — unresolvable must fail, not silently pass.
        problems = check_commits(
            RELAND_COMMITS,
            contributors=["xerj-org", "dependabot[bot]"],
            resolve=lambda email: None,
        )
        self.assertTrue(problems, "gate passed an unresolvable co-author")

    def test_signed_coauthor_passes(self):
        problems = check_commits(
            RELAND_COMMITS,
            contributors=["xerj-org", "dependabot[bot]", "buger"],
            resolve=lambda email: "buger",
        )
        self.assertEqual(problems, [])

    def test_login_match_is_case_insensitive(self):
        # GitHub logins are case-insensitive; a case mismatch between the
        # resolved login and the .contributors entry must not flag a signer.
        problems = check_commits(
            RELAND_COMMITS,
            contributors=["xerj-org", "Buger"],
            resolve=lambda email: "buger",
        )
        self.assertEqual(problems, [])

    def test_noreply_coauthor_needs_no_network(self):
        commits = [
            {
                "sha": "abc12345",
                "message": "subject\n\n"
                "Co-authored-by: B <99+buger@users.noreply.github.com>",
            }
        ]
        problems = check_commits(
            commits, contributors=["buger"], resolve=no_network
        )
        self.assertEqual(problems, [])

    def test_no_trailers_needs_no_network(self):
        commits = [{"sha": "abc12345", "message": "fix: plain commit"}]
        self.assertEqual(
            check_commits(commits, contributors=["xerj-org"], resolve=no_network), []
        )

    def test_duplicate_emails_resolved_once(self):
        calls = []

        def counting(email):
            calls.append(email)
            return "buger"

        check_commits(RELAND_COMMITS, contributors=["buger"], resolve=counting)
        self.assertEqual(calls, ["leonsbox@gmail.com"])


class WiringTests(unittest.TestCase):
    """A checker CI never runs is not a gate (cf. the #207 lesson)."""

    def _cla_job(self):
        ci = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text()
        m = re.search(r"\n  cla-config:\n(.*?)(?=\n  \S)", ci, re.S)
        self.assertIsNotNone(m, "cla-config job missing from ci.yml")
        return m.group(1)

    def test_ci_runs_the_coauthor_check(self):
        self.assertIn("check_cla_coauthors.py", self._cla_job())

    def test_ci_runs_these_tests(self):
        self.assertIn("test_cla_coauthors.py", self._cla_job())


if __name__ == "__main__":
    unittest.main()
