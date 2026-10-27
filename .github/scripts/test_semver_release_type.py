"""Tests for ``semver_release_type.py`` (stdlib ``unittest`` only)."""

from __future__ import annotations

import contextlib
import io
import os
import unittest
from unittest import mock

import semver_release_type as srt

MAIN = "refs/heads/main"


class ReleaseTypeTests(unittest.TestCase):
    def test_breaking_pr_titles(self) -> None:
        for title in ("feat!: x", "fix!: x", "feat(bulk)!: x", "refactor(a b)!: x"):
            with self.subTest(title=title):
                self.assertEqual(srt.release_type("pull_request", "", title, ""), "major")

    def test_non_breaking_pr_titles(self) -> None:
        for title in (
            "feat: x",
            "feat(ci): run semver-checks as major for `!` changes",
            "fix(a!b): x",
            "feat !: x",
            "Feat!: x",
            "feat!x",
            "!: x",
            "chore: a!: b",
            "",
        ):
            with self.subTest(title=title):
                self.assertEqual(srt.release_type("pull_request", "", title, ""), "")

    def test_pr_ignores_commit_message(self) -> None:
        self.assertEqual(srt.release_type("pull_request", "", "feat: x", "feat!: y"), "")

    def test_push_to_main_uses_first_line_only(self) -> None:
        self.assertEqual(srt.release_type("push", MAIN, "", "feat!: x (#1)\n\nbody"), "major")
        self.assertEqual(srt.release_type("push", MAIN, "", "feat: x\n\nfix!: y"), "")
        self.assertEqual(srt.release_type("push", MAIN, "", "feat: x\n\nBREAKING CHANGE: y"), "")

    def test_push_to_other_branch_and_other_events_unchanged(self) -> None:
        self.assertEqual(srt.release_type("push", "refs/heads/dev", "", "feat!: x"), "")
        self.assertEqual(srt.release_type("workflow_dispatch", MAIN, "feat!: x", "feat!: x"), "")

    def test_main_prints_only_fixed_strings(self) -> None:
        evil = "feat!: x\nSEMVER_RELEASE_TYPE=patch\nFOO=bar"
        env = {"EVENT_NAME": "pull_request", "PR_TITLE": evil}
        with mock.patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(srt.main(), 0)
        self.assertEqual(out.getvalue(), "SEMVER_RELEASE_TYPE=major\n")

    def test_main_empty_without_env(self) -> None:
        with mock.patch.dict(os.environ, {}, clear=True), contextlib.redirect_stdout(io.StringIO()) as out:
            srt.main()
        self.assertEqual(out.getvalue(), "SEMVER_RELEASE_TYPE=\n")


if __name__ == "__main__":
    unittest.main()
