# 0007. Add a vendor-neutral `analytics` feature

## Status

Accepted

## Context

The BFF/analytics plan (2026-10) gives every backend of the template family one
`track()` contract (I3), so product events reach Umami (or any other sink)
without the application naming a vendor. Rust services (template-axum) need it
here, in the crate that already owns logging and OpenTelemetry. The Python
version is [ADR 0003 of lib-observability-py](https://github.com/craftainer/lib-observability-py/blob/main/docs/adrs/0003-add-vendor-neutral-analytics-extra.md);
this ADR mirrors its behaviour. Constraints:

- Analytics is cookieless and carries no personal data by default, honours the
  Global Privacy Control (GPC) and Do-Not-Track signals, and offers an opt-in
  mode.
- A request must never fail or wait because analytics is down.
- Umami v3.4.1's `/api/batch` silently drops events whose `User-Agent` looks
  like a bot, and answers HTTP 200 with an `errors` entry even when events were
  rejected (source: `src/app/api/batch/route.ts`, `src/lib/detect.ts`,
  <https://github.com/umami-software/umami/tree/v3.4.1>).
- `tokio` is a dependency here with only `sync` and `time`; a library cannot
  assume it is called inside a runtime, and `track()` is called from handlers,
  from `spawn_blocking`, and from plain threads alike.
- `tracing` field names are static, so a `tracing` event cannot carry one field
  per allowlisted property key. OpenTelemetry log attributes must be primitives
  (<https://opentelemetry.io/docs/specs/otel/common/#attribute>), never a map.
  The logger provider that exports logs is owned by the `Telemetry` handle that
  `configure_telemetry` returns; the OpenTelemetry API has no global one.

## Decision

- Add an `analytics` feature (`reqwest/blocking`) and an `analytics` module.
  `track(name, props, url)` follows I3 exactly, including the `ANALYTICS_*`
  environment variables (`AnalyticsConfig::from_env`, with a `from_lookup` core
  so tests need no process environment). Invalid names drop the event and
  properties off the `ANALYTICS_PROP_KEYS` allowlist drop only themselves;
  strings are cut to 200 characters; `url` is reduced to a path (default `/`,
  at most 500 characters). Logs never carry a value.
- **The Umami flusher is a plain `std::thread` with `reqwest::blocking`.** The
  alternative, a tokio task, needs a runtime handle: `track()` would have to
  find one (`Handle::try_current` fails on a plain thread), the crate would need
  tokio's `rt` feature, and a runtime that is dropped or never entered silently
  loses events. The thread needs none of that, starts lazily on the first
  event, and the queue is a `Mutex<VecDeque>` capped at 1000 with a `Condvar`
  (so `track()` is a short lock, never an await). The trade-offs: one thread per
  client; `shutdown()` blocks until the final flush is done (call it through
  `spawn_blocking` in async code); and the blocking client runs its own
  runtime internally, which is why it is built and dropped on the flusher
  thread only. The crate's OTLP exporter makes the same blocking-client choice
  for the same reason.
- Umami contract: `POST {ENDPOINT}/api/batch`, at most 50 items per request,
  queue of 1000 (a full queue drops the new event), a flush every 5 s, a 2 s
  timeout, `User-Agent: Mozilla/5.0 (compatible; craftainer-rs)`, the constant
  `payload.ip = "server"`, no `payload.id`, no title. A non-200 status or an
  `errors` entry is logged as a warning with status and counts only. Redirects
  are not followed: `ANALYTICS_ENDPOINT` must be the final URL, and a 3xx batch
  is dropped with a warning naming the status and `Location`. The first
  transport failure (connect, timeout) stops the flush and drops the rest.
  After `shutdown()` events are dropped.
- Adapters: `none`; `log` (one `tracing` event on the target
  `craftainer_observability::analytics::events` with the fixed fields
  `analytics_event`, `analytics_url` and `analytics_props`, the allowlisted
  properties as one JSON string, because `tracing` cannot name dynamic fields;
  the alternative, building callsites at run time, relies on `tracing-core`
  internals marked `#[doc(hidden)]`); `otel` (an OpenTelemetry log record with
  the primitive attributes `analytics_event`, `analytics_url` and one
  `analytics_prop_<key>` per property, emitted through the logs API on the
  provider of the `Telemetry` handle, so it shares one resource and exporter
  with the rest of the process). `AnalyticsClient::with_telemetry` and
  `analytics::init(config, Some(&telemetry))` take that handle; without it, the
  `otel` feature, or an exported log signal, the `otel` backend warns and is
  off. `Telemetry::logger_provider` is `pub(crate)`.
- Request privacy is a `RequestContext { gpc, consent }`. It is a thread-local
  set around every `poll` of a wrapped future (`RequestContext::scope`) or for
  the lifetime of a guard (`enter`), not a tokio task-local: it works on any
  executor and adds no runtime dependency. It does not follow into spawned
  tasks or threads, which must be wrapped themselves. With
  `ANALYTICS_RESPECT_GPC` true (default) `track()` is a no-op under `gpc`; with
  `ANALYTICS_CONSENT=opt-in` it is a no-op unless `consent` is set (and when no
  context is active).
- `ANALYTICS_GEO` is parsed for contract parity and has no effect: backend
  events carry `ip=server`, so Umami derives no location.
- Fork safety (AN1's `register_at_fork` reset) does not apply: Rust services
  do not `fork()` a threaded process, and the flusher is created per client on
  first use.
- Rust has no `atexit`: the host calls `analytics::shutdown()` (or drops its
  client) before exit, like `Telemetry::shutdown`.

## Consequences

- Services get one call that is safe in request handlers and from sync and
  async code; a vendor swap is an environment change.
- Delivery is best effort: events are lost on overflow, a crash, a missing
  `shutdown()`, or an unreachable Umami. That is the intended trade for never
  affecting a request.
- Privacy depends on the middleware wrapping each request in a
  `RequestContext`; outside one `track()` cannot see GPC. Visitor-level
  identity is deliberately absent from backend events, so unique-visitor counts
  come from the BFF, which sees the client.
- The `log` adapter's properties are one JSON string, not separate fields; the
  `otel` adapter's are separate attributes. Both carry only allowlisted keys.
- Each client with the `umami` backend owns a thread that sleeps between
  flushes; the process-wide client is one.
- Builds without the feature gain no dependency; `reqwest` gains `blocking`
  only where `analytics` is on.
