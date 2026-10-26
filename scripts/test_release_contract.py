#!/usr/bin/env python3
"""Tests for the Makefile's publish/publish-packages/verify-published split and the verify-published probe's isolation; run with `python3 -I scripts/test_release_contract.py`."""

import json
import os
import pathlib
import shutil
import stat
import subprocess
import tempfile
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


# Stands in for cargo: records where and how it was called, and which
# `.cargo/config.toml` files its working directory would discover (the walk
# cargo does: the cwd and every parent).
FAKE_CARGO = """#!/usr/bin/env python3
import json, os, pathlib, sys
cwd = pathlib.Path.cwd().resolve()
found = [str(d / ".cargo" / "config.toml") for d in [cwd, *cwd.parents] if (d / ".cargo" / "config.toml").is_file()]
args = sys.argv[1:]
manifest = pathlib.Path(args[args.index("--manifest-path") + 1]).read_text() if "--manifest-path" in args else ""
record = {"cwd": str(cwd), "args": args, "manifest": manifest, "toolchain": os.environ.get("RUSTUP_TOOLCHAIN"),
          "target_dir": os.environ.get("CARGO_TARGET_DIR"), "configs": found}
with open(os.environ["FAKE_CARGO_LOG"], "a") as log:
    log.write(json.dumps(record) + "\\n")
"""


