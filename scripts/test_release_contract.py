#!/usr/bin/env python3
"""Tests for the Makefile's publish/publish-packages/verify-published split; run with `python3 -I scripts/test_release_contract.py`."""

import os
import pathlib
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def make(*args: str, **env: str) -> subprocess.CompletedProcess[str]:
    full_env = {k: v for k, v in os.environ.items() if k != "CARGO_PUBLISH"}
    full_env.update(env)
    return subprocess.run(  # noqa: S603
        ["make", "-C", str(ROOT), *args],  # noqa: S607
        capture_output=True,
        text=True,
        env=full_env,
        check=False,
    )


class ReleaseContract(unittest.TestCase):
    def test_publish_is_image_only_even_with_cargo_publish_set(self) -> None:
        for env in ({}, {"CARGO_PUBLISH": "1"}):
            with self.subTest(env=env):
                result = make("publish", **env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("no image to publish", result.stdout)
                self.assertNotIn("release-publish", result.stdout)

    def test_publish_packages_unset_warns_and_skips(self) -> None:
        result = make("publish-packages")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("::warning::CARGO_PUBLISH is not set", result.stdout)

    def test_publish_packages_set_runs_the_publish_script_stamped(self) -> None:
        result = make("-n", "publish-packages", CARGO_PUBLISH="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("rustup toolchain install", result.stdout)
        self.assertIn("scripts/release-stamped.sh scripts/release-publish.sh", result.stdout)

    def test_publish_packages_is_phony(self) -> None:
        result = make("-p", "-n", "publish-packages")
        self.assertRegex(result.stdout, r"(?m)^\.PHONY:.*\bpublish-packages\b")

    def test_verify_published_unset_has_nothing_to_verify(self) -> None:
        result = make("verify-published")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("nothing to verify", result.stdout)

    def test_verify_published_set_runs_the_verify_script(self) -> None:
        result = make("-n", "verify-published", CARGO_PUBLISH="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("scripts/release-verify-published.sh", result.stdout)
        self.assertIn("rustup toolchain install", result.stdout)

    def test_verify_published_is_phony(self) -> None:
        result = make("-p", "-n", "verify-published")
        self.assertRegex(result.stdout, r"(?m)^\.PHONY:.*\bverify-published\b")

    def test_release_check_gates_the_release_so_it_skips_the_registry(self) -> None:
        script = (ROOT / "scripts" / "release-check.sh").read_text()
        self.assertNotIn("CARGO_PUBLISH", script)
        self.assertNotIn("has-version", script)


if __name__ == "__main__":
    unittest.main()
