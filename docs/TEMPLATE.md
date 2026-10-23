# Template conventions

Everything about this repository's structure, tooling, and workflow that's
identical across every instance of this template — as opposed to the root
`README.md`'s short, instance-owned preface. See `../CLAUDE.md`'s "Keeping
this file current" for where a new convention belongs.

`template-base` ships the generic "devcontainer + CI + AI-assisted
workflow" scaffold only — no application runtime, no backing services, no
language assumed. It carries no `src/`/`tests/` of its own. An instance
(a Python/Node.js/Rust/Go library, an infra-only repo, or a full
application) adds its own language runtime, backing services, and release
artifact on top of the shape described here.

## Contents

- `.devcontainer/` — the devcontainer setup; see its `README.md`. An
  instance that needs backing services adds a `stack/` directory here,
  following the "Devcontainer stack pattern" convention this template's
  own instances (e.g. `template-fastapi`) already establish.
- `.github/` — CI and release workflows; see its `CONTENTS.md`.
- `.vscode/` — editor settings, tasks; see its `README.md`.
- `.claude/` — Claude Code CLI project config; see its `README.md`.
- `.mcp.json` — project-scope MCP servers not covered by a
  `.claude/settings.json` plugin; see `.claude/README.md`.
- `docs/` — knowledge about what the instance does; this file is the one
  exception, documenting the template itself rather than product/domain
  knowledge. `adrs/`, `frs/`, `nfrs/`, `plans/` ship only their own
  `README.md` + `template.md` scaffolding — no numbered content, which is
  entirely instance-owned.
- `.secrets/` — local secret files, never committed; see its `README.md`.
- `.devcontainer/Dockerfile` — a single `develop` stage (the devcontainer
  image), with its build-context excludes in
  `.devcontainer/Dockerfile.dockerignore`. The repo root has no
  `Dockerfile`: that one name is reserved for an instance's own runtime
  image (see "Instance extension points"), so a repo never has more than
  one top-level Dockerfile.
- `scripts/develop.sh`, `scripts/post-create.sh` — the `develop` stage's
  setup script and the devcontainer's `postCreateCommand`, each running
  an instance hook directory (`develop.d/`, `post-create.d/`); see
  `scripts/README.md` and "Instance extension points" below.
- `Makefile` — the release contract `release.yml` drives
  (`build`/`sbom`/`release-assets`/`publish`/`publish-packages`); here it packages the crate
  — see "Release: a Makefile contract" and "Rust crate template notes"
  below.
- `.pre-commit-config.yaml` — git hooks, run by `prek` or `pre-commit`;
  language-agnostic hooks only.
- `CLAUDE.md` — the AI-assisted coding workflow Claude Code follows in
  this repository; general conventions live in this file and each
  directory's own `README.md` instead.

## Instance extension points

Base-owned files stay `replace` tier (a sync overwrites them) and expose
hooks an instance fills with files it owns, so an instance never edits a
base-owned file directly. Don't edit `.devcontainer/Dockerfile`, `scripts/develop.sh`
or `compose.yml` in an instance; use the hooks below.

| Hook | Base side (template-owned) | Instance side (instance-owned) |
|---|---|---|
| Extra devcontainer tooling | `.devcontainer/Dockerfile` `develop` stage runs every `scripts/develop.d/*.sh` after `develop.sh` | `scripts/develop.d/NN-name.sh` |
| Extra post-create steps | `scripts/post-create.sh`, run by `devcontainer.json`'s `postCreateCommand`, runs every `scripts/post-create.d/*.sh` | `scripts/post-create.d/NN-name.sh` |
| Extra compose services / dev-service settings | `devcontainer.json` `"dockerComposeFile": ["compose.yml", "compose.instance.yml"]` | `.devcontainer/compose.instance.yml` (base ships a stub, `ignore` tier) |
| Runtime image | nothing (the devcontainer image lives in `.devcontainer/`) | top-level `Dockerfile` + `.dockerignore`, listed in `.github/template-sync-manifest.local.yml`'s `ignore:` |

