#!/usr/bin/env python3
"""Credential-isolation tests for the release scripts, with a stub `cargo` (and `curl`) on PATH that records its environment per call; run with `python3 -I scripts/test_release_scrub.py`.

Contract: no step that resolves the dependency graph or could run dependency code
(cargo fetch, update, metadata, package) holds a publish credential; only the final
`cargo publish --no-verify` sees CARGO_REGISTRY_TOKEN, and nothing sees GITHUB_TOKEN or
the OIDC request variables.
"""

import json
import os
import pathlib
import re
import shutil
import stat
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Written out here, not imported from the script under test.
CREDENTIAL = re.compile(
    r"^(GITHUB_|ACTIONS_|RUNNER_|GH_|INPUT_|OCI_|CARGO_REGISTRY_TOKEN$|CARGO_REGISTRIES_.*_TOKEN$|NODE_AUTH_TOKEN$|NPM_TOKEN$)"
)

CI_CREDENTIALS = {
    "GITHUB_TOKEN": "gh-token",
    "ACTIONS_RUNTIME_TOKEN": "runtime",
    "ACTIONS_ID_TOKEN_REQUEST_URL": "https://oidc.invalid/token?x=1",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "oidc-request",
    "GH_TOKEN": "gh",
    "NPM_TOKEN": "npm",
    "NODE_AUTH_TOKEN": "node",
    "OCI_REGISTRY_PASSWORD": "oci",
    "CARGO_REGISTRIES_PRIVATE_TOKEN": "private",
}

# Stub cargo: records the subcommand and every credential-looking variable (name and
# value), appended to a log shared with the stub curl, so call order is visible.
FAKE_CARGO = r"""#!/usr/bin/env python3
import json, os, pathlib, re, sys
credential = re.compile(%(pattern)r)
args = sys.argv[1:]
with open(os.environ["FAKE_LOG"], "a") as log:
    log.write(json.dumps({
        "tool": "cargo", "sub": args[0], "args": args,
        "creds": {k: v for k, v in os.environ.items() if credential.match(k)},
        "keeps": {k: os.environ.get(k) for k in ("KEEP_ME", "CARGO_REGISTRIES_PRIVATE_INDEX")},
    }) + "\n")
target = pathlib.Path(os.environ["FAKE_TARGET"])
if args[0] == "metadata":
    print(json.dumps({"target_directory": str(target)}))
elif args[0] == "package":
    (target / "package").mkdir(parents=True, exist_ok=True)
    (target / "package" / "demo-1.2.3.crate").write_bytes(os.environ["FAKE_CRATE"].encode())
"""

# Stub curl: the OIDC exchange (GET the JWT, POST it for a token, DELETE to revoke).
FAKE_CURL = r"""#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
method = args[args.index("-X") + 1] if "-X" in args else "GET"
with open(os.environ["FAKE_LOG"], "a") as log:
    log.write(json.dumps({"tool": "curl", "sub": method}) + "\n")
if method == "GET":
    print(json.dumps({"value": "jwt-from-github"}))
elif method == "POST":
    print(json.dumps({"token": "temporary-crates-token"}))
"""


def install(path: pathlib.Path, text: str) -> None:
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


