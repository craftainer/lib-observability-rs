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
- `health::info::info_router`: an opt-in `GET /info` returning
  `{"version","commit"}` from `APP_VERSION`/`APP_COMMIT` (null when unset or
  empty), merged next to `router()`; off unless the host merges it (ADR 0006).
- `configure_telemetry` (`logs::configure_telemetry`): installs the `tracing`
  subscriber, JSON lines on stdout in `lib-observability-py`'s flat layout
  (`timestamp`, `level`, `logger`, `message`, extra fields at the top level
  with credential-looking keys redacted (the shared key list, see "Redaction"), `trace_id`/`span_id` inside an
  OpenTelemetry span, `request_id` inside a request). With the `otel` feature
  and an OTLP endpoint in the environment it also exports logs, traces and
  metrics. It returns a `Telemetry`: keep it until exit and call `shutdown()`
  (or let it drop) so the last batch is flushed. An exporter that cannot be
  built is an `Error::ExporterBuild` (`configure_telemetry_without_export` is
  the JSON-only fallback). `configure_logging` is its deprecated old name. See
  [`docs/adrs/0005-otel-traces-metrics-log-correlation.md`](docs/adrs/0005-otel-traces-metrics-log-correlation.md).
- `otel::ServerSpanLayer` (feature `otel`): a tower layer that opens a server
  span per request (named by axum's `MatchedPath`, `/health/` excluded,
  `traceparent` honoured) and records `http.server.request.duration`;
  `otel::instrument_db(&PgPool)` (features `otel` and `postgres`) adds
  connection-pool gauges.
- `request_id::RequestIdLayer`: a tower layer (no feature needed) that reads
  `X-Request-ID` (name configurable), keeps it only if it is 1 to 128
  characters from `[A-Za-z0-9._-]` and otherwise uses a fresh UUID, echoes it
  on the response, stores a `RequestId` in the request's extensions
  (`Extension<RequestId>`) and records it as the `request_id` field of a
  `request` tracing span, so the JSON logs carry it. See
  [`docs/adrs/0004-request-ids.md`](docs/adrs/0004-request-ids.md).
- `analytics` (feature `analytics`): `analytics::track(name, props, url)`,
  vendor-neutral product analytics configured by `ANALYTICS_*` variables; see
  "Analytics" below.
- `Error`, the crate's own error type.

```rust,ignore
let mut registry = HealthRegistry::new();
registry.register(Box::new(DatabaseHealthCheck::new(pool.clone())))?;
registry.register(Box::new(RedisHealthCheck::new("redis://redis:6379/0")))?;
let telemetry = configure_telemetry()?; // keep it alive until exit
// ... implement HasHealthRegistry on the app state, then:
let app = Router::new().nest("/health", router()).with_state(state);
// opt in to /health/info: router().merge(info_router())
```

## Features

All off by default, so a service compiles only the clients it has.

- `postgres`: `DatabaseHealthCheck` over an `sqlx::PgPool` (see "sqlx TLS").
- `redis`: `RedisHealthCheck`.
- `s3`: `S3HealthCheck` over an `aws_sdk_s3::Client`.
- `http`: `HttpHealthCheck`.
- `otel`: OTLP/HTTP export of logs, traces and metrics from
  `configure_telemetry`, enabled at run time by a non-empty
  `OTEL_EXPORTER_OTLP_ENDPOINT` (or a signal's own
  `OTEL_EXPORTER_OTLP_{LOGS,TRACES,METRICS}_ENDPOINT`) and switched off by
  `OTEL_SDK_DISABLED=true` or `OTEL_{LOGS,TRACES,METRICS}_EXPORTER=none`. The
  SDK reads `OTEL_TRACES_SAMPLER(_ARG)`, `OTEL_METRIC_EXPORT_INTERVAL`,
  `OTEL_SERVICE_NAME` and `OTEL_RESOURCE_ATTRIBUTES`; `APP_VERSION` becomes
  `service.version`. Adds the `otel` module (`ServerSpanLayer`, and with
  `postgres` `instrument_db`).
