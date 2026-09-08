# health/

The health-check trait and registry backing `controllers::health`'s
`/health/ready` (`/health/live` stays dependency-free and doesn't use
this module at all).

- `mod.rs` — `HealthCheck` (an `async-trait` so `HealthRegistry` can
  hold a heterogeneous `Vec<Box<dyn HealthCheck>>`), `HealthCheckResult`,
  and `HealthRegistry::run_all` (runs every check concurrently via
  `futures::future::join_all`).
- `checks.rs` — the concrete checks: `DatabaseHealthCheck` (`SELECT 1`),
  `RedisHealthCheck` (`PING`), `S3HealthCheck` (`ListBuckets`),
  `OidcHealthCheck` (GET discovery document), and `MockHealthCheck`
  (always healthy, used under `Mode::Mock`).

Each check catches its own dependency's error type internally and
reports a fixed, non-leaking `detail` string on failure (the real error
goes to `tracing::error!` only) — `docs/nfrs/0008-health-check-
isolation.md`. Registering a new dependency's check is `impl
HealthCheck for X { ... }` plus one `registry.register(Box::new(X::
new(...)))` call in `main.rs::build_health_registry` — no other wiring.