Files with no include mechanism of their own (JSON configs,
`.pre-commit-config.yaml` — prek has no include) are `merge` tier, so an
instance's additions survive a sync. The runtime image is the
top-level `Dockerfile` with a plain `.dockerignore`, both instance-owned:
base keeps the devcontainer image and its excludes under
`.devcontainer/`, where BuildKit picks up
`.devcontainer/Dockerfile.dockerignore` for that build automatically.

**One-time upgrade (instances synced from before this layout).** A sync
adds `.devcontainer/Dockerfile` but can't delete the old top-level
`Dockerfile`/`.dockerignore`: from this release on, base's manifest no
longer classifies those paths, and a merge-tier file is never deleted
automatically. In the sync PR, by hand:

- With a runtime image: rename `app.Dockerfile` → `Dockerfile` and
  `app.Dockerfile.dockerignore` → `.dockerignore`, overwriting the old
  base files. Update every reference (`-f app.Dockerfile` in the
  `Makefile`, `dockerfile:` in compose files, CI, docs) and list both
  paths under `ignore:` in `.github/template-sync-manifest.local.yml`.
- Without one: delete the old top-level `Dockerfile` and `.dockerignore`.

## Getting started

1. Open this folder in a devcontainer (VS Code: "Reopen in Container" —
   `.vscode/extensions.json` recommends the extension that offers this —
   or any tool that reads `.devcontainer/devcontainer.json`). This builds
   the `develop` stage and installs the git hooks via `postCreateCommand`
   (`scripts/post-create.sh`).
2. There's nothing to run yet — `template-base` ships no application. An
   instance documents its own "run the app" / "run the CLI" step here.

Without a devcontainer: install [`prek`](https://prek.j178.dev/) and run
`prek install` once to enable the git hooks; install whatever else your
instance's own toolchain needs.

## Checks

`.pre-commit-config.yaml` defines whitespace/EOF fixers, YAML/TOML/JSON
checks, `conventional-pre-commit` (Conventional Commits, enforced at the
`commit-msg` stage), the `template-sync-manifest` completeness check, and
the `devcontainer-lock` lockfile-consistency check. An instance layers
its own language-specific hooks on top (lint,
format, type check, test suite, dependency-vulnerability scan, ...),
matching the shape `template-fastapi`'s own `.pre-commit-config.yaml`
already establishes for `ruff`/`mypy`/`pytest`/`pip-audit`.

Run everything at once with:

```bash
prek run --all-files --hook-stage manual
```

Every hook except the commit-message check carries the `manual` stage,
so the command above runs everything else regardless of which git hook
would normally trigger it. Commit messages can only be checked by
actually committing (see the comment in `.pre-commit-config.yaml`).

CI (`.github/workflows/checks.yml`) runs the same `--hook-stage manual`
command, inside the devcontainer itself, on every push and pull request
— as an `amd64`/`arm64` matrix, both legs native (no QEMU).
`.github/workflows/release.yml` is triggered manually to cut an
alpha/beta/rc/full release — see "Release: a Makefile contract" below and
`.github/workflows/README.md`.

## Release: a Makefile contract

`release.yml` computes the next SemVer tag (`compute_next_version.py`,
unchanged across every instance), then drives these targets:

| Target | Runs | Where | Env available |
|---|---|---|---|
| `release-arches` | once, first | host runner | none; must print a JSON array on stdout, e.g. `["amd64"]` |
| `build`, `sbom`, `release-assets`, `publish` | once **per arch**, in that order | host runner for that arch | `RELEASE_VERSION`, `RELEASE_TAG`, `RELEASE_ARCH`, `OCI_REGISTRY`, `OCI_IMAGE_NAME`, `OCI_REGISTRY_USERNAME`, `OCI_REGISTRY_PASSWORD`, all `GITHUB_*`; no package-registry variables or secrets, and no OIDC token |
| `publish-packages` | once, after all arches, **only if** `PYPI_PUBLISH`, `NPM_PUBLISH` or `CARGO_PUBLISH` is set | host runner (own job, with `id-token: write`) | `RELEASE_VERSION`, `RELEASE_TAG`, `PYPI_PUBLISH`, `NPM_PUBLISH`, `CARGO_PUBLISH`, all `GITHUB_*`, the OIDC token, `NPM_TOKEN` and `CARGO_REGISTRY_TOKEN` (bootstrap secrets; empty when unset, so treat empty as absent); `dist/` holds every arch leg's files |
| `release-check` | once, after all arches | **inside the devcontainer** (`devcontainers/ci`), stack up | `RELEASE_VERSION`, `RELEASE_TAG`, `GITHUB_REPOSITORY`, `PYPI_PUBLISH`, `NPM_PUBLISH`, `CARGO_PUBLISH` |
| `finalize` | once, after `release-check` | host runner | same as per-arch minus `RELEASE_ARCH` |

What each target is for: `build` produces the release artifact(s) (an
image, `cargo build --release`, `npm pack`, `helm package`, ...); `sbom`
writes an SBOM for what `build` produced; `release-assets` populates
`dist/` (gitignored) with every file to attach to the GitHub release;
`publish` pushes the per-arch image to its registry, skipping cleanly when
`OCI_REGISTRY` isn't set; `publish-packages` uploads packages (PyPI, npm,
crates.io) once per release, never once per arch, and runs in a job that
exists only for instances that set one of the `*_PUBLISH` variables, so
an image-only instance never has `id-token: write` or a package secret
(base's default fails if any `*_PUBLISH` variable is set, so an instance
can't silently skip it; with none set it is a no-op; it runs on a fresh runner with a checkout and the
downloaded `dist/` only, so it uploads from `dist/` and sets up its own
tooling); `release-check` runs checks that need the
backing services (e.g. tests producing a coverage report); `finalize`
does cross-arch work such as combining per-arch registry tags into one
multi-arch manifest list. `RELEASE_VERSION` (e.g. `1.2.3` or
`1.2.3-alpha.1`) and `RELEASE_TAG` (the same, `v`-prefixed) are for
embedding or tagging with the version. A single-arch no-op is the
default: `release-arches` prints `["amd64"]`.

