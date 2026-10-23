# lib-observability-rs

> [!NOTE]
> This repository is vibe coded with [Claude](https://claude.com/product/claude-code).

`craftainer-observability` (imported as `craftainer_observability`): health
checks, structured JSON logging and OTLP log export for
[Axum](https://github.com/tokio-rs/axum) services, extracted from
[`template-axum`](https://github.com/craftainer/template-axum). It is an
instance of
[`template-rust-crate`](https://github.com/craftainer/template-rust-crate)
(so it gets the devcontainer, CI, release workflow and template sync from
there); see [`docs/TEMPLATE.md`](docs/TEMPLATE.md) for the conventions shared
by every instance and
[`docs/adrs/0002-extract-the-observability-layer.md`](docs/adrs/0002-extract-the-observability-layer.md)
for why the crate exists and where its boundary is.

- `health::HealthCheck` and `HealthRegistry`: a trait for one dependency's
  check and a registry that runs them all concurrently, each under a timeout
  (5 seconds by default, as in `lib-observability-py`; set it per registry
  with `with_default_timeout` and per check with `HealthCheck::timeout` or
  the ready-made checks' `with_timeout`). A timed-out check reports
  unhealthy. `register` rejects a duplicate name.
- `health::checks`: ready-made checks for Postgres, Redis or Valkey (one
  reused connection), S3 and any HTTP endpoint, plus, behind the `testing`
  feature, an always-healthy mock. Their logged errors have credentials
  redacted.
- `health::router::router`: the `/live` and `/ready` routes, over any state
  that implements `HasHealthRegistry`.
- `logs::configure_logging`: JSON lines on stdout, and with the `otel`
  feature OTLP/HTTP export of the same logs. It returns a `LoggingGuard`:
  keep it until exit and call `shutdown()` (or let it drop) so the last batch
  is flushed. An exporter that cannot be built is an `Error::ExporterBuild`
  (`configure_logging_without_export` is the JSON-only fallback).
- `Error`, the crate's own error type.

```rust,ignore
let mut registry = HealthRegistry::new();
registry.register(Box::new(DatabaseHealthCheck::new(pool.clone())))?;
registry.register(Box::new(RedisHealthCheck::new("redis://redis:6379/0")))?;
let _logging = configure_logging()?; // keep it alive until exit
// ... implement HasHealthRegistry on the app state, then:
let app = Router::new().nest("/health", router()).with_state(state);
```

## Features

All off by default, so a service compiles only the clients it has.

- `postgres`: `DatabaseHealthCheck` over an `sqlx::PgPool` (see "sqlx TLS").
- `redis`: `RedisHealthCheck`.
- `s3`: `S3HealthCheck` over an `aws_sdk_s3::Client`.
- `http`: `HttpHealthCheck`.
- `otel`: OTLP/HTTP log export from `configure_logging`, enabled at run time
  by a non-empty `OTEL_EXPORTER_OTLP_ENDPOINT` or
  `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`, and switched off by
  `OTEL_SDK_DISABLED=true` or `OTEL_LOGS_EXPORTER=none`.
- `testing`: `MockHealthCheck`. Enable it from a dev-dependency or a
  test/mock mode, not in production builds.

### sqlx TLS

The crate depends on `sqlx` with `default-features = false` and no TLS
backend, so it adds none to your build. A Postgres server that requires TLS
(most managed ones) fails to connect unless your own `sqlx` dependency enables
one, for example `features = ["tls-rustls"]` (or `tls-native-tls`,
`tls-rustls-aws-lc-rs`, ...). Cargo unifies features, so the pool you pass to
`DatabaseHealthCheck::new` and this crate share the backend you chose.

## Development

Open the repository in the devcontainer: it starts Postgres, Valkey and
RustFS, which the integration tests need (`tests/README.md`). Then run
`prek run --all-files --hook-stage manual`; that runs `rustfmt`,
`clippy -D warnings`, the tests with a 100% line-coverage gate, `cargo-deny`
and the template-sync manifest check.

## Publishing to crates.io

`make publish-packages` is a no-op (with a `::warning::` in the log) until you opt in, per repository, by setting the
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
2. Set `CARGO_PUBLISH=1` and run the release workflow. `make publish-packages` sees
   the token and publishes with it.
3. On crates.io, open the crate's Settings, Trusted Publishing, and add a
   GitHub publisher: the owner, the repository, the workflow `release.yml`,
   and no environment.
4. Delete the `CARGO_REGISTRY_TOKEN` secret and revoke the token. From the
   next release, `make publish-packages` finds no token and exchanges the job's
   GitHub OIDC identity for a 30-minute crates.io token, publishes with it
   and revokes it.

## License

Licensed under the [GNU General Public License v3.0](LICENSE).
