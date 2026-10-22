# tests/

The integration tier. Each file links against the crate's public API only, as
a dependent would, and runs against the devcontainer's backing services
(`.devcontainer/compose.instance.yml`).

- `postgres_health_check.rs` — `DatabaseHealthCheck` against Postgres
  (`POSTGRES_*`). Feature `postgres`.
- `redis_health_check.rs` — `RedisHealthCheck` against Valkey (`REDIS_URL`), including connection reuse.
  Feature `redis`.
- `s3_health_check.rs` — `S3HealthCheck` against RustFS (`S3_ENDPOINT_URL`,
  `RUSTFS_ACCESS_KEY`, `RUSTFS_SECRET_KEY`). Feature `s3`.
- `http_health_check.rs` — `HttpHealthCheck` against RustFS's `/health`.
  Feature `http`.
- `logs_configure_logging.rs` — `configure_logging`, which installs a
  process-global subscriber and so can run only once per process; Cargo
  builds each file here as its own process, so this is the only place it is
  called.

Unit tests live next to the code they test.

## Do

- Read connection details from the environment, with defaults that match the
  devcontainer.
- Keep a global-state test in a file of its own.
- Run the full suite with every feature (`--all-features`): the 100% line
  gate counts the feature-gated code.
