# scripts/

- `develop.sh` — `../.devcontainer/Dockerfile`'s `develop`-stage setup: installs `prek`,
  the Claude Code CLI, `snip` and the GitHub CLI (`gh`), the only tooling this template requires
  regardless of instance language. An instance adds its own language
  runtime/tooling install on top, either by extending this script or
  copying its shape into a second script. Prefer the hook below over
  either. Every download goes through its `fetch` helper (bounded curl
  retries with backoff; GitHub URLs authenticated when the optional
  `github_token` BuildKit secret is present) and the one tool that
  downloads for itself (`uv python install`) runs through `retry` (never
  around a `fetch`, so the two layers of retries do not multiply; a failed
  download names what failed and the URL before the script aborts, and
  `fetch` itself retries DNS and
  connect failures (curl exit 6/7) and truncated transfers (18/52/55/56) up
  to 4 times; timeouts are left to curl's own `--retry`), so CI's shared-IP
  HTTP 429s don't fail the image build. Local builds need no token.
  No vendor install script is run: `uv` and `prek` are release tarballs
  checked against the `.sha256` published beside them (`fetch_checked`),
  and Claude Code's binary is checked against its GPG-signed
  `manifest.json` (key fingerprint pinned in `claude_signing_fpr`); see
  `../docs/TEMPLATE.md`'s "Supply chain and release hardening".
- `post-create.sh` — the devcontainer's `postCreateCommand`: git
  `safe.directory`, `prek install`, then every `post-create.d/*.sh`.
- `crate_info.py` (+ `test_crate_info.py`, run by a hook; `test_release_contract.py` covers the Makefile's `publish` / `publish-packages` split and the `verify-published` probe's isolation) — the one place that
  reads the crate name and version from `Cargo.toml` and computes a crate's
  crates.io sparse-index URL and published versions; the Makefile, the
  release scripts and the semver-checks hook call it.
- `release-check.sh`, `release-verify-published.sh`, `release-lib.sh`,
  `release-publish.sh`, `release-stamped.sh`,
  `release-stamp-version.py` — the `Makefile`'s release contract
  (`docs/TEMPLATE.md`, "Rust crate template notes"): `release-check.sh`
  smoke-tests the packaged crate (it gates the release, so only the local
  artifact), `release-verify-published.sh` (`make verify-published`, after
  `publish-packages`, only when a `*_PUBLISH` variable is set) builds a
  throwaway crate against the version just published, with no secrets,
  built from its temp dir outside the repo (so the repo's `.cargo/config.toml`
  `[patch]`, source replacement and rustflags cannot apply) with the
  channel read from `rust-toolchain.toml` and passed as `RUSTUP_TOOLCHAIN`;
  `release-lib.sh` holds the `probe` helper both use; `release-publish.sh` (`make
  publish-packages`) compares the re-packaged crate with `dist/`'s and then runs
  `cargo publish --no-verify` with a bootstrap token or crates.io trusted publishing:
  everything before it (the re-package, `cargo metadata`) runs without credentials, and
  the publish itself keeps only `CARGO_REGISTRY_TOKEN`, so nothing that builds or
  resolves dependencies ever holds a credential;
  `release-stamped.sh --scrub` runs its command without the CI credentials in its environment (`make build`'s `cargo package`),
  `release-stamped.sh` runs a command with `RELEASE_VERSION` stamped into
  `Cargo.toml`/`Cargo.lock` (restored afterwards) using
  `release-stamp-version.py`. It runs `cargo fetch --locked` first, because
  the stamp's `cargo update --offline` needs the locked dependencies in the
  local registry cache, which a fresh runner lacks; both `cargo` calls run
  scrubbed.
- `lib/cargo-scrub.sh` — `scrubbed [--keep NAME] <command>...`, the credential
  scrub the release scripts share (sourced, or run as
  `scripts/lib/cargo-scrub.sh <command>...`); `test_release_scrub.py` records
  each `cargo` call's environment with a stub `cargo`.
- `develop.d/`, `post-create.d/` — instance hook directories, empty apart
  from their own `README.md`; see `docs/TEMPLATE.md`'s "Instance extension
  points".

## Do

- Pass a token to the build as a BuildKit secret (`github_token`), never
  a `--build-arg`/`ARG`: an ARG is baked into the image history.
- Keep this script's own tools pinned to an exact version (a `# renovate:`
  ARG in `../.devcontainer/Dockerfile`) and checksum-verified where the upstream project
  publishes checksums, matching the pattern each install step here already
  follows.
- Run anything that compiles dependency code (`cargo package`'s verify, `cargo build`)
  or resolves the dependency graph (`cargo fetch`, `update`, `metadata`)
  without registry, OIDC or `GITHUB_TOKEN` credentials in its environment
  (`scrubbed`); publish with `--no-verify`, and give only that call
  `CARGO_REGISTRY_TOKEN`.

## Don't

- Add a language runtime or stack-specific tool here — that belongs in the
  instance's own layer (`develop.d/`), not the base template's.
