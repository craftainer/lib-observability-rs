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
| Instance skills and agents | base's manifest lists each of its own skills (`.claude/skills/<name>/`) and agents (`.claude/agents/<name>.md`) one by one under `replace`, not as a directory glob; a deleted base skill or agent keeps its line as a tombstone so sync removes it from instances. An instance's own name must not reuse a base or tombstoned one, since sync would overwrite or remove it | `.claude/skills/<name>/`, `.claude/agents/<name>.md`, listed in `.github/template-sync-manifest.local.yml`'s `ignore:` |
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
| `release-check` | once, after all arches; gates the GitHub release and `publish-packages` (not the per-arch image `publish`, which already ran in `build`) | **inside the devcontainer** (`devcontainers/ci`), stack up | `RELEASE_VERSION`, `RELEASE_TAG`, `GITHUB_REPOSITORY`; **not** `RELEASE_ARCH` (it runs once, not per arch) and no `ARCHES`: to check each arch's files in `dist/`, iterate over `make -s release-arches` |
| `finalize` | once, after `release-check`; then the GitHub release is created | host runner | same as per-arch minus `RELEASE_ARCH` |
| `publish-packages` | once, after the GitHub release exists, **only if** `PYPI_PUBLISH`, `NPM_PUBLISH` or `CARGO_PUBLISH` is set | host runner (own job, with `id-token: write`) | `RELEASE_VERSION`, `RELEASE_TAG`, `PYPI_PUBLISH`, `NPM_PUBLISH`, `CARGO_PUBLISH`, all `GITHUB_*`, the OIDC token, `NPM_TOKEN` and `CARGO_REGISTRY_TOKEN` (bootstrap secrets; empty when unset, so treat empty as absent); `dist/` holds every arch leg's files |
| `verify-published` | once, after `publish-packages` succeeded, **only if** a `*_PUBLISH` variable is set | host runner (own job, no secrets, no OIDC token) | `RELEASE_VERSION`, `RELEASE_TAG`, `PYPI_PUBLISH`, `NPM_PUBLISH`, `CARGO_PUBLISH`; `dist/` as for `publish-packages` |

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
tooling); `verify-published` checks the release from the registry (e.g.
installs the just-published package) and is a no-op in base's default, since
there is nothing to verify unless an instance implements it;
`release-check` runs checks that need the backing services (e.g. tests
producing a coverage report) and the local artifact; `finalize`
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
to make target`. Either way nothing is uploaded, and because the GitHub
release is created first (see "Job order"), the release already exists
when the `publish-packages` job fails: it is out without its packages.
Fix the Makefile and re-run the failed jobs to publish them; do not cut
another release for it.

**Job order.** `build` (per arch) → `release-check` → `release`
(`finalize`, then the GitHub release) → `publish-packages` →
`verify-published`. `release-check` gates both the release and package publishing. It does
not gate the per-arch image `publish`, which runs inside `build`, so a pushed
per-arch image can exist for a release that `release-check` then fails.
The GitHub release exists before any registry upload, so a partial
registry failure leaves the release in place and a re-run of the failed
job finishes publishing. So an instance's `publish-packages` **must be
safe to run again**: it skips versions already on the registry (for
example `uv publish --check-url`, or an `npm view` guard) instead of failing
on them or double-publishing.

The `release` job is re-runnable too: if the release already exists it
skips `finalize` and the release creation (a leftover draft is deleted and
recreated), so re-running failed jobs after a later job failed does not
fail on the existing release. It first checks that the existing
release's tag points at the commit this run built, and fails (rather than
warns) if the tag can't be resolved or points elsewhere
(`.github/scripts/verify_release_tag.sh`). The tag lookup is tried up to six
times over about two minutes (waits of 5, 10, 20, 40 and 60 seconds, enough
for a minute-long secondary rate limit), but only for a transient failure
(HTTP 5xx, 408 or 429, a rate limit, or a network error). A tag that does not
resolve (the API answers 404, or 422 "No commit found") or any other permanent
error fails at once. The 404 message names both causes: for a deleted tag,
restore it or delete the release and run the workflow again; for a release
that was only just created (say by a concurrent run) whose tag is not visible
yet, re-run the job. An API answer without a commit sha is reported as such,
not as an unresolvable tag. If `release` itself failed before the release
existed, `finalize` runs again on the re-run, so an instance's `finalize`
must be safe to repeat (it must not double-apply its side effects).
Because the release is created before `publish-packages` runs, a GitHub
release can exist for a version whose `publish-packages` failed. Re-run the
failed jobs to finish publishing; do not delete the release.
`verify-published` runs only after `publish-packages` succeeded, so after a
partial `publish-packages` failure nothing is verified until the re-run of
the failed jobs, which then runs it.

Two runs for the same tag cannot overlap in the `release` job: it sits in a
`concurrency` group named after the tag (`cancel-in-progress: false`), so the
second waits and then sees the published release. If that release was cut
from a different commit than the waiting run's (two dispatches that computed
the same tag), the waiting run fails instead of publishing its own `dist/`
under that version (detected only here, after the per-arch images of both runs were
pushed). GitHub keeps at most one waiting run per group, and a
newer waiting run replaces it.

A release that exists on GitHub counts as complete: whatever failed after it
is finished by re-running the failed jobs, never by cutting another version.

**Opting into a registry.** The `PYPI_PUBLISH` (`testpypi` or `pypi`),
`NPM_PUBLISH` and `CARGO_PUBLISH` repository variables are what opt an
instance into a registry; with none set, `publish-packages` and
`verify-published` do not run. `NPM_TOKEN` and `CARGO_REGISTRY_TOKEN` are
only for the first release of a package, before trusted publishing is set up
at the registry. See `.github/workflows/README.md`,
"Release variables and secrets".

Re-run only the *failed jobs* of a run. "Re-run all jobs" runs `version`
again, which sees the tag the first attempt created and computes the next
version, so it cuts another release instead of finishing the first.

In `release-check`, if `dist/` holds files owned by another user (the
devcontainer ran as root), the job takes them over with `sudo`, which must
be passwordless (`sudo -n`); the runner images provide that. A self-hosted
runner without it fails there with a message saying so.

Package templates (npm, Python, Rust) move their
registry check ("install what was just published") from `release-check` to
`verify-published`; their `publish-packages` is unchanged and already skips
versions already on the registry. Base's
`release-check` no longer receives the `*_PUBLISH` variables.

**Package instance checklist.** An instance that publishes a package also:
sets `repository`, homepage and bugs URLs in its package manifest
(`package.json`, `pyproject.toml`, `Cargo.toml`) -- npm provenance
verifies the repository URL against the publishing repository, and the
other registries show these links to users; and implements `publish-packages`
(re-runnable: it skips versions already on the registry) in its own `Makefile`, which replaces base's default (the
`DefaultPublishPackagesTest` in `.github/scripts` skips itself once the
default message is gone). That only works where the `Makefile` is
instance-owned, as in base. A downstream template that makes the `Makefile`
replace-tier (an image template, say) gives its instances no place to
implement it, so such a template either ships the `publish-packages` logic
itself, driven by variables the instance sets, or is not a fit for package
instances. `release-check` receives
`GITHUB_REPOSITORY`, so an instance can run repository guards there.
It also moves any "install what was just published" check from
`release-check` to `verify-published` (see "Job order" above).

`dist/` rules: each arch leg's `dist/` is uploaded as artifact
`release-assets-<arch>` (a tarball, so it is never empty), and
`release-check`'s job assembles all of them into one `dist/`, failing if an
arch's upload is missing or two legs produced the same file name, so
**per-arch file names must include the arch**. `release-check` may add
more files to `dist/`, and the assembled `dist/` (uploaded at the end of the
`release-check` job, before `finalize`) is what `release`,
`publish-packages` and `verify-published` start from. `finalize` may add
more files, but they are attached to the GitHub release only:
`publish-packages` and `verify-published` do not see them. A path that two
legs both produce (the same file name, a file in one and a directory of
that name in another, or a symlink) fails the assembly. The assembly also
refuses to run when `dist/` already exists and is not empty, so stray files
cannot end up in a release.
`release.yml` then runs `gh release create` with every file left in
`dist/`.

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

The workflow runs `template_sync.py apply` from the *new* tag's checkout,
with that tag's manifest. Do the same when applying a sync by hand: the
instance's own copy of the script is the old release's, and an older one
does not expand glob entries such as `scripts/release-*.sh`, so it skips
those files (a second run with the new script then picks them up). `apply`
is meant to run once per tag pair: the state file moves to the new tag
afterwards. Running it again is harmless for files already merged cleanly,
but it merges a hand-resolved conflict again and re-creates its markers.

Further sync behaviour, detailed in `.github/scripts/README.md`:

- `apply` is not atomic. CI discards the checkout and does not advance the
  state on an error; after a failed manual run, reset the working tree with
  git before trying again.
- A binary `merge`-tier file the instance changed cannot be merged. It is
  kept as it is, reported as a conflict, listed as `merge.binary` in the
  summary and labelled in the sync PR.
- The moderation `-apply` workflows fail on an expired `moderation-result`
  artifact instead of passing: a produced result was lost, so re-run the
  worker run (see `.github/workflows/README.md`, "Issue moderation").
- The scripts under `.github/scripts/` run on the runner's system `python3`.
  Base ships `.github/scripts/ruff.toml` (`replace` tier), pinned to that
  Python's `target-version`, and `test_script_syntax.py` guards it with
  `ast.parse(feature_version=...)`. A Python instance must not override or
  remove it: its own `pyproject.toml` targets a newer Python and ruff would
  rewrite those scripts into syntax the runner cannot parse.

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

## Template family

`template-base` is the root of a small family. Four mid-layer templates sit
under it, and each library or app is an instance of exactly one of them (the
three app templates through `template-docker`):

```text
template-base
├── template-python-package ── lib-crud-py, lib-observability-py, lib-crud-sdk-py
├── template-npm-package ───── lib-crud-sdk-ts, lib-crud-views-ts
├── template-rust-crate ────── lib-crud-rs, lib-observability-rs
└── template-docker
    ├── template-fastapi
    ├── template-axum
    └── template-react
