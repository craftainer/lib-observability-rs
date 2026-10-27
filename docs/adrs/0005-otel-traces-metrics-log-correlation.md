# 0005. OpenTelemetry traces, metrics and log correlation at parity with lib-observability-py

## Status

Accepted

2026-10-26: the level-names deviation below no longer holds. The JSON log `level`
field reports `WARN` as `WARNING` (as Python's `logging` does); `TRACE`, `DEBUG`,
`INFO` and `ERROR` are unchanged (LV1, released in 0.3.0).

## Context

AX8 adds what `lib-observability-py` has in `otel.py`, `logs.py` and its
FastAPI instrumentation and the crate lacked: traces, metrics, the server span,
trace and request IDs in log lines, credential redaction in log extras, and
connection-pool gauges. The rule from ADR 0003 holds: match Python, defaults
included, and record each deviation.

The crate installs one global `tracing` subscriber (`configure_logging`), so a
trace layer can only be added where that subscriber is built. Python has two
independent calls (`configure_logging`, `configure_telemetry`) because its
logging handlers and OTel providers are separate globals.

Verified before choosing dependencies (2026-10-26):

- **`tracing-opentelemetry` 0.33.0** is the release that pairs with
  `opentelemetry` 0.32: its only OpenTelemetry dependency is
  `opentelemetry ^0.32.0`, and its dev-dependencies are the 0.32 SDK, OTLP and
  stdout crates (<https://crates.io/api/v1/crates/tracing-opentelemetry/0.33.0/dependencies>,
  published 2026-05-18). 0.34.0 needs `opentelemetry ^0.33.0`, so it does not
  pair. Pinned `=0.33.0`, with default features off.
- **The SDK reads `OTEL_TRACES_SAMPLER` and `OTEL_TRACES_SAMPLER_ARG`**
  itself: `opentelemetry_sdk` 0.32.1, `src/trace/config.rs` (`Config::default`,
  used by `SdkTracerProvider::builder()`), documented in the crate's `lib.rs`
  environment-variable table. Values: `always_on`, `always_off`,
  `traceidratio`, `parentbased_always_on` (default), `parentbased_always_off`,
  `parentbased_traceidratio`; an invalid ratio falls back to 1.0, and
  `jaeger_remote`/`xray` fall back to `parentbased_always_on`. The same
  crate's `PeriodicReader` reads `OTEL_METRIC_EXPORT_INTERVAL`, and
  `Resource::builder()` reads `OTEL_SERVICE_NAME` and
  `OTEL_RESOURCE_ATTRIBUTES`.
- **The SDK does not read `OTEL_TRACES_EXPORTER` / `OTEL_METRICS_EXPORTER`**
  for hand-built providers (as in Python), so the crate handles `none`.
- `opentelemetry::global::tracer()` is not late-bound: a tracer taken before
  `set_tracer_provider` stays a no-op
  (`opentelemetry-0.32.0/src/global/trace.rs`). A trace layer therefore cannot
  be installed first and pointed at a provider that is configured later.
- Inside `Layer::on_event`, `tracing::Span::current()` returns a disabled span
  (the dispatcher is hidden from code running inside it), so the JSON layer
  looks the event's span up through the registry and
  `tracing_opentelemetry::get_otel_context`.

## Decision

**One entry point installs everything.** `configure_telemetry() ->
Result<Telemetry, Error>` installs the subscriber: `EnvFilter`, the JSON
layer, with the `otel` feature the OTLP logs bridge, the `tracing-opentelemetry`
trace layer and the meter provider; it also sets the global tracer and meter
providers and a `traceparent` propagator, so `ServerSpanLayer::new()` and
`instrument_db` find them. `Telemetry` holds the three providers;
`Telemetry::shutdown`/`force_flush` flush all of them, and dropping it shuts
down. `configure_telemetry_without_export()` is the JSON-only fallback after
an `Error::ExporterBuild`. Without the `otel` feature, or with no endpoint
set, `configure_telemetry` still installs JSON logging and builds no exporter.

`configure_logging()`, `configure_logging_without_export()` stay as
`#[deprecated(since = "0.3.0")]` shims that delegate and return a
`LoggingGuard` wrapping a `Telemetry`. Deprecation is not a semver break, but
see "Behaviour changes" below.

**Matched with Python:**

- Nothing runs unless an OTLP endpoint is non-empty and `OTEL_SDK_DISABLED` is
  not `true`; `OTEL_{TRACES,METRICS,LOGS}_EXPORTER` containing `none` switches
  one signal off.
- One shared `Resource` for all three signals; `service.version` from
  `APP_VERSION` unless `OTEL_RESOURCE_ATTRIBUTES` sets it.
- Server spans and a request-duration histogram per request, `/health/*`
  excluded (Python's `/health/.*`), a valid `traceparent` makes the span a
  child, trace and span IDs in every log line inside a span, `request_id` in
  every line inside a request, credential redaction by key substring
  (`token`, `password`, `secret`, `authorization`, `claims`, `credential`, any
  case) with the value `[REDACTED]`.
- Pool usage under the name SQLAlchemy's instrumentation uses
  (`db.client.connections.usage`, `state` of `idle`/`used`).
- Log line keys: `timestamp` (RFC 3339 UTC with microseconds and `+00:00`,
  Python's `isoformat()`), `level`, `logger` (the event target), `message`,
  extras at the top level, then `trace_id`, `span_id`, `request_id`.

**Deviations from Python:**

- **Per-signal endpoints.** `OTEL_EXPORTER_OTLP_{TRACES,METRICS,LOGS}_ENDPOINT`
  also enable (or, when empty, disable) their signal; Python reads only the
  general endpoint. (The logs behaviour predates this ADR.)
- **Level names** are tracing's (`WARN`, not `WARNING`; no `CRITICAL`).
- **A `request` span is exported too.** `RequestIdLayer`'s span (ERROR level,
  field `request_id`) is an ordinary tracing span, so the trace layer exports
  it as an internal child of the server span, with `request_id` as an
  attribute. Python has no such span.
- **The `http.request.header.x-request-id` attribute** (ADR 0004's deferred
  item) is set by `ServerSpanLayer` with the validated ID in either layer
  order: from the response header when the layer is outside `RequestIdLayer`,
  from the request's `RequestId` extension when it is inside.
- **Outcome edge cases.** A 5xx response, an error from the inner service and a
  request dropped before it completes (a client that went away) all set the
  span status to error and `error.type` on the metric (the status code,
  `_OTHER`, `cancelled`). A method other than the nine standard ones is
  recorded as `_OTHER` (semconv), so a client cannot grow the metric's
  cardinality.
- **The exporter's own stack is kept out of the trace layer** by the same
  target filter as the logs bridge (hyper, h2, reqwest, rustls, the
  OpenTelemetry crates), so exporting a batch cannot create spans that go into
  the next one.
- **Spans are at ERROR level**, like the request-ID span: tracing disables a
  span below the active filter, and `RUST_LOG=warn` would silently stop all
  tracing otherwise.
- **Event fields named like the line's own keys are dropped** (`timestamp`,
  `level`, `logger`, `message` handled apart, `trace_id`, `span_id`,
  `request_id`): Python lets an `extra` overwrite them and then overwrites
  `trace_id`/`request_id` itself.
- **Only event fields are redacted**, as in Python (its `extra`); span fields
  are not part of the line.
- **Exported OTLP log records carry no trace context.** The `tracing` to
  OpenTelemetry log bridge reads the OpenTelemetry *current context*, which
  `tracing-opentelemetry` does not attach on span entry, so a collector cannot
  join exported logs to traces by ID (the stdout JSON lines carry the IDs).
  Python's `LoggingHandler` does attach them. Left as a follow-up.
- **Pool gauges are observable gauges** (`u64`) read at export time, with an
  added `db.client.connections.max`; no per-query spans (Python's SQLAlchemy
  instrumentation adds them; `sqlx` has no equivalent hook).
- **`instrument_db(pool: &sqlx::PgPool)`** exists only with both `otel` and
  `postgres` (the crate must not force sqlx on users).
- **A `Telemetry` handle instead of module globals**: `shutdown()` is a method,
  because Rust needs an explicit flush (no `atexit`).

**Behaviour changes for existing users (user-facing):**

- The JSON log line layout changes for every `configure_telemetry`/
  `configure_logging` user, with or without `otel`. Before, tracing-subscriber's
  JSON format: `{"timestamp":...,"level":...,"fields":{"message":...,...},
  "target":...,"span":{...},"spans":[...]}`. Now the flat Python layout above.
  A log pipeline reading `fields.message`, `target` or `span.request_id` must
  read `message`, `logger` and `request_id`. Span fields other than
  `request_id` are no longer printed.
- Credential-looking event fields are redacted in the stdout JSON line. The
  OTLP log bridge is a separate layer and still receives the original
  attribute values, so an exported log record is not redacted (Python's
  redaction is in its stdout formatter only as well).
- `configure_logging()` now also builds the tracer and meter providers when the
  environment asks for them (it is `configure_telemetry`). `Error::ExporterBuild`
  and `Error::Flush` messages are no longer log-specific.
- OpenTelemetry's own internal log events (target `opentelemetry*`) appear in
  the JSON lines as before.

Public API: `logs::{Telemetry, configure_telemetry,
configure_telemetry_without_export}` (re-exporting `configure_telemetry` and
`Telemetry` at the crate root), the deprecated `logs::{configure_logging,
configure_logging_without_export, LoggingGuard}`, and with the `otel` feature
`otel::{ServerSpanLayer, ServerSpanService, ResponseFuture,
DEFAULT_EXCLUDED_PREFIXES}` plus, with `postgres`, `otel::instrument_db`. New
dependencies: `tracing-opentelemetry` (optional), `serde_json` (the JSON line),
and the `trace`/`metrics` features of the OpenTelemetry crates already used;
`axum`'s `matched-path` feature.

## Consequences

A service calls `configure_telemetry()` once, keeps the `Telemetry`, adds
`.layer(RequestIdLayer::new()).layer(ServerSpanLayer::new())` to its router
(the later layer is the outer one), calls `instrument_db(&pool)` after
`configure_telemetry`, and `shutdown()`s on exit. The server span is only a
OpenTelemetry span when the trace layer is installed, that is, through
`configure_telemetry`; a host that installs its own subscriber gets plain
tracing spans and a working (but unexported) histogram.

Redaction is by field name only; a secret inside a message string, a
non-credential-named field or the `url.path` span attribute (a token in a URL
path) is not caught. `/health` without the trailing slash is not excluded, as
with Python's `/health/.*`; pass `with_excluded_prefixes` for other probes.
`Telemetry`'s `Drop` blocks like `shutdown()` does (as `LoggingGuard`'s always
did); an async `main` should call `shutdown` through `spawn_blocking` first. A custom event formatter replaces
`tracing-subscriber`'s JSON format, so a feature of that formatter (flattened
span lists, `RUST_LOG_FORMAT`) is not available.

Python follow-up: none required for this item; the exported-log trace context
gap above is on the Rust side.
