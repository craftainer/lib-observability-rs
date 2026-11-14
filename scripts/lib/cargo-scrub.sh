#!/usr/bin/env bash
# shellcheck shell=bash
# The credential scrub for every step that runs third-party build code
# (build.rs scripts, proc-macros) or resolves the dependency graph (cargo
# fetch, update, metadata, package). Sourced by release-stamped.sh and
# release-publish.sh; also runnable: `scripts/lib/cargo-scrub.sh <command>...`
# is `scrubbed <command>...`.
#
# scrubbed [--keep NAME]... <command> [args...]: runs the command with the CI
# credentials removed from its environment: every GITHUB_*, ACTIONS_*,
# RUNNER_*, GH_* and INPUT_* variable (GITHUB_TOKEN, ACTIONS_RUNTIME_TOKEN,
# ACTIONS_ID_TOKEN_REQUEST_*, ...), every OCI_* variable (the image registry
# login the build job holds), the cargo registry tokens (CARGO_REGISTRY_TOKEN,
# CARGO_REGISTRIES_<name>_TOKEN), NODE_AUTH_TOKEN and NPM_TOKEN. Cargo's
# non-secret registry settings (CARGO_REGISTRIES_<name>_INDEX, ...) stay.
# --keep NAME spares one variable: the final `cargo publish` keeps
# CARGO_REGISTRY_TOKEN and nothing else of this list.
#
# A registry that needs a CARGO_REGISTRIES_<name>_TOKEN to resolve or fetch
# dependencies is not supported under the scrub; this template uses crates.io.
scrubbed() {
    local name keep=() unset_args=() k spare
    while [[ ${1:-} == --keep ]]; do
        keep+=("${2:?scrubbed: --keep needs a variable name}")
        shift 2
    done
    while IFS= read -r name; do
        if [[ $name =~ ^(GITHUB_|ACTIONS_|RUNNER_|GH_|INPUT_|OCI_) ||
            $name == CARGO_REGISTRY_TOKEN || $name =~ ^CARGO_REGISTRIES_.*_TOKEN$ ||
            $name == NODE_AUTH_TOKEN || $name == NPM_TOKEN ]]; then
            spare=
            for k in ${keep[@]+"${keep[@]}"}; do
                [[ $k == "$name" ]] && spare=1
            done
            [[ -n $spare ]] || unset_args+=(-u "$name")
        fi
    done < <(compgen -e)
    env ${unset_args[@]+"${unset_args[@]}"} "$@"
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
    set -euo pipefail
    scrubbed "$@"
fi
