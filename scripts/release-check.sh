#!/usr/bin/env bash
# `make release-check`: smoke-test the packaged .crate by building a throwaway
# binary crate that depends on it, and the published release when publishing
# is on.
#
# Reads: CARGO_PUBLISH (non-empty: also check crates.io), RELEASE_VERSION
# (the version to look for there), CRATE / CARGO_PACKAGE (see crate_info.py),
# CRATES_INDEX_URL (default https://index.crates.io), RELEASE_CHECK_ATTEMPTS /
# RELEASE_CHECK_DELAY (retry tuning for index propagation, defaults 10 and 15 seconds, doubling up
# to 120). A release that never shows up passes with a notice, not a failure.
set -euo pipefail

cd "$(dirname "$0")/.."

name=$(python3 -I scripts/crate_info.py name)
# The lib target name: hyphens become underscores unless [lib] name says otherwise.
module=${name//-/_}
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

probe() { # <dir> <dependency spec>: a binary crate that links the dependency
    mkdir -p "$1/src"
    cat >"$1/Cargo.toml" <<TOML
[package]
name = "release-probe"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
$name = $2
TOML
    printf '// Throwaway: proves the dependency resolves and links.\nuse %s as _;\n\nfn main() {}\n' \
        "$module" >"$1/src/main.rs"
    cargo build --quiet --manifest-path "$1/Cargo.toml"
}

echo "== local crate: $(basename "$crate")"
mkdir -p "$tmp/unpacked"
tar -xzf "$crate" -C "$tmp/unpacked" --strip-components=1
# The packaged sources must build and pass their doctests on their own: a
# file missing from the package (a doctest include, a README) shows up here.
cargo test --quiet --locked --doc --manifest-path "$tmp/unpacked/Cargo.toml"
probe "$tmp/local-probe" "{ path = \"$tmp/unpacked\" }"
echo "OK: $name builds as a path dependency and its doctests pass"

if [[ -z ${CARGO_PUBLISH:-} ]]; then
    echo "CARGO_PUBLISH not set -- skipping the published-release check."
    exit 0
fi
if [[ -z ${RELEASE_VERSION:-} ]]; then
    echo "RELEASE_VERSION is required to check the published release" >&2
    exit 1
fi

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
        probe "$tmp/published-probe" "\"=$RELEASE_VERSION\""
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
echo "NOTICE: $name $RELEASE_VERSION is not on crates.io (index lag, or publishing was skipped); only the local crate was verified."
