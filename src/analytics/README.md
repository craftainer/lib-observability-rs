# analytics/

`track(name, props, url)`: vendor-neutral product analytics ([ADR 0007](../../docs/adrs/0007-add-vendor-neutral-analytics-feature.md)).
Feature `analytics`. Never fails, panics or waits for the network.

- `validate.rs` — event name (`^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$`, at most 50 chars), allowlisted scalar props (`PropValue`, values at most 200 chars), path-only `url`.
- `config.rs` — `AnalyticsConfig::from_env()` / `from_lookup()`: `ANALYTICS_ENABLED`, `_BACKEND` (`none|log|umami|otel`), `_ENDPOINT`, `_SITE_ID`, `_HOSTNAME`, `_CONSENT` (`implied|opt-in`), `_RESPECT_GPC`, `_GEO`, `_PROP_KEYS`.
- `context.rs` — `RequestContext { gpc, consent }` with `scope(future)` and `enter()`; a thread-local, no web-framework import.
- `emit.rs` — the `log` adapter (a `tracing` event) and the `otel` adapter (a log record with flat primitive `analytics_prop_<key>` attributes).
- `umami.rs` — `POST {ENDPOINT}/api/batch`, at most 50 per request, 2 s timeout, no redirects; the bounded queue (1000, flushed every 5 s) and its flusher thread.
- `client.rs` — `AnalyticsClient`; `mod.rs` — the process-wide `init` / `track` / `shutdown`.

## Do

- Set `ANALYTICS_ENDPOINT` to the final URL: a 3xx batch is dropped with a warning naming the `Location`.
- Wrap each request in `RequestContext::scope` (or hold `enter()`); wrap spawned tasks too, the context does not follow them.
- Call `analytics::shutdown()` before exit (through `spawn_blocking` in async code).
- Add a prop key to `ANALYTICS_PROP_KEYS` before sending it.

## Don't

- Import a web framework here, or log the value of a dropped or invalid prop.
- Create or drop the blocking HTTP client outside the flusher thread.
