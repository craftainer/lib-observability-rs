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
#
# The probe is built from the temp dir, outside the repo, so what a consumer
# of the published crate would get is what is checked: cargo finds
# `.cargo/config.toml` by walking up from its working directory, so the repo's
# [patch] sections, source replacement and rustflags cannot apply. (The
# runner's own $CARGO_HOME/config.toml still applies; it belongs to the
# runner, not the repo.) Outside the repo rustup no longer sees
# rust-toolchain.toml either, so the channel is read from it and handed to
# rustup explicitly via RUSTUP_TOOLCHAIN (same effect as `cargo +<channel>`,
# and a no-op for a cargo that is not a rustup proxy).
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
toolchain=$(python3 -I -c 'import sys, tomllib
with open(sys.argv[1], "rb") as f:
    print(tomllib.load(f)["toolchain"]["channel"])' rust-toolchain.toml) || {
    echo "could not read [toolchain] channel from rust-toolchain.toml" >&2
    exit 1
}

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
        # Subshell: leaves the repo (config discovery) and pins the toolchain
        # for this build only.
        (
            cd "$tmp"
            export RUSTUP_TOOLCHAIN=$toolchain
            probe "$tmp/published-probe" "$name" "\"=$RELEASE_VERSION\""
        )
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
