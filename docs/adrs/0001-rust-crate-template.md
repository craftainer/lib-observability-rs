# 0001. Provide a Rust crate template with crates.io trusted publishing

## Status

Proposed

## Context

The generic CRUD and observability layers of `template-axum` are being
extracted into libraries that several services consume, published to
crates.io under the `craftainer-<name>` names. Each crate needs the same
quality bar (formatting, `clippy -D warnings`, a 100% coverage gate,
dependency audit, API-compatibility checks, the devcontainer and CI from
`template-base`) and a release path. Copying that by hand into every crate
would let the copies drift.

`template-base` is the upstream for language-agnostic scaffolding and
template sync, but ships no toolchain and a no-op release Makefile.
`template-python-package` solved the same problem for PyPI (its ADR 0001).

Publishing needs credentials. A long-lived crates.io token stored as a
repository secret is a standing risk; crates.io supports trusted publishing
through GitHub's OIDC identity instead. Two constraints shape how:

- crates.io has no pending publishers. A trusted publisher can only be added
  to a crate that already exists, so a crate's first release needs another
  credential.
- `template-base`'s `release.yml` is base-owned (template sync overwrites
  it), so this template can't add the step crates.io's documentation shows,
  `rust-lang/crates-io-auth-action`. `release.yml` does grant
  `id-token: write` to the build job, which runs `make publish`.

## Decision

We will create `craftainer/template-rust-crate`, an instance of
`template-base` that is itself a template, and create every Rust library
from it. It contains:

- a Cargo workspace (resolver 3, edition 2024) with one example library
  crate, lints and `rust-version` set once in `[workspace]`, the toolchain
  pinned in `rust-toolchain.toml` and installed by `scripts/develop.d/10-rust.sh`
  (one version, three files, bumped together by one Renovate group);
- `cargo fmt --check`, `clippy -D warnings`, a 100% `cargo-llvm-cov` line
  gate plus doctests, `cargo-deny` and `cargo-semver-checks` (skipped until
  the crate has a crates.io baseline), all run by `prek`;
- a `Makefile` implementing `template-base`'s release contract: `build` runs
  `cargo package --locked` (the release version is stamped into `Cargo.toml`
  and `Cargo.lock` only for the command and restored, so the tag stays the
  only home of the version), `sbom` runs Syft over the unpacked crate,
  `release-arches` is `["amd64"]`, and `release-assets` is the `.crate` and
  the SBOM;
- `make publish`, enabled by the `CARGO_PUBLISH` repository variable and
  skipped when it is empty. With a non-empty `CARGO_REGISTRY_TOKEN` (the
  first-release bootstrap secret) it publishes with that token. Otherwise
  it performs the trusted-publishing exchange itself, in
  `scripts/release-publish.sh`, copying what `crates-io-auth-action` does:
  GitHub OIDC token for audience `crates.io`, `POST
  https://crates.io/api/v1/trusted_publishing/tokens` for a 30-minute
  token, `cargo publish` with it, then `DELETE` on the same endpoint to
  revoke it. A first release therefore needs a short-lived, crate-scoped
  token; the owner then registers the trusted publisher on crates.io and
  deletes the token;
- `make release-check` (devcontainer, after publish): unpack the `.crate`,
  run its doctests and build a throwaway binary crate depending on it by
  path; with `CARGO_PUBLISH` set, poll the sparse index for the version and
  build a throwaway crate depending on `=<version>` from crates.io. A
  version that doesn't appear within the bounded retry is a notice, not a
  failure;
- the GPL-3.0 license, matching the other templates and the applications
  that consume the libraries.

## Consequences

- A new crate starts with the full quality bar and release path, and
  receives fixes through two sync hops (`template-base`, then this
  template). The `Makefile` and `scripts/release-*` are template-owned, so
  a crate never edits them.
- After the one-time bootstrap token, no crates.io secret exists to leak or
  rotate. The cost is a manual step per crate (add the trusted publisher,
  delete the token) and the dependency on `release.yml` keeping
  `id-token: write` on the build job.
- The token exchange is reimplemented in shell instead of using the
  maintained action. If crates.io changes the endpoint or payload, this
  script must follow; the action's source is the reference. Moving the step
  into `release.yml` would be cleaner but is a change to a base-owned file
  for every family member.
- Packaging and publishing run on the bare runner, which needs `rustup` (it
  installs the toolchain from `rust-toolchain.toml`), Python 3, `curl` and
  Docker (Syft). They are present on GitHub-hosted Ubuntu runners.
- `release-check` can't prove the registry copy when the index is slow: it
  passes with a notice after the retry window, so a broken upload that never
  appears is visible only as that notice in the log.
- The dependency graph is the instance's: template-side bumps of
  `Cargo.toml`/`Cargo.lock` don't propagate by sync; Renovate keeps each
  crate current.
- Library crates only: a crate with a binary, a build script needing
  system libraries, or per-arch artifacts needs a different template.
