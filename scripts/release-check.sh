#!/usr/bin/env bash
# `make release-check`: smoke-test the packaged .crate by building a throwaway
# binary crate that depends on it. It gates the GitHub release and
# publishing, so it checks only the local crate; the check of the published
# release is scripts/release-verify-published.sh (`make verify-published`),
# which runs after `publish-packages`.
#
# Reads: CRATE / CARGO_PACKAGE (see crate_info.py).
set -euo pipefail

cd "$(dirname "$0")/.."
# shellcheck source=scripts/release-lib.sh
source scripts/release-lib.sh

name=$(python3 -I scripts/crate_info.py name)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# One target dir for every probe build, so the dependency graph compiles once.
export CARGO_TARGET_DIR="$tmp/target"

shopt -s nullglob
crates=(dist/*.crate)
if [[ ${#crates[@]} -ne 1 ]]; then
    echo "expected exactly one .crate in dist/, found ${#crates[@]}" >&2
    exit 1
fi
crate=$(realpath "${crates[0]}")

echo "== local crate: $(basename "$crate")"
mkdir -p "$tmp/unpacked"
tar -xzf "$crate" -C "$tmp/unpacked" --strip-components=1
# The packaged sources must build and pass their doctests on their own: a
# file missing from the package (a doctest include, a README) shows up here.
cargo test --quiet --locked --doc --manifest-path "$tmp/unpacked/Cargo.toml"
probe "$tmp/local-probe" "$name" "{ path = \"$tmp/unpacked\" }"
echo "OK: $name builds as a path dependency and its doctests pass"
