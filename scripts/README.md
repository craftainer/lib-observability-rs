# scripts/

- `develop.sh` — `../.devcontainer/Dockerfile`'s `develop`-stage setup: installs `prek`,
  the Claude Code CLI, `snip` and the GitHub CLI (`gh`), the only tooling this template requires
  regardless of instance language. An instance adds its own language
  runtime/tooling install on top, either by extending this script or
  copying its shape into a second script. Prefer the hook below over
  either. Every download goes through its `fetch` helper (bounded curl
  retries with backoff; GitHub URLs authenticated when the optional
  `github_token` BuildKit secret is present) and the installers that
  download for themselves through `retry`, so CI's shared-IP HTTP 429s
  don't fail the image build. Local builds need no token.
- `post-create.sh` — the devcontainer's `postCreateCommand`: git
  `safe.directory`, `prek install`, then every `post-create.d/*.sh`.
- `crate_info.py` (+ `test_crate_info.py`, run by a hook; `test_release_contract.py` covers the Makefile's `publish` / `publish-packages` split) — the one place that
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
  throwaway crate against the version just published, with no secrets
  (`release-lib.sh` holds the `probe` helper both use), `release-publish.sh` (`make
  publish-packages`) runs `cargo publish` with a bootstrap token or crates.io trusted publishing,
  `release-stamped.sh` runs a command with `RELEASE_VERSION` stamped into
  `Cargo.toml`/`Cargo.lock` (restored afterwards) using
  `release-stamp-version.py`.
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

## Don't

- Add a language runtime or stack-specific tool here — that belongs in the
  instance's own layer (`develop.d/`), not the base template's.
