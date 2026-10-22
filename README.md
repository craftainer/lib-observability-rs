# template-rust-crate

> [!NOTE]
> This repository is vibe coded with [Claude](https://claude.com/product/claude-code).

A template for a tested, linted and publishable Rust library crate: a Cargo
workspace with one example crate, `rustfmt`, `clippy -D warnings`, a 100%
line-coverage gate (`cargo-llvm-cov`), `cargo-deny`, `cargo-semver-checks`,
and a release that publishes to crates.io through trusted publishing. It is
itself an instance of
[`template-base`](https://github.com/craftainer/template-base) (so it gets the
devcontainer, CI, release workflow and template sync from there) and is
itself a template: a crate created from it receives fixes through template
sync. See [`docs/TEMPLATE.md`](docs/TEMPLATE.md) for the conventions shared
by every instance, and
[`docs/adrs/0001-rust-crate-template.md`](docs/adrs/0001-rust-crate-template.md)
for why it exists.

## Creating a crate from this template

1. On GitHub, use "Use this template" on this repository (or
   `gh repo create <org>/<name> --template craftainer/template-rust-crate`).
2. Rename the crate. The convention for this organization is
   `craftainer-<name>` on crates.io (repository `lib-<name>-rs`), imported as
   `craftainer_<name>`. These places move together:
   - `Cargo.toml`: `[package] name` (`template-rust-crate`), the
     description, and `[workspace.package] repository`;
   - `src/lib.rs`: the `template_rust_crate::` path in the doctest;
   - `tests/*.rs`: every `use template_rust_crate::…`;
   - this file's title and description.

   Then run `cargo update --workspace` to refresh `Cargo.lock` for the new
   name, and add `keywords` and `categories` to `[package]`.
3. Point template sync at this template. `.github/template-sync-state.json`
   here records the `template-base` tag this template itself last synced
   from; the copy in a new crate must instead name this repository and the
   tag it was created at (the latest release of
   `craftainer/template-rust-crate`):

   ```json
   {"template_repo": "craftainer/template-rust-crate", "synced_tag": "vX.Y.Z"}
   ```

   Add a `TEMPLATE_SYNC_TOKEN` secret too (see `.github/workflows/README.md`,
   "Template sync"). Files the crate owns beyond the manifest's `ignore`
   tier go in `.github/template-sync-manifest.local.yml` under `ignore:`.
4. Open the repository in the devcontainer, then run
   `prek run --all-files --hook-stage manual`; everything should pass before
   you write any code.
5. Keep `LICENSE` (GPL-3.0) and `deny.toml`'s license allow-list, or change
   both together.

## Publishing to crates.io

`make publish` is a no-op until you opt in, per repository, by setting the
repository variable `CARGO_PUBLISH` to any non-empty value (Settings,
Secrets and variables, Actions, Variables). The version lives only in the
git tag `release.yml` computes; `Cargo.toml` stays at `0.0.0` and
`make build` stamps `RELEASE_VERSION` into the build without committing it.

crates.io has no "pending publisher": a trusted publisher can only be added
to a crate that already exists. So the first release of each crate uses a
short-lived bootstrap token, and trusted publishing takes over after it:

1. Create a crates.io API token scoped to the new crate name
   (`publish-new` only for the first release) with a short expiry, and store
   it as the repository secret `CARGO_REGISTRY_TOKEN`.
2. Set `CARGO_PUBLISH=1` and run the release workflow. `make publish` sees
   the token and publishes with it.
3. On crates.io, open the crate's Settings, Trusted Publishing, and add a
   GitHub publisher: the owner, the repository, the workflow `release.yml`,
   and no environment.
4. Delete the `CARGO_REGISTRY_TOKEN` secret and revoke the token. From the
   next release, `make publish` finds no token and exchanges the job's
   GitHub OIDC identity for a 30-minute crates.io token, publishes with it
   and revokes it.

## License

Licensed under the [GNU General Public License v3.0](LICENSE).
