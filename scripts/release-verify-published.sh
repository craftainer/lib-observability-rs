#!/usr/bin/env bash
# `make verify-published`: build a throwaway binary crate that depends on the
# version `publish-packages` just uploaded to crates.io.
#
# Runs after the GitHub release exists and the upload succeeded, only when a
# *_PUBLISH variable is set (release.yml), with no secrets or OIDC token, on
# the bare runner's own rustup/cargo (the Makefile installs the pinned
# toolchain first). With CARGO_PUBLISH unset (another registry is
# publishing) it does nothing.
#
# Reads: CARGO_PUBLISH, RELEASE_VERSION (the version to look for),
# CRATE / CARGO_PACKAGE (see crate_info.py), CRATES_INDEX_URL (default
# https://index.crates.io), RELEASE_CHECK_ATTEMPTS / RELEASE_CHECK_DELAY
# (retry tuning for index propagation, defaults 10 and 15 seconds, doubling
# to 120). A release that never shows up passes with a notice, not a failure.
set -euo pipefail

cd "$(dirname "$0")/.."
# shellcheck source=scripts/release-lib.sh
source scripts/release-lib.sh

if [[ -z ${CARGO_PUBLISH:-} ]]; then
    echo "CARGO_PUBLISH not set -- nothing to verify."
    exit 0
fi
if [[ -z ${RELEASE_VERSION:-} ]]; then
    echo "RELEASE_VERSION is required to check the published release" >&2
    exit 1
fi

name=$(python3 -I scripts/crate_info.py name)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# One target dir for the probe build.
export CARGO_TARGET_DIR="$tmp/target"

echo "== published $name $RELEASE_VERSION on crates.io"
attempts=${RELEASE_CHECK_ATTEMPTS:-10}
delay=${RELEASE_CHECK_DELAY:-15}
status=1
for ((i = 1; i <= attempts; i++)); do
    # A yanked version doesn't count. 0 = listed, 1 = not (yet), 2 = the index
    # failed or answered garbage: retried like 1, but fatal if it persists.
    status=0
    python3 -I scripts/crate_info.py has-version "$RELEASE_VERSION" --not-yanked || status=$?
    if [[ $status -eq 0 ]]; then
        probe "$tmp/published-probe" "$name" "\"=$RELEASE_VERSION\""
        echo "OK: $name =$RELEASE_VERSION builds from crates.io"
        exit 0
    fi
    if ((i < attempts)); then
        echo "attempt $i/$attempts: $name $RELEASE_VERSION not in the index yet; retrying in ${delay}s"
        sleep "$delay"
        delay=$((delay * 2 > 120 ? 120 : delay * 2))
    fi
done
if [[ $status -ne 1 ]]; then
    echo "could not read the crates.io index for $name after $attempts attempts" >&2
    exit 1
fi
echo "::warning::$name $RELEASE_VERSION was not on crates.io after $attempts attempts; the published-release check was skipped"
echo "NOTICE: $name $RELEASE_VERSION is not on crates.io (index lag, or publishing was skipped); the published release was not verified."
