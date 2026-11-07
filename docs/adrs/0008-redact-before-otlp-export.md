# 0008. Redact credentials in front of the OTLP exporters

## Status

Accepted

## Context

ADR 0005 accepted that credential redaction lived only in the stdout JSON
layer: `OpenTelemetryTracingBridge` and `tracing-opentelemetry` received the
raw message and fields, so `tracing::info!(password = %p)` printed
`[REDACTED]` on stdout and arrived in the collector in clear, and
`#[instrument]` arguments became span attributes. Whoever can read the
telemetry backend is usually a wider group than whoever can read the pods.
The OWASP review (OW-lib-observability-rs, 2026-11-04) rated it Medium and the
user decided on one shared key list in front of both exporters, here and in
`lib-observability-py`.

The SDK's `SdkLogRecord` can only add attributes and set the body; it has no
replace or remove, so a `LogProcessor` cannot redact what the appender already
attached.

## Decision

- `src/redact_keys.rs` holds the one key list and `redact_text` (the health
  checks' `key=value` and URL-userinfo scrubber).
- `logs::otlp::RedactingBridge` replaces `opentelemetry-appender-tracing`'s
  bridge (the dependency is dropped). It builds each log record through the
  public Logger API, redacting the message, the string fields and the whole
  `source()` chain of error fields, replacing the value of any secret-named
  field, and sets the trace context from the event's span like `JsonLayer`.
- `otel::redact::RedactingSpanExporter` wraps the span exporter and redacts
  the span name, attributes, events and error status.

## Consequences

The crate owns about 100 lines of bridge code that must follow the SDK's log
record API. Redaction by key name and `key=value` shape over-redacts a field
such as `session_id` on purpose; a secret in free text of another shape is not
caught. Array-valued span attributes are not scrubbed. Metrics carry no
free-form fields and are not touched.
