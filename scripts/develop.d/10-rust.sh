#!/usr/bin/env bash
# Installs the Rust toolchain (rustup, pinned) and the gate tools the
# pre-commit hooks call: cargo-llvm-cov, cargo-deny, cargo-semver-checks.
# Runs as root during the `develop` stage build (see README.md here).
#
# Keep RUST_VERSION in step with rust-toolchain.toml and Cargo.toml's
# `rust-version`.
set -euo pipefail

# renovate: datasource=docker depName=rust
RUST_VERSION=1.99.1
# renovate: datasource=github-releases depName=rust-lang/rustup
RUSTUP_VERSION=1.28.2
# renovate: datasource=github-releases depName=taiki-e/cargo-llvm-cov
CARGO_LLVM_COV_VERSION=0.9.1
# renovate: datasource=github-releases depName=EmbarkStudios/cargo-deny
CARGO_DENY_VERSION=0.20.2
# renovate: datasource=github-releases depName=obi1kenobi/cargo-semver-checks
CARGO_SEMVER_CHECKS_VERSION=0.51.0

# A C linker for rustc (and any -sys crate a dependency adds).
if ! command -v cc >/dev/null; then
    apt-get update
    apt-get install -y --no-install-recommends build-essential pkg-config
    apt-get clean
    rm -rf /var/lib/apt/lists/*
fi

case "$(dpkg --print-architecture)" in
    amd64) arch=x86_64 ;;
    arm64) arch=aarch64 ;;
    *)
        echo "unsupported architecture for the Rust tools" >&2
        exit 1
        ;;
esac
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
chmod a+rwx "$tmpdir"

# rustup-init as a release binary, checksum-verified against
# static.rust-lang.org's published .sha256 (same pattern as template-axum),
# not the unversioned https://sh.rustup.rs script.
rustup_url="https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${arch}-unknown-linux-gnu/rustup-init"
curl -LsSf -o "${tmpdir}/rustup-init" "$rustup_url"
curl -LsSf -o "${tmpdir}/rustup-init.sha256" "${rustup_url}.sha256"
(cd "$tmpdir" && sha256sum -c rustup-init.sha256)
chmod +x "${tmpdir}/rustup-init"
# `minimal` + explicit components keeps the image small; llvm-tools-preview
# is what cargo-llvm-cov instruments with.
sudo -u vscode env HOME=/home/vscode "${tmpdir}/rustup-init" -y \
    --default-toolchain "$RUST_VERSION" --profile minimal \
    -c clippy -c rustfmt -c llvm-tools-preview --no-modify-path
for bin in rustup cargo rustc rustfmt cargo-fmt cargo-clippy clippy-driver rustdoc; do
    ln -sf "/home/vscode/.cargo/bin/${bin}" "/usr/local/bin/${bin}"
done

# Gate tools as prebuilt release binaries instead of `cargo install`: a
# source build of these three takes many minutes and gigabytes of RAM.
# cargo-deny publishes a .sha256 per asset; the other two publish none, so
# the asset's SHA-256 is taken from GitHub's release API (`digest` field,
# computed by GitHub at upload) over TLS.
github_digest() { # repo tag asset
    curl -LsSf "https://api.github.com/repos/$1/releases/tags/$2" \
        | python3 -c 'import json, sys
for a in json.load(sys.stdin)["assets"]:
    if a["name"] == sys.argv[1]:
        print(a["digest"].removeprefix("sha256:")); break
else:
    sys.exit("asset not found: " + sys.argv[1])' "$3"
}
fetch_tarball() { # repo tag asset expected-sha256 -> extracts into $tmpdir/<asset>.d
    curl -LsSf -o "${tmpdir}/$3" "https://github.com/$1/releases/download/$2/$3"
    echo "$4  ${tmpdir}/$3" | sha256sum -c -
    mkdir -p "${tmpdir}/$3.d"
    tar -xzf "${tmpdir}/$3" -C "${tmpdir}/$3.d"
}

asset="cargo-llvm-cov-${arch}-unknown-linux-gnu.tar.gz"
tag="v${CARGO_LLVM_COV_VERSION}"
fetch_tarball taiki-e/cargo-llvm-cov "$tag" "$asset" "$(github_digest taiki-e/cargo-llvm-cov "$tag" "$asset")"
install -m755 "${tmpdir}/${asset}.d/cargo-llvm-cov" /usr/local/bin/cargo-llvm-cov

asset="cargo-semver-checks-${arch}-unknown-linux-gnu.tar.gz"
tag="v${CARGO_SEMVER_CHECKS_VERSION}"
fetch_tarball obi1kenobi/cargo-semver-checks "$tag" "$asset" "$(github_digest obi1kenobi/cargo-semver-checks "$tag" "$asset")"
install -m755 "${tmpdir}/${asset}.d/cargo-semver-checks" /usr/local/bin/cargo-semver-checks

# cargo-deny ships musl builds only.
dir="cargo-deny-${CARGO_DENY_VERSION}-${arch}-unknown-linux-musl"
curl -LsSf -o "${tmpdir}/${dir}.tar.gz" \
    "https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/${dir}.tar.gz"
curl -LsSf -o "${tmpdir}/${dir}.tar.gz.sha256" \
    "https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/${dir}.tar.gz.sha256"
# The .sha256 holds the bare hash (no filename), so rebuild a checksum line.
echo "$(cut -d' ' -f1 "${tmpdir}/${dir}.tar.gz.sha256")  ${tmpdir}/${dir}.tar.gz" | sha256sum -c -
tar -xzf "${tmpdir}/${dir}.tar.gz" -C "$tmpdir"
install -m755 "${tmpdir}/${dir}/cargo-deny" /usr/local/bin/cargo-deny
