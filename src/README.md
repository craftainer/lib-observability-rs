# src/

The `craftainer-observability` library crate (imported as
`craftainer_observability`): the observability half of an Axum service. It
knows nothing about an application's configuration, error type, state or
authentication; a host passes plain values (a pool, a client, a URL) and
implements one accessor trait.

- `health/` — `HealthCheck`, `HealthRegistry`, the ready-made checks and the
  `/live` and `/ready` router (`health/README.md`).
- `logs.rs` — `configure_logging`, JSON lines on stdout; with the `otel`
  feature (`logs/otlp.rs`) also OTLP/HTTP log export when the standard
  `OTEL_EXPORTER_OTLP_*` endpoint variables are set.
- `error.rs` — `Error`, the crate's own error type.
- `request_id.rs` — `RequestIdLayer`, a tower layer that validates or creates
  the `X-Request-ID` header, echoes it, stores a `RequestId` in the request's
  extensions and puts it in a `request` span as `request_id`. No feature: it
  needs no backing-service client (ADR 0004).

OpenTelemetry traces and metrics, and `/health/info` are planned
as their own modules (see `docs/adrs/0002-extract-the-observability-layer.md`).

## Do

- Keep every public item documented: `missing_docs` is a warning and clippy
  runs with `-D warnings`.
- Gate each backing-service client behind its own Cargo feature, off by
  default.
- Keep the crate free of an application's names, settings and error types.

## Don't

- Depend on an ORM, an auth library or an application's state type.
- Mutate process-wide state (environment variables, the global subscriber)
  from a unit test: the crate forbids `unsafe`, and a global subscriber can
  be installed once per process. Pass the value in as a parameter, or test
  from a file under `tests/`, which is its own process.
