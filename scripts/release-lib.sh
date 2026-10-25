# shellcheck shell=bash
# Shared by release-check.sh (the local crate) and release-verify-published.sh
# (the published release); sourced, not run.

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
