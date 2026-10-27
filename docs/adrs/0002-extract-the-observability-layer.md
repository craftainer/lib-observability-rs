# 0002. Extract health checks and logging from template-axum into their own crate

## Status

Accepted

2026-10-26: `/health/info`, listed below as a later item, is now provided by the
`/health/info` router; see [ADR 0006](0006-health-info.md).

## Context

`template-axum` keeps its readiness machinery in `src/health/` (the
`HealthCheck` trait, `HealthRegistry` and the concrete checks), the `/live` and
`/ready` router in `src/generic/controllers/health.rs`, and JSON logging with an
optional OTLP log bridge in `src/telemetry.rs`. A static read of the code (work
item RS4 of the templates, SDKs and UI plan) found the boundary clean: none of
the three imports `crate::hero`, `AppError`, `Settings` or `Mode`. The only
application types in view were the accessor trait `HasHealthRegistry`, which
sat in `generic/controllers/mod.rs`, and `OidcHealthCheck`, which was a generic
HTTP probe in all but name. `template-fastapi` already ships the equivalent as
the `craftainer-observability` package, so the Rust side follows the same shape:
a crate published as `craftainer-observability` (repository
`lib-observability-rs`), built from `template-rust-crate`.

Four questions needed an answer.

1. **What moves.** The three pieces above and the tests that exercise them.
   Request IDs, OpenTelemetry traces and metrics and `/health/info` do not exist
   in `template-axum` yet; RS4 lists them as new items (AX7, AX8, AX9a) on top of
   this crate, so they are not part of this extraction.
2. **How the database check avoids the ORM.** `DatabaseHealthCheck` took a
   `sea_orm::DatabaseConnection`. Depending on SeaORM for one `SELECT 1` would
   put an ORM and its migration stack into every dependent.
3. **Which checks are core.** Each ready-made check pulls a client library
   (`sqlx`, `redis`, `aws-sdk-s3`, `reqwest`), and a service rarely has all
   four.
4. **Which error type.** Nothing in the extracted code returned an error to a
   caller, except that `configure_logging` panicked when called twice.

## Decision

We extract with history, with `git filter-repo` run on a throwaway clone of
`template-axum`'s `main`, keeping the paths classified below and renaming them
into the crate layout. That history is merged into this repository with
`--allow-unrelated-histories`, so `git log` and `git blame` follow the code back
to its origin.

**Classification of the paths.**

| Path in `template-axum` | Class | Path here |
| --- | --- | --- |
| `src/health/mod.rs`, `src/health/README.md` | moved, rewritten for the crate | `src/health/mod.rs`, `src/health/README.md` |
| `src/health/checks.rs` | moved; one feature per client | `src/health/checks.rs` |
| `src/generic/controllers/health.rs` | moved, renamed | `src/health/router.rs` |
| `HasHealthRegistry` (in `src/generic/controllers/mod.rs`) | moved with the router | `src/health/router.rs` |
| `src/telemetry.rs` | moved, renamed | `src/logs.rs`, `src/logs/otlp.rs` |
| `tests/postgres_health_check.rs`, `tests/s3_health_check.rs` | moved, rewritten for the crate's own services | same names |
| `tests/telemetry_configure_logging.rs` | moved, renamed | `tests/logs_configure_logging.rs` |
| health half of `tests/redis_rate_limiter.rs` | split out, rewritten | `tests/redis_health_check.rs` |
| `OidcHealthCheck` (in `checks.rs`) | generalized | `HttpHealthCheck` |
| `src/config.rs`, `src/oidc/`, `src/rate_limit.rs`, `src/problem_details.rs`, `src/http_headers.rs`, `src/migration/`, `src/hero/`, `src/lib.rs`, `src/main.rs` | stays in `template-axum` (or moves to `lib-crud-rs`, see its ADR 0002) | none |
| `src/generic/controllers/{audit,mock}.rs`, `HasSettings`, `HasRateLimiter` | stays: they need `oidc`, `config`, `rate_limit` | none |
| `tests/common/`, `tests/keycloak_oidc.rs`, `tests/dev_mode_wiring.rs`, `tests/e2e.rs`, `tests/main_failure_modes.rs`, `tests/check_layering.rs`, `tests/perf/` | stays: they build the application's state | none |
| `docs/frs/FR-0010..0012`, `docs/nfrs/NFR-0008`, `NFR-0009` | stays: they are the application's requirements; the crate's `README.md`s state the same behavior | none |
| `request_id`, `otel`, `health::info` | new, later items (AX7, AX8, AX9a) | not in this extraction |

The crate has no application-specific error to convert from. It defines one
`Error` enum (`#[non_exhaustive]`) with a single variant today,
`SubscriberAlreadyInstalled`, and `logs::configure_logging` returns
`Result<(), Error>` through `try_init` instead of panicking, so a library never
aborts its host.

`DatabaseHealthCheck` takes an `sqlx::PgPool` and sits behind the `postgres`
feature. A SeaORM host passes `connection.get_postgres_connection_pool().clone()`.
The other checks are the `redis`, `s3` and `http` features; `MockHealthCheck` is
core. The OTLP log export is the `otel` feature. All are off by default.

`HasHealthRegistry` is defined next to the router. `HealthRegistry::register`
still takes a `Box<dyn HealthCheck>` so a host's registration code does not
change.

The two process-wide effects the original tests mutated through
`std::env::set_var` (an `unsafe` call under edition 2024) are not allowed: the
crate sets `unsafe_code = "forbid"`. The OTLP layer is built by a function that
takes the "endpoint configured" decision and the exporter constructor as
parameters, so unit tests drive every branch without touching the environment.

Hero-specific words, FR/NFR/ADR numbers of `template-axum` and `Mode::Mock` in
comments and test data are replaced with neutral text.

## Consequences

- A dependent gets health checks and logging without Hero, SeaORM, OIDC or
  Redis, and compiles only the clients it enables.
- `template-axum` wires the crate in a later item (AX9): its `OidcHealthCheck`
  call becomes `HttpHealthCheck::new("oidc", format!("{issuer}/.well-known/openid-configuration"))`,
  its SeaORM connection is handed over as a pool, its `HasHealthRegistry`
  implementation moves to the crate's trait, and its `main` handles the
  `Result` from `configure_logging`.
- `HealthCheck::name` still returns `&str`, so a check can own its name
  (`HttpHealthCheck`) or return a literal.
- The crate's tests need Postgres, Valkey and an S3-compatible store, so the
  devcontainer starts all three and CI needs the same services.
- The 100% line-coverage gate counts the feature-gated code, so it runs with
  `--all-features`; code behind a feature that no test enables would be
  invisible to it.
- Linking the `llvm-cov` test binaries with every client library enabled needs
  more memory than a four-job build has in a small container; run that step
  with one job.
