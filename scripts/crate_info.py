#!/usr/bin/env python3
"""Answers "which crate, which version, where on the sparse index?" for the release tooling.

One implementation for the Makefile, release-check.sh, release-publish.sh and
the semver-checks hook, so the name lookup and the index layout can't drift.

Usage (run with `python3 -I`):
  crate_info.py name                 crate name
  crate_info.py version              RELEASE_VERSION, else the version in Cargo.toml
  crate_info.py index-url [name]     sparse-index URL of a crate (default: this one)
  crate_info.py index-info           "<name> <index-url>" on one line
  crate_info.py has-version VERSION [--not-yanked]
                                     exit 0 if the index lists VERSION, 1 if not
                                     (crate unknown included), 2 if the index failed
                                     or answered garbage
  crate_info.py checksum VERSION     the index's sha256 (`cksum`) of VERSION's .crate;
                                     exit codes as has-version
  crate_info.py target-dir           cargo's target directory (needs cargo)

Usage errors exit 64.

Env: CRATE or CARGO_PACKAGE (override the name; a virtual workspace needs one),
RELEASE_VERSION, CRATES_INDEX_URL (default https://index.crates.io; a file://
URL works for tests).
"""

import json
import os
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request

DEFAULT_INDEX = "https://index.crates.io"


def load_manifest(path: str = "Cargo.toml") -> dict:
    with open(path, "rb") as handle:
        return tomllib.load(handle)


def crate_name(manifest: dict, env: "os._Environ[str] | dict[str, str]") -> str:
    for key in ("CRATE", "CARGO_PACKAGE"):
        if env.get(key):
            return env[key]
    try:
        return manifest["package"]["name"]
    except KeyError:
        sys.exit("Cargo.toml has no [package] name (virtual workspace?): set CRATE or CARGO_PACKAGE")


def crate_version(manifest: dict, env: "os._Environ[str] | dict[str, str]") -> str:
    if env.get("RELEASE_VERSION"):
        return env["RELEASE_VERSION"]
    version = manifest.get("package", {}).get("version")
    if version is None or isinstance(version, dict):
        version = manifest.get("workspace", {}).get("package", {}).get("version")
    if not isinstance(version, str):
        sys.exit("no literal version in Cargo.toml's [package] or [workspace.package]")
    return version


def index_path(name: str) -> str:
    """Sparse-index path of a crate: the index lowercases the whole name."""
    name = name.lower()
    if len(name) <= 2:
        return f"{len(name)}/{name}"
    if len(name) == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[:2]}/{name[2:4]}/{name}"


def index_url(name: str, env: "os._Environ[str] | dict[str, str]") -> str:
    base = (env.get("CRATES_INDEX_URL") or DEFAULT_INDEX).rstrip("/")
    return f"{base}/{index_path(name)}"


class IndexError_(Exception):
    """The index couldn't be read or didn't hold what a sparse index holds."""


def fetch_entries(url: str) -> list[dict]:
    """The version entries of a crate's index file; [] when the crate is unknown."""
    request = urllib.request.Request(url, headers={"User-Agent": "craftainer-release-tooling"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310
            body = response.read().decode()
    except urllib.error.HTTPError as error:
        if error.code in (404, 410):
            return []
        raise IndexError_(f"HTTP {error.code} for {url}") from error
    except (urllib.error.URLError, OSError, UnicodeDecodeError) as error:
        if isinstance(getattr(error, "reason", None), FileNotFoundError) or isinstance(error, FileNotFoundError):
            return []
        raise IndexError_(f"{error} ({url})") from error
    entries = []
    for number, line in enumerate(body.splitlines(), 1):
        if not line.strip():
            continue
        try:
            entry = json.loads(line)
        except ValueError as error:
            raise IndexError_(f"line {number} of {url} is not JSON") from error
        if not isinstance(entry, dict) or not isinstance(entry.get("vers"), str) or not isinstance(entry.get("yanked"), bool):
            raise IndexError_(f"line {number} of {url} lacks `vers`/`yanked`")
        entries.append(entry)
    return entries


def has_version(url: str, version: str, not_yanked: bool) -> int:
    try:
        entries = fetch_entries(url)
    except IndexError_ as error:
        print(f"index lookup failed: {error}", file=sys.stderr)
        return 2
    found = any(e["vers"] == version and not (not_yanked and e["yanked"]) for e in entries)
    return 0 if found else 1


def checksum(url: str, version: str) -> tuple[int, str]:
    """(exit status, cksum): 0 listed, 1 not listed, 2 index failure."""
    try:
        entries = fetch_entries(url)
    except IndexError_ as error:
        print(f"index lookup failed: {error}", file=sys.stderr)
        return 2, ""
    for entry in entries:
        if entry["vers"] == version:
            cksum = entry.get("cksum")
            if not isinstance(cksum, str) or not cksum:
                print(f"index entry for {version} has no `cksum`", file=sys.stderr)
                return 2, ""
            return 0, cksum
    return 1, ""


def target_directory(run=subprocess.run) -> str:  # noqa: ANN001
    result = run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        sys.exit(f"cargo metadata failed: {result.stderr.strip()}")
    return json.loads(result.stdout)["target_directory"]


def main(argv: list[str]) -> int:
    env = os.environ
    command = argv[0] if argv else ""
    if command == "name" and len(argv) == 1:
        print(crate_name(load_manifest(), env))
    elif command == "version" and len(argv) == 1:
        print(crate_version(load_manifest(), env))
    elif command == "index-url" and len(argv) in (1, 2):
        name = argv[1] if len(argv) == 2 else crate_name(load_manifest(), env)
        print(index_url(name, env))
    elif command == "index-info" and len(argv) == 1:
        name = crate_name(load_manifest(), env)
        print(name, index_url(name, env))
    elif command == "has-version" and len(argv) >= 2 and set(argv[2:]) <= {"--not-yanked"}:
        name = crate_name(load_manifest(), env)
        return has_version(index_url(name, env), argv[1], "--not-yanked" in argv[2:])
    elif command == "checksum" and len(argv) == 2:
        name = crate_name(load_manifest(), env)
        status, cksum = checksum(index_url(name, env), argv[1])
        if status == 0:
            print(cksum)
        return status
    elif command == "target-dir" and len(argv) == 1:
        print(target_directory())
    else:
        print(__doc__, file=sys.stderr)
        return 64
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
