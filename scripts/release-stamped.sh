#!/usr/bin/env bash
# Runs a command with RELEASE_VERSION stamped into Cargo.toml (and Cargo.lock
# refreshed to match), then restores both, so the checkout stays clean: the
# version lives only in the git tag.
#
# Usage: release-stamped.sh [--scrub] <command> [args...]
# With RELEASE_VERSION unset the command runs unchanged (a local build).
# The preparation (cargo fetch, cargo update) always runs without CI
# credentials in its environment (see `scrubbed` in lib/cargo-scrub.sh): it
# resolves the dependency graph. --scrub does the same for the command itself:
# for one that compiles dependency code in a job that holds credentials it does
# not need. Without it the command keeps the environment (release-publish.sh,
# which scrubs every cargo call except the final publish itself).
set -euo pipefail

cd "$(dirname "$0")/.."

# shellcheck source=scripts/lib/cargo-scrub.sh
source scripts/lib/cargo-scrub.sh

scrub=
if [[ ${1:-} == --scrub ]]; then
    scrub=1
    shift
fi

if [[ -n ${RELEASE_VERSION:-} ]]; then
    backup=$(mktemp -d)
    cp Cargo.toml Cargo.lock "$backup/"
    trap 'cp "$backup/Cargo.toml" "$backup/Cargo.lock" . && rm -rf "$backup"' EXIT
    # The --offline update below resolves from the local registry cache only,
    # which is empty on a fresh runner: a crate with any dependency would fail
    # with "no matching package named ..." Fetch the locked set first,
    # before the stamp, while Cargo.lock still matches Cargo.toml (--locked).
    scrubbed cargo fetch --locked
    python3 -I scripts/release-stamp-version.py Cargo.toml "$RELEASE_VERSION"
    # --workspace rewrites only the workspace members' own versions in
    # Cargo.lock; dependency versions stay exactly as locked.
    scrubbed cargo update --workspace --offline --quiet
fi
if [[ -n $scrub ]]; then
    scrubbed "$@"
else
    "$@"
fi