**Upgrading a package instance.** Before this contract, `publish` also
uploaded packages, once per arch. Now `publish` is image-only and gets no
`*_PUBLISH` variables, secrets or OIDC token. An instance that publishes
packages must move that upload from `publish` into a new
`publish-packages` target (reading artifacts from `dist/`, setting up its
own tooling). Until it does, base's default `publish-packages` (if the
instance kept it) fails with a message naming the variable that is set;
an instance whose Makefile has no such target fails with `make: No rule
to make target`. Either way the release stops instead of skipping the
upload.

**Package instance checklist.** An instance that publishes a package also:
sets `repository`, homepage and bugs URLs in its package manifest
(`package.json`, `pyproject.toml`, `Cargo.toml`) -- npm provenance
verifies the repository URL against the publishing repository, and the
other registries show these links to users; and implements `publish-packages` itself, which replaces base's
default (the `DefaultPublishPackagesTest` in `.github/scripts` skips itself
once the default message is gone). `release-check` receives
`GITHUB_REPOSITORY`, so an instance can run repository guards there.

`dist/` rules: each arch leg's `dist/` is uploaded as artifact
`release-assets-<arch>`, and the final job downloads all of them into one
`dist/`, so **per-arch file names must include the arch**. `release-check`
and `finalize` may add more files to `dist/`. `release.yml` then runs
`gh release create` with every file left in `dist/`.

The arch legs run natively, not under QEMU: `arm64` on `ubuntu-24.04-arm`,
`amd64` on `ubuntu-24.04`, each overridable with the `CI_RUNNER_ARM64` /
`CI_RUNNER_AMD64` repository/organization variables.

This repo's own `Makefile` implements every target as a documented
no-op, since `template-base` ships no artifact — it's both a working
example of the contract's shape and what `release.yml` needs to run
cleanly if this repo itself is ever released. An instance's `Makefile` is
not template-owned (it's the whole point that it differs per artifact
type), so it's tracked like any other instance-owned file — untouched by
this template's own sync manifest.

