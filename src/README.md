# src/

The `craftainer-observability` library crate (imported as
`craftainer_observability`): the observability half of an Axum service. It
knows nothing about an application's configuration, error type, state or
authentication; a host passes plain values (a pool, a client, a URL) and
implements one accessor trait.

- `health/` — `HealthCheck`, `HealthRegistry`, the ready-made checks and the
  `/live` and `/ready` router and the opt-in `info_router` (`health/README.md`).
- `logs.rs` — `configure_telemetry`, the one call that installs the
  `tracing` subscriber: JSON lines on stdout (`logs/json.rs`, the flat
  `lib-observability-py` layout with `trace_id`/`span_id`/`request_id` and
  credential redaction) and, with the `otel` feature, OTLP/HTTP export of
  logs (`logs/otlp.rs`), traces and metrics when the standard
  `OTEL_EXPORTER_OTLP_*` endpoint variables are set. Returns a `Telemetry`
  that flushes every exporter. `configure_logging` is the deprecated old name.
- `otel/` — feature `otel`: `resource.rs` (which signals the environment asks
  for, the shared `Resource`), `providers.rs` (tracer and meter providers, the
  trace layer, `Providers` for flush and shutdown), `server.rs`
  (`ServerSpanLayer`), `db.rs` (`instrument_db`, with `postgres`). ADR 0005.
- `error.rs` — `Error`, the crate's own error type.
- `request_id.rs` — `RequestIdLayer`, a tower layer that validates or creates
  the `X-Request-ID` header, echoes it, stores a `RequestId` in the request's
  extensions and puts it in a `request` span as `request_id`. No feature: it
  needs no backing-service client (ADR 0004).

`health/info.rs` holds the opt-in `/health/info` router (ADR 0006).

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
  be installed once per process. Test the OpenTelemetry side with the SDK's
  in-memory exporters and a scoped subscriber (`tracing::subscriber::set_default`). Pass the value in as a parameter, or test
  from a file under `tests/`, which is its own process.
