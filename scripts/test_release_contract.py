#!/usr/bin/env python3
"""Tests for the Makefile's publish/publish-packages split; run with `python3 -I scripts/test_release_contract.py`."""

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


if __name__ == "__main__":
    unittest.main()