## Template sync

Once instantiated, a repo created from this template can pull in later
template fixes/improvements via `.github/workflows/template-sync.yml`
(itself template-owned, `replace`-tier): on a schedule (cadence set by
the `TEMPLATE_SYNC_INTERVAL` repository variable, `weekly` or `monthly`)
or on demand, it diffs the instance against the template's latest tagged
release, per `.github/template-sync-manifest.yml`'s three tiers
(`replace`, `ignore`, `merge` — see that file's header), and opens a PR
with the result. It never pushes directly or auto-merges; a genuine
`merge`-tier conflict is left with `<<<<<<<` markers for a human to
resolve. An instance that predates this workflow bootstraps its
`.github/template-sync-state.json` via the workflow's manual
`initial_sync_tag`/`template_repo` inputs first.

An instance that isn't itself a template shouldn't edit that manifest to
list its own files (`src/`, say): it's `merge`-tier, so edits would
conflict with later syncs. It lists them under an `ignore:` key in the
instance-owned
`.github/template-sync-manifest.local.yml` instead; the
`template-sync-manifest` check merges that file in, but sync never reads
it, and it accepts no other tier.

A repo can itself be both an instance of `template-base` *and* its own
template for further instances (e.g. `template-fastapi`): its own
`.github/template-sync-manifest.yml` classifies its *own* tracked files
for *its* downstream instances, entirely separate from this template's
manifest (it edits the manifest directly and resolves base's additions by
hand) — see that repo's own docs for the two-hop chain this produces.

## Versions and config

Every version and config value is defined in exactly one place; nothing
duplicates or re-pins it elsewhere. Everything pinned here (base image,
Actions, hook revisions, `prek`/Claude Code CLI/`snip`/`gh`/`uv`/Python/
Node.js versions) is pinned once, at its single point of use, to an exact
patch version — never a floating range or `latest` — so Renovate can
bump them one at a time and the diff shows exactly what changed. An
instance's own language runtime/package versions follow the same rule in
its own Dockerfile/manifest.

`python3` and Node.js, both installed by `scripts/develop.sh` and pinned
via `.devcontainer/Dockerfile`'s `PYTHON_VERSION`/`NODE_VERSION` ARGs, are
infrastructure tooling only — not an application runtime this template
assumes. `python3` exists because `prek` needs an interpreter to build
the venv for any `language: python` hook (`.pre-commit-config.yaml`'s
`pre-commit-hooks` repo) and because `.github/scripts/*.py` need one
directly; `uv` (its own `UV_VERSION` ARG) is only the mechanism used to
install that exact, checksum-verified CPython build rather than an
unpinned apt package, and stays on `PATH` afterward. Node.js exists
because `npx` (the `clear-thought` MCP server in `.mcp.json`) needs it.
An instance that adds Python or Node.js as its own application runtime
can reuse these directly rather than installing a second copy.

## Rust crate template notes

`template-rust-crate` extends `template-base` for a publishable library
crate. The sections above still apply; these are the crate-specific
additions.

- **Crate scaffold (instance-owned).** `Cargo.toml`, `Cargo.lock`, `src/`,
  `tests/` and `deny.toml` are `ignore` tier. A crate renames itself to
  `craftainer-<name>` (the root `README.md` lists every place the name
  appears, including the `use template_rust_crate::…` paths in `tests/` and
  the doctest in `src/lib.rs`) and owns its dependencies, code, tests and
  license/advisory policy. Dependency versions are kept fresh by Renovate
  (`cargo` manager), not by sync.
- **Template-owned tooling (`replace`).** The `Makefile`,
  `scripts/release-*`, `scripts/develop.d/10-rust.sh`,
  `scripts/post-create.d/10-cargo-fetch.sh`, `rust-toolchain.toml` and
  `rustfmt.toml`. Don't edit them in a crate; change them here and release
  a new tag. `.pre-commit-config.yaml`, `.gitignore`, `README.md` and the
  docs are `merge` tier.
- **Three pins move together.** The Rust version appears in
  `scripts/develop.d/10-rust.sh` (`RUST_VERSION`, the devcontainer
  toolchain), `rust-toolchain.toml` (`channel`, for anyone outside the
  devcontainer) and `Cargo.toml` (`[workspace.package] rust-version`, as
  `major.minor`). A mismatch makes the devcontainer and CI disagree on the
  compiler. `.github/renovate.json` bumps all three in one grouped PR (the
  "rust toolchain" group, not automerged); if you bump by hand, change all
  three.
- **Checks.** Run through `prek` like every other check:

  | Hook | Command | Stage |
  |---|---|---|
  | `cargo-fmt` | `cargo fmt --all --check` | commit |
  | `cargo-clippy` | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | commit |
  | `cargo-lock-check` | `cargo metadata --locked` (a stale `Cargo.lock` fails) | push |
  | `cargo-doc` | `cargo doc` with `RUSTDOCFLAGS=-Dwarnings` | push |
  | `cargo-llvm-cov` | unit and integration tests, `--fail-under-lines 100` | push |
  | `cargo-test-doc` | `cargo test --doc` (llvm-cov doesn't run doctests on stable) | push |
  | `cargo-deny` | licenses, advisories, bans, sources (`deny.toml`) | push |
  | `cargo-semver-checks` | API compatibility against the latest crates.io release | push |

  `cargo-semver-checks` needs a published baseline. The hook asks the
  crates.io sparse index for the root package's name (read from
  `Cargo.toml`): HTTP 200 runs the check, 404 (a crate's first release, and
  this template) skips with a message, anything else fails. An instance
  with a virtual workspace sets `CRATE` (the name and index URL come from
  `scripts/crate_info.py`, which the Makefile and release scripts share).
  The hook passes `--release-type "${SEMVER_RELEASE_TYPE:-minor}"`: the
  in-git version is a permanent `0.0.0`, so without it cargo-semver-checks
  would infer the bump from the version number. A maintainer planning a
  breaking release runs the hook with `SEMVER_RELEASE_TYPE=major`; use
  `patch` for a release that must not add API. `prek ... --hook-stage manual`
  runs all of them.
- **Build output and CARGO_HOME.** `target/` is in the repository
  directory (a bind mount in the devcontainer, gitignored), so a large
  build lands on the host disk; set `CARGO_TARGET_DIR` to a path inside the
  container to keep it in the container's writable layer. `CARGO_HOME`
  keeps its default (`~/.cargo`), also in the writable layer, so the
  registry cache is rebuilt after a devcontainer rebuild
  (`scripts/post-create.d/10-cargo-fetch.sh` refills it). The toolchain
  itself (`~/.rustup`) is baked into the image.
- **`deny.toml` warnings.** The license allow-list is longer than the
  template's (empty) dependency graph needs, so `cargo deny` prints
  "license was not encountered" warnings. They are expected and not
  errors; prune the list once the crate's dependencies settle.
- **Release.** The version lives only in the tag. `make build` stamps
  `RELEASE_VERSION` into `Cargo.toml` (both `[package]` and
  `[workspace.package]`) and `Cargo.lock` for the duration of the command
  and restores them, then runs `cargo package --locked` and copies the
  `.crate` into `dist/`. `make sbom` runs Syft over the unpacked crate
  (SPDX JSON). `make publish` is a no-op (image-only, per arch).
  `make publish-packages` (once, after every arch leg, in its own job)
  skips with a `::warning::` unless `CARGO_PUBLISH` is non-empty; with it, it authenticates with the bootstrap `CARGO_REGISTRY_TOKEN` if
  one is set (a crate's first release), otherwise by crates.io trusted
  publishing (below). `make release-check` runs in the devcontainer: it
  unpacks `dist/*.crate`, runs its doctests and builds a throwaway binary
  crate that depends on it by path; with `CARGO_PUBLISH` set it also polls
  the sparse index for the new version and builds a crate that depends on
  `=<version>` from crates.io. A version that never appears (index lag)
  ends in a notice and a pass, not a failure. `make build` finds the
  `.crate` through `cargo metadata`'s `target_directory`, so a
  `CARGO_TARGET_DIR` (or `build.target-dir`) outside the repo works.
  `make publish-packages` is idempotent: a version already in the sparse index is
  skipped with a notice, so re-running a failed release job completes it.
  For a virtual workspace set `CARGO_PACKAGE` (both `build` and
  `publish-packages` then pass `-p`).
- **Trusted publishing from `make`.** `release.yml` is base-owned and can't
  add the `rust-lang/crates-io-auth-action` step, so `scripts/release-publish.sh`
  does what that action does: request a GitHub OIDC token for audience
  `crates.io` (`ACTIONS_ID_TOKEN_REQUEST_URL` + `&audience=crates.io`,
  Bearer `ACTIONS_ID_TOKEN_REQUEST_TOKEN`; present because the
  `publish-packages` job has `id-token: write`; the per-arch `build` job has
  none), `POST https://crates.io/api/v1/trusted_publishing/tokens`
  with `{"jwt": ...}` for a token valid 30 minutes, publish with it as
  `CARGO_REGISTRY_TOKEN`, and `DELETE` the same URL (Bearer token) to
  revoke it. Tokens are masked in the log and never put on a command line.
  `RELEASE_DRY_RUN=1 CARGO_PUBLISH=1 make publish-packages` prints which path is
  chosen and every step without any network call or publish.
- **Runner toolchain.** `build` and `publish-packages` run on bare runners, not
  in the devcontainer: the runner's `rustup` installs the pinned toolchain from
  `rust-toolchain.toml` (`rustup toolchain install`), and `docker` runs Syft.
  `publish-packages` has a checkout and the downloaded `dist/` only. Cargo
  cannot upload a prebuilt `.crate`, so it stamps the version again and runs
  `cargo publish` from the checkout; `release-publish.sh` uses `dist/`'s
  `.crate` to tell whether the version is already on crates.io with the same
  contents.
- **Syncing an existing crate instance** (from a template older than 1.1.0,
  which published from `publish` and so needed base 1.9.0's contract). The
  `Makefile` and `scripts/release-*.sh` are replace tier, so one sync brings
  in the new `publish-packages` target together with base's `release.yml`,
  whose `build` job no longer has `id-token: write` or `CARGO_REGISTRY_TOKEN`.
  Nothing to do by hand unless the instance edited its `Makefile`: then
  rename its `publish` to `publish-packages`, add it to `.PHONY` and make
  `publish` a no-op. An instance that stays on the old `Makefile` fails
  loudly at `publish-packages` when `CARGO_PUBLISH` is set.

## Code style

Every file gets a brief header stating what the file *is for* — one
line, sometimes two: a leading comment for most formats, a module
docstring where the language has one. Never describe the file's contents
in the header; that's what reading the file is for. Markdown files' own
title/opening line already serves this purpose. A format with no comment
syntax (`.json`) documents itself via the directory's `README.md`
instead.

A comment earns its place by saying something the code/config next to it
can't: *why* it's written this way, a non-obvious consequence, or a
constraint that isn't visible locally. Don't add a comment that just
restates what the following line already says in code — if removing a
comment loses no information, remove it. Every existing comment in this
repository follows this rule; keep new ones held to the same bar.

## Do

- Read `CLAUDE.md` and the `README.md` of every directory on the path to
  whatever you're changing before you change it.
- Open this repo in the devcontainer rather than assembling the
  toolchain by hand — it's the one environment this template guarantees.

## Don't

- Commit a `.env` file, or read one from application code — configuration
  lives in the compose files; secrets live in `.secrets/`.
- Add a `ports:` mapping or a `networks:` block to any file under
  `.devcontainer/`.
- Assume a language runtime, backing service, or release artifact exists
  in this template — every one of those is instance-owned.