```

Naming: a library repo is `lib-<name>-<py|ts|rs>` and an app repo is
`template-<runtime>`; the registry package is `craftainer-<name>` on PyPI and
crates.io and `@craftainer/<name>` on npm (`lib-crud-sdk-ts` publishes
`@craftainer/crud-sdk`). Libraries declare `GPL-3.0-or-later` in their package
manifest; base itself ships the GPL v3 text in `LICENSE`.

Each mid-layer template is an instance of base that is itself a template. It
adds one kind of artifact to base's release contract and keeps everything
else from base:

| Template | Adds to base | Tooling it owns (`replace`) | What an instance owns (`ignore`) |
|---|---|---|---|
| `template-docker` | the container-image release: multi-arch build, SBOM, OCI labels, smoke test, Trivy scan, Dockerfile version check, and `publish-packages`/`verify-published` as no-ops | `Makefile`, `scripts/image-*.sh`, `scripts/runner-setup.sh`, `scripts/check-dockerfile-versions.sh`, `scripts/skopeo.sh` | `Dockerfile`, `image.mk`, `scripts/runner-setup.d/*.sh` |
| `template-python-package` | a pure-Python library on PyPI (trusted publishing) | `Makefile`, `scripts/release-*.sh`, `scripts/post-create.d/10-uv-sync.sh`, `codspeed.yml` | `pyproject.toml`, `uv.lock`, `src/`, `tests/` |
| `template-npm-package` | an ESM TypeScript package on npm (staged first publish, then trusted publishing) | `scripts/release-*.sh`, `scripts/develop.d/10-corepack.sh`, `scripts/post-create.d/10-pnpm-install.sh` | `package.json`, `tsconfig*.json`, `tsdown.config.ts`, `vitest.config.ts`, `src/`, `tests/`, `pnpm-lock.yaml`, and the `Makefile` (a thin caller of the scripts) |
| `template-rust-crate` | a library crate on crates.io (trusted publishing, bootstrap-token fallback) | `Makefile`, `scripts/release-*`, `rust-toolchain.toml`, `rustfmt.toml`, the rustup and `cargo fetch` hooks | `Cargo.toml`, `Cargo.lock`, `src/`, `tests/`, `deny.toml` |

Each template's own `docs/TEMPLATE.md` has the details and the exact
manifest; its `docs/adrs/0001-*.md` records why it exists. The three app
templates inherit the image layer from `template-docker`; they are still
the place for their own runtime (FastAPI, axum, React) and their own ADRs.

**Sync tiers across the family.** Every hop uses the same three tiers
(`replace`, `ignore`, `merge`; see "Template sync" and the manifest header).
A mid-layer classifies its own tree for its instances in its own
`.github/template-sync-manifest.yml`, which is `merge` tier in every
instance: base's additions are resolved by hand there.
An instance lists extra paths of its own under `ignore:` in
`.github/template-sync-manifest.local.yml`. Which template an instance
follows is recorded in its `.github/template-sync-state.json`
(`template_repo`, `synced_tag`). An instance follows its immediate parent: a
package or image instance its mid-layer, and the mid-layer follows base.

**Order of a change.** A sync reads the parent's latest tagged release, so a
change travels one released tag at a time:

1. Fix it in `template-base`, merge, and release a new tag.
2. Each mid-layer takes the base release through a sync PR, merges it and
   releases its own tag. A base fix reaches `template-docker` and the
   three package templates this way; the app templates take it from
   `template-docker` in step 3, after it has released.
3. Every instance takes its parent's tag through its own sync PR (the app
   templates take `template-docker`'s; the libraries, their package
   template's).

A fix to a mid-layer's own files starts at step 2 and skips base. A change to
the release contract itself (a new target, a changed job order) is a minor
release of base, and each mid-layer then adapts its own implementation of the
target before instances re-sync; the "Job order" above is the current
result of two such changes. Instances that publish packages follow the
registry's own ordering on top: a package that depends on another is
released after it.

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
  | `cargo-semver-checks` | API compatibility against the latest non-yanked crates.io release | push |

  `cargo-semver-checks` needs a published baseline. The hook asks the
  crates.io sparse index for the root package's name (read from
  `Cargo.toml`): HTTP 200 runs the check against the highest non-yanked
  version (`crate_info.py latest-version`, passed as `--baseline-version`:
  by default cargo-semver-checks wants a baseline at or below the in-git
  `0.0.0` and finds none), 404 (a crate's first release, and this template)
  or a crate with every version yanked skips with a message, anything else
  fails. That one version is used for every workspace member checked. An instance
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
  publishing (below). `make release-check` runs in the devcontainer and
  gates the GitHub release and publishing: it unpacks `dist/*.crate`, runs
  its doctests and builds a throwaway binary crate that depends on it by
  path. `make verify-published` (after `publish-packages`, only when a
  `*_PUBLISH` variable is set, on the bare runner with no secrets) polls the
  sparse index for the new version when `CARGO_PUBLISH` is set and builds a
  crate that depends on `=<version>` from crates.io, built from a temp dir
  outside the repo (the repo's `.cargo/config.toml` cannot apply; the
  toolchain is taken from `rust-toolchain.toml` explicitly). A version that never
  appears (index lag) ends in a notice and a pass, not a failure. `make build` finds the
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
