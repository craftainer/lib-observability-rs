//! Ready-made [`HealthCheck`] implementations.
//!
//! Each failing check catches its dependency's error here, reports a fixed,
//! non-leaking `detail` string, and sends the real error to `tracing` only.

use async_trait::async_trait;

use crate::health::{HealthCheck, HealthCheckResult};

#[cfg(any(
    feature = "postgres",
    feature = "redis",
    feature = "s3",
    feature = "http"
))]
const FAILURE_DETAIL: &str = "dependency check failed; see server logs";

#[cfg(any(
    feature = "postgres",
    feature = "redis",
    feature = "s3",
    feature = "http"
))]
fn failure(name: &str, err: impl std::fmt::Display) -> HealthCheckResult {
    tracing::error!(check = name, error = %err, "health check failed");
    HealthCheckResult {
        healthy: false,
        detail: Some(FAILURE_DETAIL.to_string()),
    }
}

#[cfg(any(
    feature = "postgres",
    feature = "redis",
    feature = "s3",
    feature = "http"
))]
fn ok() -> HealthCheckResult {
    HealthCheckResult {
        healthy: true,
        detail: None,
    }
}

/// `SELECT 1` against Postgres, over a host-owned pool.
#[cfg(feature = "postgres")]
pub struct DatabaseHealthCheck {
    pool: sqlx::PgPool,
}

#[cfg(feature = "postgres")]
impl DatabaseHealthCheck {
    /// Checks `pool`. A pool is cheap to clone, so pass a clone of the one
    /// the application already uses.
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[cfg(feature = "postgres")]
#[async_trait]
impl HealthCheck for DatabaseHealthCheck {
    fn name(&self) -> &'static str {
        "database"
    }

    async fn check(&self) -> HealthCheckResult {
        match sqlx::query("SELECT 1").execute(&self.pool).await {
            Ok(_) => ok(),
            Err(err) => failure(self.name(), err),
        }
    }
}

/// `PING` against Redis or Valkey.
#[cfg(feature = "redis")]
pub struct RedisHealthCheck {
    redis_url: String,
}

#[cfg(feature = "redis")]
impl RedisHealthCheck {
    /// Checks the server at `redis_url` (for example `redis://host:6379/0`).
    #[must_use]
    pub fn new(redis_url: impl Into<String>) -> Self {
        Self {
            redis_url: redis_url.into(),
        }
    }
}

#[cfg(feature = "redis")]
#[async_trait]
impl HealthCheck for RedisHealthCheck {
    fn name(&self) -> &'static str {
        "redis"
    }

    async fn check(&self) -> HealthCheckResult {
        let result: Result<(), redis::RedisError> = async {
            let client = redis::Client::open(self.redis_url.as_str())?;
            let mut conn = client.get_multiplexed_async_connection().await?;
            redis::cmd("PING").query_async::<String>(&mut conn).await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => ok(),
            Err(err) => failure(self.name(), err),
        }
    }
}

/// `ListBuckets` against S3 or an S3-compatible store.
#[cfg(feature = "s3")]
pub struct S3HealthCheck {
    client: aws_sdk_s3::Client,
}

#[cfg(feature = "s3")]
impl S3HealthCheck {
    /// Checks the store `client` is configured for.
    #[must_use]
    pub fn new(client: aws_sdk_s3::Client) -> Self {
        Self { client }
    }
}

#[cfg(feature = "s3")]
#[async_trait]
impl HealthCheck for S3HealthCheck {
    fn name(&self) -> &'static str {
        "s3"
    }

    async fn check(&self) -> HealthCheckResult {
        match self.client.list_buckets().send().await {
            Ok(_) => ok(),
            Err(err) => failure(self.name(), err),
        }
    }
}

/// `GET` a URL and expect a 2xx answer, for example an `OpenID` Connect
/// issuer's `/.well-known/openid-configuration`.
#[cfg(feature = "http")]
pub struct HttpHealthCheck {
    name: String,
    url: String,
    http: reqwest::Client,
}

#[cfg(feature = "http")]
impl HttpHealthCheck {
    /// Reports under `name` and probes `url`.
    #[must_use]
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            http: reqwest::Client::new(),
        }
    }
}

#[cfg(feature = "http")]
#[async_trait]
impl HealthCheck for HttpHealthCheck {
    fn name(&self) -> &str {
        &self.name
    }

    async fn check(&self) -> HealthCheckResult {
        let result: Result<(), reqwest::Error> = async {
            self.http.get(&self.url).send().await?.error_for_status()?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => ok(),
            Err(err) => failure(self.name(), err),
        }
    }
}

/// An always-healthy stub for modes that run without infrastructure: no
/// network, no real dependency.
pub struct MockHealthCheck {
    name: &'static str,
}

impl MockHealthCheck {
    /// Reports healthy under `name`.
    #[must_use]
    pub fn new(name: &'static str) -> Self {
        Self { name }
    }
}

#[async_trait]
impl HealthCheck for MockHealthCheck {
    fn name(&self) -> &str {
        self.name
    }

    async fn check(&self) -> HealthCheckResult {
        HealthCheckResult {
            healthy: true,
            detail: Some("mocked".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_health_check_is_always_healthy_with_a_mocked_marker() {
        let check = MockHealthCheck::new("database");
        assert_eq!(check.name(), "database");
        let result = check.check().await;
        assert!(result.healthy);
        assert_eq!(result.detail.as_deref(), Some("mocked"));
    }

    #[cfg(any(
        feature = "postgres",
        feature = "redis",
        feature = "s3",
        feature = "http"
    ))]
    #[test]
    fn ok_helper_reports_healthy_with_no_detail() {
        let result = ok();
        assert!(result.healthy);
        assert!(result.detail.is_none());
    }

    #[cfg(any(
        feature = "postgres",
        feature = "redis",
        feature = "s3",
        feature = "http"
    ))]
    #[test]
    fn failure_helper_reports_unhealthy_with_a_fixed_non_leaking_detail() {
        let result = failure("database", "connection refused: password=hunter2");
        assert!(!result.healthy);
        // The real error text must never reach the client, only the fixed
        // string does.
        assert_eq!(result.detail.as_deref(), Some(FAILURE_DETAIL));
        assert!(!result.detail.unwrap().contains("hunter2"));
    }

    // No live service is needed for the failure paths: an unroutable address
    // is refused at once rather than hanging.

    #[cfg(feature = "redis")]
    #[tokio::test]
    async fn redis_health_check_reports_unhealthy_for_an_unreachable_url() {
        let check = RedisHealthCheck::new("redis://127.0.0.1:1/0");
        assert_eq!(check.name(), "redis");
        let result = check.check().await;
        assert!(!result.healthy);
        assert_eq!(result.detail.as_deref(), Some(FAILURE_DETAIL));
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn http_health_check_reports_unhealthy_for_an_unreachable_url() {
        let check = HttpHealthCheck::new("issuer", "http://127.0.0.1:1/ping");
        assert_eq!(check.name(), "issuer");
        let result = check.check().await;
        assert!(!result.healthy);
        assert_eq!(result.detail.as_deref(), Some(FAILURE_DETAIL));
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn database_health_check_reports_unhealthy_for_an_unreachable_pool() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect_lazy("postgres://u:p@127.0.0.1:1/none")
            .unwrap();
        let check = DatabaseHealthCheck::new(pool);
        assert_eq!(check.name(), "database");
        let result = check.check().await;
        assert!(!result.healthy);
        assert_eq!(result.detail.as_deref(), Some(FAILURE_DETAIL));
    }
}
