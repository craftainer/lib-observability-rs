#!/usr/bin/env bash
# `make publish-packages` (when CARGO_PUBLISH is set): cargo publish to crates.io,
# authenticated either by a bootstrap token or by crates.io trusted
# publishing. Runs under release-stamped.sh, so Cargo.toml carries the
# release version.
#
# Reads: CARGO_REGISTRY_TOKEN (the first-release bootstrap token; empty counts
# as absent), ACTIONS_ID_TOKEN_REQUEST_URL / _TOKEN (GitHub's OIDC endpoint,
# present when the job has `id-token: write`), CRATES_IO_URL (default
# https://crates.io), CRATES_INDEX_URL (sparse index, default
# https://index.crates.io), CRATE / CARGO_PACKAGE (see crate_info.py),
# RELEASE_DRY_RUN (non-empty: print the chosen path and every step; the only
# network call is the read-only index lookup, and nothing is published),
# RELEASE_REPACKAGE_STRICT (non-empty: fail, not warn, when the re-packaged
# crate differs from dist/'s; see publish()).
#
# Idempotent: a version already in the sparse index with the same checksum as
# dist/<crate>-<version>.crate is skipped, so re-running a failed release job
# can complete; a listed version with other contents fails.
#
# Credentials exist only where they are used. Everything that resolves the
# dependency graph or could run dependency code (release-stamped.sh's cargo
# fetch and update, cargo metadata, the re-package) runs `scrubbed`, without any
# CI credential, and finishes before the OIDC exchange; the final
# `cargo publish --no-verify` then keeps CARGO_REGISTRY_TOKEN and nothing else
# (no GITHUB_TOKEN, no OIDC request variables).
#
# Trusted publishing is what rust-lang/crates-io-auth-action does, minus the
# action (release.yml is base-owned): ask GitHub for an OIDC token with
# audience crates.io, POST it to $URL/api/v1/trusted_publishing/tokens for a
# token valid 30 minutes, publish with it, then DELETE that same endpoint
# (Bearer token) to revoke it.
set -euo pipefail

url=${CRATES_IO_URL:-https://crates.io}
url=${url%/}
audience=${url#http://}
audience=${audience#https://}
endpoint="$url/api/v1/trusted_publishing/tokens"
dry=${RELEASE_DRY_RUN:-}

cd "$(dirname "$0")/.."
# shellcheck source=scripts/lib/cargo-scrub.sh
source scripts/lib/cargo-scrub.sh
name=$(python3 -I scripts/crate_info.py name)
version=$(python3 -I scripts/crate_info.py version)
package_args=()
if [[ -n ${CARGO_PACKAGE:-} ]]; then
    package_args=(-p "$CARGO_PACKAGE")
fi

# Idempotence: has the index got this exact version? 0 = listed, 1 = not
# listed, 2 = the index failed or answered garbage (fail: publishing blind
# could mask a duplicate).
status=0
cksum=$(python3 -I scripts/crate_info.py checksum "$version") || status=$?
case $status in
    0)
        # Listed (a yanked version counts: crates.io never accepts it again).
        # Only skip when it is *our* crate; the index's cksum is the sha256 of
        # the .crate that was uploaded.
        crate_file="dist/$name-$version.crate"
        if [[ ! -f $crate_file ]]; then
            echo "$name $version is already on crates.io but $crate_file is missing, so its contents can't be compared (run make build)" >&2
            exit 1
        fi
        local_sum=$(sha256sum "$crate_file" | cut -d' ' -f1)
        if [[ $local_sum == "$cksum" ]]; then
            echo "NOTICE: $name $version is already on crates.io with identical contents; nothing to publish."
            exit 0
        fi
        echo "$name $version is already published with different contents (index sha256 $cksum, $crate_file $local_sum)" >&2
        exit 1
        ;;
    1) ;;
    *)
        echo "could not read the crates.io index for $name; not publishing blind" >&2
        exit 1
        ;;
esac

# cargo publish cannot upload a prebuilt .crate (the Cargo Book: it always
# packages the working tree itself), so it re-packages the tree this job
# checked out and stamped, and that is what is uploaded. Two things keep that
# safe:
#  - --no-verify: cargo's verify step builds the unpacked crate, which runs
#    dependency build.rs scripts and proc-macros. The crate was verify-built
#    without credentials in `make build` (release-stamped.sh --scrub), so nothing
#    here compiles dependency code.
#  - the re-package is compared with dist/<crate>-<version>.crate (the file
#    that was verify-built, tested, SBOM'd and attached to the release); see
#    check_repackage, which runs before any credential is used.
# The final cargo call keeps CARGO_REGISTRY_TOKEN and drops the rest.
publish() {
    # CARGO_REGISTRY_TOKEN is read by cargo from the environment, so the
    # token never appears on a command line. --allow-dirty: release-stamped.sh
    # has edited Cargo.toml and Cargo.lock.
    local args=(--locked --allow-dirty --no-verify ${package_args[@]+"${package_args[@]}"})
    if [[ -n $dry ]]; then
        echo "[dry run] would: cargo publish ${args[*]} (only CARGO_REGISTRY_TOKEN kept in its environment)"
    else
        scrubbed --keep CARGO_REGISTRY_TOKEN cargo publish "${args[@]}"
    fi
}