class Sandbox(unittest.TestCase):
    """A copy of the release scripts in a throwaway repo, with stub cargo and curl first on PATH."""

    def setUp(self) -> None:
        self.work = pathlib.Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(shutil.rmtree, self.work, ignore_errors=True)
        self.repo = self.work / "repo"
        (self.repo / "scripts" / "lib").mkdir(parents=True)
        for name in (
            "release-publish.sh",
            "release-stamped.sh",
            "release-stamp-version.py",
            "crate_info.py",
            "lib/cargo-scrub.sh",
        ):
            shutil.copy(ROOT / "scripts" / name, self.repo / "scripts" / name)
        (self.repo / "Cargo.toml").write_text('[package]\nname = "demo"\nversion = "0.0.0"\n')
        (self.repo / "Cargo.lock").write_text("# lock\n")
        (self.repo / "dist").mkdir()
        self.crate = "verified-crate-bytes"
        (self.repo / "dist" / "demo-1.2.3.crate").write_text(self.crate)
        self.target = self.work / "target"
        bin_dir = self.work / "bin"
        bin_dir.mkdir()
        install(bin_dir / "cargo", FAKE_CARGO % {"pattern": CREDENTIAL.pattern})
        install(bin_dir / "curl", FAKE_CURL)
        self.log = self.work / "calls.log"

    def env(self, **extra: str) -> dict[str, str]:
        base = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(("CARGO", "RELEASE", "RUSTUP")) and not CREDENTIAL.match(k)
        }
        return {
            **base,
            "PATH": f"{self.work / 'bin'}{os.pathsep}{os.environ['PATH']}",
            "FAKE_LOG": str(self.log),
            "FAKE_TARGET": str(self.target),
            "FAKE_CRATE": self.crate,
            "CRATES_INDEX_URL": (self.work / "no-index").as_uri(),
            "KEEP_ME": "yes",
            "CARGO_REGISTRIES_PRIVATE_INDEX": "sparse+https://x/",
            **extra,
        }

    def run_cmd(self, argv: list[str], **extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(  # noqa: S603
            argv,
            capture_output=True,
            text=True,
            env=self.env(**extra),
            cwd=self.repo,
            check=False,
        )

    def calls(self) -> list[dict]:
        if not self.log.exists():
            return []
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def cargo_calls(self) -> list[dict]:
        return [c for c in self.calls() if c["tool"] == "cargo"]


class StampedScrub(Sandbox):
    def stamped(self, *argv: str) -> subprocess.CompletedProcess[str]:
        return self.run_cmd(
            ["bash", "scripts/release-stamped.sh", *argv], RELEASE_VERSION="1.2.3", **CI_CREDENTIALS
        )

    def test_fetch_and_update_never_see_credentials_with_or_without_scrub(self) -> None:
        for argv in (["--scrub", "cargo", "package"], ["cargo", "publish"]):
            with self.subTest(argv=argv):
                self.log.unlink(missing_ok=True)
                result = self.stamped(*argv)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                fetch, update, last = self.cargo_calls()
                self.assertEqual((fetch["sub"], update["sub"]), ("fetch", "update"))
                self.assertEqual(fetch["creds"], {})
                self.assertEqual(update["creds"], {})
                self.assertEqual(fetch["keeps"]["KEEP_ME"], "yes")
                self.assertEqual(fetch["keeps"]["CARGO_REGISTRIES_PRIVATE_INDEX"], "sparse+https://x/")

    def test_scrub_flag_scrubs_the_command_and_its_absence_leaves_it_alone(self) -> None:
        self.assertEqual(self.stamped("--scrub", "cargo", "package").returncode, 0)
        self.assertEqual(self.cargo_calls()[-1]["creds"], {})
        self.log.unlink()
        self.assertEqual(self.stamped("cargo", "package").returncode, 0)
        self.assertEqual(self.cargo_calls()[-1]["creds"], CI_CREDENTIALS)


class ScrubLibrary(Sandbox):
    def scrub(self, *argv: str) -> dict[str, str]:
        result = self.run_cmd(["bash", "scripts/lib/cargo-scrub.sh", "env", *argv], **CI_CREDENTIALS)
        self.assertEqual(result.returncode, 0, result.stderr)
        return dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)

    def test_run_directly_it_drops_every_credential_and_keeps_the_rest(self) -> None:
        env = self.scrub()
        self.assertEqual({k for k in env if CREDENTIAL.match(k)}, set())
        self.assertEqual(env["KEEP_ME"], "yes")
        self.assertEqual(env["CARGO_REGISTRIES_PRIVATE_INDEX"], "sparse+https://x/")

    def test_keep_spares_exactly_the_named_variable(self) -> None:
        result = subprocess.run(  # noqa: S603
            [
                "bash",
                "-c",
                'source scripts/lib/cargo-scrub.sh; scrubbed --keep CARGO_REGISTRY_TOKEN env',
            ],
            capture_output=True,
            text=True,
            env=self.env(CARGO_REGISTRY_TOKEN="tok", **CI_CREDENTIALS),
            cwd=self.repo,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        names = {name for name in (line.split("=", 1)[0] for line in result.stdout.splitlines()) if CREDENTIAL.match(name)}
        self.assertEqual(names, {"CARGO_REGISTRY_TOKEN"})
        self.assertIn("CARGO_REGISTRY_TOKEN=tok", result.stdout.splitlines())

    def test_sourcing_runs_nothing(self) -> None:
        result = subprocess.run(  # noqa: S603
            ["bash", "-c", "source scripts/lib/cargo-scrub.sh; type -t scrubbed"],
            capture_output=True,
            text=True,
            env=self.env(),
            cwd=self.repo,
            check=False,
        )
        self.assertEqual(result.stdout.strip(), "function")


class PublishCredentialOrdering(Sandbox):
    def publish(self, **extra: str) -> subprocess.CompletedProcess[str]:
        return self.run_cmd(
            ["bash", "scripts/release-stamped.sh", "scripts/release-publish.sh"],
            RELEASE_VERSION="1.2.3",
            **{**CI_CREDENTIALS, **extra},
        )

    def assert_only_publish_holds_the_token(self, token: str) -> None:
        calls = self.cargo_calls()
        subs = [c["sub"] for c in calls]
        self.assertEqual(subs[:2], ["fetch", "update"], subs)
        self.assertIn("package", subs)
        self.assertIn("metadata", subs)
        self.assertEqual(subs[-1], "publish", subs)
        self.assertEqual(subs.count("publish"), 1)
        for call in calls[:-1]:
            self.assertEqual(call["creds"], {}, call["sub"])
        publish = calls[-1]
        self.assertEqual(publish["creds"], {"CARGO_REGISTRY_TOKEN": token})
        self.assertIn("--no-verify", publish["args"])
        self.assertEqual(publish["keeps"]["KEEP_ME"], "yes")

    def test_bootstrap_token_reaches_only_cargo_publish(self) -> None:
        result = self.publish(CARGO_REGISTRY_TOKEN="bootstrap-token")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_only_publish_holds_the_token("bootstrap-token")
        self.assertIn("identical to dist/demo-1.2.3.crate", result.stdout)

    def test_oidc_exchange_happens_after_every_credential_free_step(self) -> None:
        result = self.publish()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_only_publish_holds_the_token("temporary-crates-token")
        order = [f"{c['tool']}:{c['sub']}" for c in self.calls()]
        first_curl = order.index("curl:GET")
        for step in ("cargo:fetch", "cargo:update", "cargo:metadata", "cargo:package"):
            self.assertLess(order.index(step), first_curl, order)
        self.assertEqual(order[first_curl:], ["curl:GET", "curl:POST", "cargo:publish", "curl:DELETE"])

    def test_a_differing_repackage_still_never_hands_out_credentials_early(self) -> None:
        result = self.publish(CARGO_REGISTRY_TOKEN="bootstrap-token", FAKE_CRATE="something else")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("re-packaged crate differs", result.stdout)
        self.assert_only_publish_holds_the_token("bootstrap-token")

    def test_strict_mismatch_fails_before_any_credential_is_used(self) -> None:
        result = self.publish(FAKE_CRATE="something else", RELEASE_REPACKAGE_STRICT="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("publish", [c["sub"] for c in self.cargo_calls()])
        self.assertEqual([c for c in self.calls() if c["tool"] == "curl"], [])

    def test_dry_run_publishes_nothing_and_calls_nothing_with_credentials(self) -> None:
        result = self.publish(RELEASE_DRY_RUN="1", CARGO_REGISTRY_TOKEN="bootstrap-token")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("publish", [c["sub"] for c in self.cargo_calls()])
        for call in self.cargo_calls():
            self.assertEqual(call["creds"], {}, call["sub"])


if __name__ == "__main__":
    unittest.main()
