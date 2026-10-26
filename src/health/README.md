# health/

Health checks and the `/live` and `/ready` router.

- `mod.rs` — `HealthCheck` (an `async-trait`, so `HealthRegistry` can hold a
  heterogeneous `Vec<Box<dyn HealthCheck>>`), `HealthCheckResult` and
  `HealthRegistry::run_all`, which runs every check concurrently with
  `futures::future::join_all`.
- `checks.rs` — the ready-made checks: `DatabaseHealthCheck` (`SELECT 1` over
  an `sqlx::PgPool`, feature `postgres`), `RedisHealthCheck` (`PING`, feature
  `redis`), `S3HealthCheck` (`ListBuckets`, feature `s3`), `HttpHealthCheck`
  (GET a URL, expect 2xx, feature `http`) and `MockHealthCheck` (always
  healthy, no feature).
- `router.rs` — `router()`, the `/live` and `/ready` routes, generic over any
  state implementing `HasHealthRegistry`.
- `info.rs` — `info_router()`, the opt-in `GET /info` route (`{"version",
  "commit"}` from `APP_VERSION`/`APP_COMMIT`, read per request, empty or unset
  is `null`); the host merges it next to `router()`. ADR 0006.

`/live` never touches the registry, so a downstream outage cannot fail
liveness. Each failing check catches its dependency's error and reports a
fixed, non-leaking `detail`; the real error goes to `tracing::error!` only.
Registering another dependency is `impl HealthCheck for X` plus one
`registry.register(Box::new(X::new(..)))` call in the host.