# Packages the tree again (no build, no credentials) and compares
# the result with dist/'s .crate. A difference means the upload would not be
# the artifact that was checked, so it is reported loudly. It is a warning, not
# a failure, until a real release has shown the two are byte-identical in CI
# (cargo's output is meant to be deterministic, but nothing here has exercised
# it yet); set RELEASE_REPACKAGE_STRICT=1 to make it fail.
check_repackage() {
    local built="dist/$name-$version.crate" again sum_built sum_again
    if [[ ! -f $built ]]; then
        echo "publish: $built is missing; cannot compare the re-packaged crate with the verified one" >&2
        return 0
    fi
    scrubbed cargo package --locked --allow-dirty --no-verify \
        ${package_args[@]+"${package_args[@]}"} >/dev/null
    again="$(scrubbed python3 -I scripts/crate_info.py target-dir)/package/$name-$version.crate"
    if [[ ! -f $again ]]; then
        echo "publish: the re-packaged crate is not at $again; cannot compare it with $built" >&2
        [[ -z ${RELEASE_REPACKAGE_STRICT:-} ]] || exit 1
        return 0
    fi
    sum_built=$(sha256sum "$built" | cut -d' ' -f1)
    sum_again=$(sha256sum "$again" | cut -d' ' -f1)
    if [[ $sum_built == "$sum_again" ]]; then
        echo "publish: the re-packaged crate is identical to $built ($sum_built)"
        return 0
    fi
    echo "::warning title=re-packaged crate differs::cargo publish re-packages the tree; its crate ($sum_again) differs from the verified $built ($sum_built)."
    [[ -z ${RELEASE_REPACKAGE_STRICT:-} ]] || exit 1
}

if [[ -n ${CARGO_REGISTRY_TOKEN:-} ]]; then
    echo "publish: using the CARGO_REGISTRY_TOKEN bootstrap token"
    [[ -n $dry ]] || check_repackage
    export CARGO_REGISTRY_TOKEN
    publish
    exit 0
fi
unset CARGO_REGISTRY_TOKEN

if [[ -z ${ACTIONS_ID_TOKEN_REQUEST_URL:-} || -z ${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-} ]]; then
    if [[ -z $dry ]]; then
        cat >&2 <<'MSG'
publish: no credentials. Either set the CARGO_REGISTRY_TOKEN secret (first
release of a crate, before a trusted publisher exists) or run in a job with
`id-token: write` for crates.io trusted publishing.
MSG
        exit 1
    fi
fi

echo "publish: using crates.io trusted publishing (OIDC audience $audience)"
# Before the OIDC exchange: nothing below this line runs until the re-package
# has been checked without credentials.
[[ -n $dry ]] || check_repackage
if [[ -n $dry ]]; then
    echo "[dry run] would: GET \$ACTIONS_ID_TOKEN_REQUEST_URL&audience=$audience (Bearer \$ACTIONS_ID_TOKEN_REQUEST_TOKEN)"
    echo "[dry run] would: POST $endpoint {\"jwt\": <oidc token>}"
    publish
    echo "[dry run] would: DELETE $endpoint (Bearer <temporary token>)"
    exit 0
fi

# Credentials go in headers read from a file descriptor and bodies from
# stdin, never on a command line (visible to `ps`).
json_field() { # <field>: prints a string field of the JSON on stdin
    python3 -I -c 'import json, sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"
}

jwt=$(curl -fsS -H @<(printf 'Authorization: Bearer %s\n' "$ACTIONS_ID_TOKEN_REQUEST_TOKEN") \
    "${ACTIONS_ID_TOKEN_REQUEST_URL}&audience=${audience}" | json_field value)
echo "::add-mask::$jwt"

token=$(printf '{"jwt":"%s"}' "$jwt" | curl -fsS -X POST \
    -H 'Content-Type: application/json' -H 'User-Agent: craftainer-release-publish' \
    --data-binary @- "$endpoint" | json_field token)
echo "::add-mask::$token"

revoke() {
    if curl -fsS -o /dev/null -X DELETE -H @<(printf 'Authorization: Bearer %s\n' "$token") \
        -H 'User-Agent: craftainer-release-publish' "$endpoint"; then
        echo "publish: temporary crates.io token revoked"
    else
        echo "publish: WARNING: could not revoke the temporary token (it expires after 30 minutes)" >&2
    fi
}
trap revoke EXIT

CARGO_REGISTRY_TOKEN=$token publish