class VerifyPublishedProbe(unittest.TestCase):
    """The probe must build outside the repo, with the repo's pinned toolchain."""

    def setUp(self) -> None:
        self.work = pathlib.Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.repo = self.work / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        for name in ("release-verify-published.sh", "release-lib.sh", "crate_info.py"):
            shutil.copy(ROOT / "scripts" / name, self.repo / "scripts" / name)
        (self.repo / "Cargo.toml").write_text('[package]\nname = "demo-crate"\nversion = "0.0.0"\n')
        (self.repo / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.2.3"\n')
        # A repo config that would redirect the probe's dependency, if it applied.
        (self.repo / ".cargo").mkdir()
        (self.repo / ".cargo" / "config.toml").write_text(
            '[patch.crates-io]\ndemo-crate = { path = "/nonexistent/patched" }\n'
            '[build]\nrustflags = ["--cfg", "from_repo_config"]\n'
        )
        index = self.work / "index" / "de" / "mo"
        index.mkdir(parents=True)
        (index / "demo-crate").write_text(json.dumps({"vers": "9.9.9", "yanked": False, "cksum": "x"}) + "\n")
        bin_dir = self.work / "bin"
        bin_dir.mkdir()
        fake = bin_dir / "cargo"
        fake.write_text(FAKE_CARGO)
        fake.chmod(fake.stat().st_mode | stat.S_IEXEC)
        self.log = self.work / "cargo.log"

    def run_script(self) -> subprocess.CompletedProcess[str]:
        (self.work / "tmp").mkdir(exist_ok=True)
        env = {
            **{k: v for k, v in os.environ.items() if not k.startswith(("CARGO", "RUSTUP", "RELEASE"))},
            "PATH": f"{self.work / 'bin'}{os.pathsep}{os.environ['PATH']}",
            "TMPDIR": str(self.work / "tmp"),
            "CARGO_PUBLISH": "1",
            "RELEASE_VERSION": "9.9.9",
            "CRATES_INDEX_URL": (self.work / "index").as_uri(),
            "RELEASE_CHECK_ATTEMPTS": "1",
            "FAKE_CARGO_LOG": str(self.log),
        }
        return subprocess.run(  # noqa: S603
            ["bash", str(self.repo / "scripts" / "release-verify-published.sh")],  # noqa: S607
            capture_output=True,
            text=True,
            env=env,
            cwd=self.repo,
            check=False,
        )

    def test_probe_builds_outside_the_repo_so_its_cargo_config_cannot_apply(self) -> None:
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        records = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual(len(records), 1, records)
        record = records[0]
        self.assertNotIn(self.repo, [pathlib.Path(record["cwd"]), *pathlib.Path(record["cwd"]).parents])
        # Only configs inside this test's own tree count; the ambient /tmp is not ours.
        ours = [c for c in record["configs"] if c.startswith(str(self.work))]
        self.assertEqual(ours, [], "cargo would discover a config file from the probe's cwd")
        self.assertEqual(record["args"][0], "build")
        self.assertNotIn(str(self.repo), record["target_dir"])

    def test_probe_takes_the_toolchain_from_rust_toolchain_toml(self) -> None:
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        record = json.loads(self.log.read_text().splitlines()[0])
        self.assertEqual(record["toolchain"], "1.2.3")

    def test_probe_depends_on_the_exact_published_version(self) -> None:
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        record = json.loads(self.log.read_text().splitlines()[0])
        self.assertIn('demo-crate = "=9.9.9"', record["manifest"])

    def test_missing_toolchain_file_fails_loudly(self) -> None:
        (self.repo / "rust-toolchain.toml").unlink()
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rust-toolchain.toml", result.stderr)
        self.assertFalse(self.log.exists())


# Records each call's argv (one JSON list per line) and the version Cargo.toml
# carried at that moment.
ORDER_CARGO = """#!/usr/bin/env python3
import json, os, pathlib, sys
with open(os.environ["FAKE_CARGO_LOG"], "a") as log:
    log.write(json.dumps({"args": sys.argv[1:], "manifest": pathlib.Path("Cargo.toml").read_text()}) + "\\n")
"""


class StampedRelease(unittest.TestCase):
    """A crate with dependencies must have them fetched before the offline stamp."""

    def setUp(self) -> None:
        self.work = pathlib.Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.repo = self.work / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        for name in ("release-stamped.sh", "release-stamp-version.py"):
            shutil.copy(ROOT / "scripts" / name, self.repo / "scripts" / name)
        (self.repo / "Cargo.toml").write_text('[package]\nname = "demo"\nversion = "0.0.0"\n')
        (self.repo / "Cargo.lock").write_text("# lock\n")
        bin_dir = self.work / "bin"
        bin_dir.mkdir()
        fake = bin_dir / "cargo"
        fake.write_text(ORDER_CARGO)
        fake.chmod(fake.stat().st_mode | stat.S_IEXEC)
        self.log = self.work / "cargo.log"

    def run_script(self, version: str = "1.2.3") -> subprocess.CompletedProcess[str]:
        env = {
            **{k: v for k, v in os.environ.items() if not k.startswith(("CARGO", "RELEASE"))},
            "PATH": f"{self.work / 'bin'}{os.pathsep}{os.environ['PATH']}",
            "FAKE_CARGO_LOG": str(self.log),
        }
        if version:
            env["RELEASE_VERSION"] = version
        return subprocess.run(  # noqa: S603
            ["bash", str(self.repo / "scripts" / "release-stamped.sh"), "cargo", "package"],  # noqa: S607
            capture_output=True,
            text=True,
            env=env,
            cwd=self.repo,
            check=False,
        )

    def calls(self) -> list[dict[str, object]]:
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_fetches_locked_deps_before_the_offline_update(self) -> None:
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = self.calls()
        self.assertEqual(
            [c["args"] for c in calls],
            [["fetch", "--locked"], ["update", "--workspace", "--offline", "--quiet"], ["package"]],
        )

    def test_fetch_runs_before_the_stamp_so_locked_still_matches(self) -> None:
        self.assertEqual(self.run_script().returncode, 0)
        fetch, update, package = self.calls()
        self.assertIn('version = "0.0.0"', fetch["manifest"])
        self.assertIn('version = "1.2.3"', update["manifest"])
        self.assertIn('version = "1.2.3"', package["manifest"])

    def test_checkout_is_restored_afterwards(self) -> None:
        self.assertEqual(self.run_script().returncode, 0)
        self.assertIn('version = "0.0.0"', (self.repo / "Cargo.toml").read_text())

    def test_unstamped_run_leaves_the_command_alone(self) -> None:
        self.assertEqual(self.run_script(version="").returncode, 0)
        self.assertEqual([c["args"] for c in self.calls()], [["package"]])


if __name__ == "__main__":
    unittest.main()