- `analytics`: the `analytics` module (see "Analytics"). Its `otel` backend
  also needs `otel`.
- `testing`: `MockHealthCheck`. Enable it from a dev-dependency or a
  test/mock mode, not in production builds.

## Analytics

Feature `analytics`, [ADR 0007](docs/adrs/0007-add-vendor-neutral-analytics-feature.md).
`track(name, props, url)` sends a product event to the backend chosen by
`ANALYTICS_BACKEND`:

| Variable | Meaning |
| --- | --- |
| `ANALYTICS_ENABLED` | `1\|true\|yes\|on` or `0\|false\|no\|off`; default true |
| `ANALYTICS_BACKEND` | `none` (default), `log`, `umami`, `otel` |
| `ANALYTICS_ENDPOINT`, `ANALYTICS_SITE_ID` | Umami base URL (`/api/batch` is appended; no redirects are followed) and website ID; `umami` without both is off |
| `ANALYTICS_HOSTNAME` | sent as `payload.hostname` |
| `ANALYTICS_CONSENT` | `implied` (default) or `opt-in` |
| `ANALYTICS_RESPECT_GPC` | same values as `ANALYTICS_ENABLED`; default true |
| `ANALYTICS_GEO` | `none` (default) or `coarse`; no effect (events carry `ip=server`) |
| `ANALYTICS_PROP_KEYS` | comma-separated allowlist of property keys; empty by default |

An unset or empty variable takes its default; **an unknown `ANALYTICS_ENABLED`, `_RESPECT_GPC`, `_BACKEND`, `_CONSENT` or `_GEO` is a startup error** (`Error::InvalidConfig`, fail closed: `ANALYTICS_CONSENT=optin` must not quietly become `implied`). `AnalyticsConfig::from_env()` / `from_lookup()` return `Result<AnalyticsConfig, Error>`, so call `from_env()?` at startup. The lazy read inside `analytics::track` (no `init`) cannot return an error: it logs it at error level and turns analytics off. An Umami endpoint with embedded credentials is refused and a plain `http` one to a non-loopback host logs a warning. Names match
`^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$` (at most 50 characters) or the event is
dropped; properties are scalars on the allowlist (strings cut to 200
characters); `url` is reduced to its path.

```rust,ignore
use craftainer_observability::analytics::{self, AnalyticsConfig, RequestContext};

let telemetry = configure_telemetry()?;
analytics::init(&AnalyticsConfig::from_env()?, Some(&telemetry)); // optional
// In a request, wrap the handler's future so GPC / opt-in are honoured:
RequestContext::from_headers(request.headers())
    .scope(async { analytics::track("signup.done", &[("plan", "pro".into())], Some("/signup")) })
    .await;
// Sync code holds the guard instead: let _g = ctx.enter();
analytics::shutdown(); // before exit; blocking, use spawn_blocking in async code
```

`RequestContext::from_headers(&HeaderMap)` reads `Sec-GPC: 1` and `DNT: 1`
(either sets `gpc`) and `Analytics-Consent: 1` (sets `consent`). **Trust
`Analytics-Consent` only from the BFF**: any client can send it, so the BFF
must strip the client's header and set its own from the visitor's recorded
choice, and a backend reachable without the BFF must not call `from_headers`
on raw client requests when `ANALYTICS_CONSENT=opt-in`.

The context is a thread-local set around every poll of the wrapped future: it
does not follow into tasks or threads the handler spawns unless they are
wrapped as well. The Umami backend queues up to 1000 events, posts at most 50
per request every 5 s from its own thread with a 2 s timeout, and drops on
overflow. The `log` backend writes a `tracing` event (target
`craftainer_observability::analytics::events`, fields `analytics_event`,
`analytics_url`, `analytics_props`); the `otel` backend (features `analytics`
and `otel`, a `Telemetry` passed to `init`) emits a log record with the flat
attributes `analytics_event`, `analytics_url` and `analytics_prop_<key>` on the
same logger provider as the rest of the telemetry.

