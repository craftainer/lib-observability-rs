# shellcheck shell=bash
# Shared by release-check.sh (the local crate), release-verify-published.sh
# (the published release), release-publish.sh and release-stamped.sh --scrub;
# sourced, not run.

# <dir> <crate name> <dependency spec>: a binary crate that links the
# dependency, built with cargo (the caller sets CARGO_TARGET_DIR).
probe() {
    local dir=$1 name=$2 spec=$3
    # The lib target name: hyphens become underscores unless [lib] name says otherwise.
    local module=${name//-/_}
    mkdir -p "$dir/src"
    cat >"$dir/Cargo.toml" <<TOML
[package]
name = "release-probe"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
$name = $spec
TOML
    printf '// Throwaway: proves the dependency resolves and links.\nuse %s as _;\n\nfn main() {}\n' \
        "$module" >"$dir/src/main.rs"
    cargo build --quiet --manifest-path "$dir/Cargo.toml"
}

# scrubbed <command> [args...]: runs the command with the CI credentials
# removed from its environment: every GITHUB_*, ACTIONS_*, RUNNER_*, GH_* and
# INPUT_* variable (GITHUB_TOKEN, ACTIONS_RUNTIME_TOKEN,
# ACTIONS_ID_TOKEN_REQUEST_*, ...), every OCI_* variable (the image registry
# login the build job holds), the cargo registry tokens (CARGO_REGISTRY_TOKEN,
# CARGO_REGISTRIES_<name>_TOKEN), NODE_AUTH_TOKEN and NPM_TOKEN. Cargo's
# non-secret registry settings (CARGO_REGISTRIES_<name>_INDEX, ...) stay.
# For commands that compile dependency code (`cargo package` verifies by
# building, which runs dependency build.rs scripts and proc-macros) in a job
# that holds credentials the build does not need.
scrubbed() {
    local name unset_args=()
    while IFS= read -r name; do
        if [[ $name =~ ^(GITHUB_|ACTIONS_|RUNNER_|GH_|INPUT_|OCI_) ||
            $name == CARGO_REGISTRY_TOKEN || $name =~ ^CARGO_REGISTRIES_.*_TOKEN$ ||
            $name == NODE_AUTH_TOKEN || $name == NPM_TOKEN ]]; then
            unset_args+=(-u "$name")
        fi
    done < <(compgen -e)
    env ${unset_args[@]+"${unset_args[@]}"} "$@"
}
