#!/usr/bin/env bash
# Runs a command with RELEASE_VERSION stamped into Cargo.toml (and Cargo.lock
# refreshed to match), then restores both, so the checkout stays clean: the
# version lives only in the git tag.
#
# Usage: release-stamped.sh <command> [args...]
# With RELEASE_VERSION unset the command runs unchanged (a local build).
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -n ${RELEASE_VERSION:-} ]]; then
    backup=$(mktemp -d)
    cp Cargo.toml Cargo.lock "$backup/"
    trap 'cp "$backup/Cargo.toml" "$backup/Cargo.lock" . && rm -rf "$backup"' EXIT
    python3 -I scripts/release-stamp-version.py Cargo.toml "$RELEASE_VERSION"
    # --workspace rewrites only the workspace members' own versions in
    # Cargo.lock; dependency versions stay exactly as locked.
    cargo update --workspace --offline --quiet
fi
"$@"
