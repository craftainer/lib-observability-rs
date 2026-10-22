# 0003. Harden the crate to match lib-observability-py before its first release

## Status

Accepted

## Context

AX2 extracted the crate; its review (2026-10-22) found gaps before any
crates.io release. The rule for fixing them: match `lib-observability-py`
(`craftainer-observability` 0.2.0), defaults included, and where Python is
demonstrably wrong, take the documented best practice, record the deviation
and queue a Python follow-up. The crate is unreleased, so public API may
change.

## Decision

Matched with Python:

- Default check timeout of 5 seconds (Python's `HTTPCheck` default).
- The real error goes to the logs, `/ready` shows a fixed string
  (`dependency check failed; see server logs`).
- `OTEL_SDK_DISABLED=true`, `OTEL_LOGS_EXPORTER=none` and an empty endpoint
  switch the OTLP log export off.
- An exporter that cannot be built is an error to the caller, not a silent
  no-op (Python raises).
- Checks report under a name and the registry keeps registration order.

Deviations from Python (candidates for a Python follow-up):

- **Timeouts for every check, enforced by the registry.** Python has a timeout
  only in `HTTPCheck`; a hung Postgres, Redis or S3 stalls `/ready`. Here the
  registry wraps every check in a timeout (default 5 s, `HealthRegistry::
  with_default_timeout`, per check via `HealthCheck::timeout` and the
  `with_timeout` builders) and reports a timeout as unhealthy.
- **Duplicate names are rejected** (`register` returns
  `Error::DuplicateCheckName`). Python appends them, and `/ready` then
  collapses the results into one dict key, hiding a failing check.
- **Credentials are redacted in logged errors.** Python's
  `logger.exception` logs the raw exception, which can embed a DSN.
- **Shutdown handle.** Python's SDK flushes at `atexit`; Rust has no such hook,
  so `configure_logging` returns a `LoggingGuard` with `force_flush` and
  `shutdown` (also run on drop).
- **A subscriber check precedes building the provider.** Python adds a second
  set of handlers when called twice; Rust returns
  `Error::SubscriberAlreadyInstalled` before it builds anything.
- **One Redis connection is reused** (`ConnectionManager`); Python opens and
  closes a client per check.
- **`MockHealthCheck` is behind the `testing` feature**; Python ships its
  equivalents in the main package.
- **Logs-specific endpoint.** `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` also enables
  export (Python reads only the general endpoint).

Found while testing: the OTLP exporter used the async `reqwest` client, which
panics on the batch processor's plain thread ("there is no reactor running"),
so nothing was ever exported, and no `reqwest` TLS feature meant https
endpoints could not work. The crate now uses `reqwest-blocking-client` and
`reqwest-rustls`.

Public API changes:

- `HealthRegistry::register` returns `Result<(), Error>`.
- New: `HealthRegistry::with_default_timeout`, `default_timeout`,
  `DEFAULT_CHECK_TIMEOUT`, `HealthCheck::timeout` (defaulted), `with_timeout`
  on each ready-made check.
- `logs::configure_logging` returns `Result<LoggingGuard, Error>`; new
  `LoggingGuard`, `configure_logging_without_export`.
- New `Error` variants `DuplicateCheckName`, `ExporterBuild`, `Flush`.
- `MockHealthCheck` needs the `testing` feature.
- The exporter-target filter matches whole crate names (no `tonic`, which is
  not a dependency).
- `sqlx` has no TLS backend; the README documents the features a host adds.

## Consequences

A host must keep the `LoggingGuard` alive and handle `register`'s `Result`.
Timeouts cancel the check's future, which may leave a half-open connection to
the pool or client's own timeouts. The Python library has a follow-up list
(timeouts for non-HTTP checks, duplicate names, redaction).