## Redaction

One key list (`src/redact_keys.rs`) decides which values are never exported.
A field, span attribute or event attribute whose name contains one of these
(any case) has its value replaced by `[REDACTED]`: `token`, `password`,
`passwd`, `pwd`, `secret`, `authorization`, `claims`, `credential`, `api_key`,
`apikey`, `access_key`, `cookie`, `bearer`, `jwt`, `session`, `dsn`,
`private`, `signature` (`-` counts as `_`, so `X-Api-Key` matches). It applies to the stdout JSON lines, the OTLP log
export and the OTLP span export (ADR 0008). On the OTLP side the message,
string values, span and event names, a span's error status and an error's whole
`source()` chain are also scrubbed for `key=value` pairs and `scheme://user:pw@`
userinfo, an `Authorization`-style `Bearer`/`Basic` scheme before a value,
`Proxy-Authorization` with any scheme and `Digest` parameters, `key=value`
pairs inside escaped JSON, a bare `Bearer x` and a bare JWT, as the health checks' logged errors are.
`token` is matched as a substring on purpose, so counters like `input_tokens`
are redacted too (no allowlist). On spans, `http.url`, `url.full`, `http.target` and `url.path` lose their
query and fragment and `url.query` is dropped. A credential in free text of
another shape is not caught: keep secrets out of messages.

## Health endpoints

`/ready` is unauthenticated and probes every dependency (a query, a `PING`, a
billable `ListBuckets`). **Mount it on an internal port**, a second listener
the orchestrator's probes reach and the internet does not, and keep `/live`
the only public health route if one is needed. As a second line of defence
`HealthRegistry::run_all_cached` (what `/ready` serves) reuses a finished run
for 1 second (`with_ready_cache_ttl`) and lets concurrent requests wait for
the run in flight, so a client looping on `/ready` costs each dependency at
most one probe per second. A zero TTL turns the cache and the waiting off:
every request probes. The run is driven by the request that started it (the
crate has no runtime handle to detach it), so if that request is dropped, for
instance by a probe timeout shorter than a slow check, the next waiter starts
a fresh run.

## Upgrading

Breaking for a minor release (0.x):

- `AnalyticsConfig::from_env()` and `from_lookup()` return
  `Result<AnalyticsConfig, Error>`; an unknown `ANALYTICS_ENABLED`,
  `_RESPECT_GPC`, `_BACKEND`, `_CONSENT` or `_GEO` is `Error::InvalidConfig`
  (new variant of the non-exhaustive `Error`).
- `Error::InvalidConfig`, `RequestContext::from_headers` (a repeated
  `Analytics-Consent` header counts only when every copy is `1`),
  `HealthRegistry::{with_ready_cache_ttl, run_all_cached}` are new.
- The `opentelemetry-appender-tracing` dependency is gone: the OTLP log layer
  is the crate's own (`/ready` and redaction behave as above).

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
   and the environment `release`. The environment field is the binding
   control: it limits trusted publishing to jobs that declare
   `environment: release`, which template-base's `release.yml` does for its
   privileged jobs (with a deployment-branch rule allowing only the default
   branch).
4. Delete the `CARGO_REGISTRY_TOKEN` secret and revoke the token. A present
   token is preferred over OIDC (`make publish-packages` publishes with it and
   never tries trusted publishing), so the token must be gone for OIDC to
   engage; if it is an organization secret, restrict its repository access so
   this repository no longer sees it. From the next release,
   `make publish-packages` finds no token and exchanges the job's GitHub OIDC
   identity for a 30-minute crates.io token, publishes with it and revokes
   it.

## License

Licensed under the [GNU General Public License v3.0](LICENSE).
