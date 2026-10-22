# stack/

One subdirectory per supporting service the integration tests need that is
not defined inline in `../compose.instance.yml`. Each holds a `compose.yml`
fragment (listed in that file's `include:` list) and any config it
reads. No fragment declares `ports:` or `networks:`: services reach one
another by name on the compose network.

- `redis/` — Valkey, the server behind `tests/redis_health_check.rs` (the
  `redis` feature). Host `redis`, port 6379, no auth (local development
  only).
- `s3/` — RustFS, an S3-compatible store: the backend of
  `tests/s3_health_check.rs` (the `s3` feature) and the HTTP endpoint
  `tests/http_health_check.rs` probes (the `http` feature). Host `s3`, port
  9000; the local-dev credentials are in `s3.env`.

Postgres, which `tests/postgres_health_check.rs` needs, is defined inline in
`../compose.instance.yml`.
