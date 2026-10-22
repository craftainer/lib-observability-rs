//! Ready-made [`HealthCheck`] implementations.
//!
//! Each failing check catches its dependency's error here, reports a fixed,
//! non-leaking `detail` string, and sends the real error to `tracing` only,
//! with credentials (URL userinfo, `password=...`) redacted.
//!
//! Every ready-made check has a `with_timeout` builder; without it the
//! registry's default applies (see [`crate::health`]).
//!
//! [`MockHealthCheck`] is behind the `testing` feature.

#[cfg(any(
    feature = "postgres",
    feature = "redis",
    feature = "s3",
    feature = "http"
))]
use std::time::Duration;

#[cfg(any(
    feature = "postgres",
    feature = "redis",
    feature = "s3",
    feature = "http",
    feature = "testing",
    test
))]
use async_trait::async_trait;

#[cfg(any(
    feature = "postgres",
    feature = "redis",
    feature = "s3",
    feature = "http",
    feature = "testing",
    test
))]
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
    let error = crate::health::redact::redact(&err.to_string());
    tracing::error!(check = name, %error, "health check failed");
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
///
/// The crate builds `sqlx` without a TLS backend. To reach a server that
/// requires TLS, enable one of sqlx's `tls-*` features in the host (Cargo
/// unifies features, so the host's choice applies); see "sqlx TLS" in the
/// README.
#[cfg(feature = "postgres")]
pub struct DatabaseHealthCheck {
    pool: sqlx::PgPool,
    timeout: Option<Duration>,
}

#[cfg(feature = "postgres")]
impl DatabaseHealthCheck {
    /// Checks `pool`. A pool is cheap to clone, so pass a clone of the one
    /// the application already uses.
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            timeout: None,
        }
    }

    /// Gives up on the check after `timeout` instead of the registry default.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

#[cfg(feature = "postgres")]
#[async_trait]
impl HealthCheck for DatabaseHealthCheck {
    fn name(&self) -> &'static str {
        "database"
    }

    fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    async fn check(&self) -> HealthCheckResult {
        match sqlx::query("SELECT 1").execute(&self.pool).await {
            Ok(_) => ok(),
            Err(err) => failure(self.name(), err),
        }
    }
}

/// `PING` against Redis or Valkey.
///
/// The connection is a [`redis::aio::ConnectionManager`], opened on the
/// first check and reused by every later one; it reconnects by itself after
/// an outage. (`lib-observability-py` opens and closes a client per check;
/// this keeps one.) The connection belongs to the Tokio runtime that ran the
/// first check, so use one check value per runtime.
#[cfg(feature = "redis")]
pub struct RedisHealthCheck {
    redis_url: String,
    timeout: Option<Duration>,
    connection: tokio::sync::OnceCell<redis::aio::ConnectionManager>,
}

#[cfg(feature = "redis")]
impl RedisHealthCheck {
    /// Checks the server at `redis_url` (for example `redis://host:6379/0`).
    #[must_use]
    pub fn new(redis_url: impl Into<String>) -> Self {
        Self {
            redis_url: redis_url.into(),
            timeout: None,
            connection: tokio::sync::OnceCell::new(),
        }
    }

    /// Gives up on the check after `timeout` instead of the registry default.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// The shared connection, created on first use. No reconnect retries
    /// inside one call: an unreachable server is an immediate unhealthy
    /// answer, and the next probe tries again.
    async fn connection(&self) -> Result<redis::aio::ConnectionManager, redis::RedisError> {
        self.connection
            .get_or_try_init(|| async {
                let client = redis::Client::open(self.redis_url.as_str())?;
                let config = redis::aio::ConnectionManagerConfig::new().set_number_of_retries(0);
                redis::aio::ConnectionManager::new_with_config(client, config).await
            })
            .await
            .cloned()
    }
}

#[cfg(feature = "redis")]
#[async_trait]
impl HealthCheck for RedisHealthCheck {
    fn name(&self) -> &'static str {
        "redis"
    }

    fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    async fn check(&self) -> HealthCheckResult {
        let result: Result<(), redis::RedisError> = async {
            let mut conn = self.connection().await?;
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
    timeout: Option<Duration>,
}

#[cfg(feature = "s3")]
impl S3HealthCheck {
    /// Checks the store `client` is configured for.
    #[must_use]
    pub fn new(client: aws_sdk_s3::Client) -> Self {
        Self {
            client,
            timeout: None,
        }
    }

    /// Gives up on the check after `timeout` instead of the registry default.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

#[cfg(feature = "s3")]
#[async_trait]
impl HealthCheck for S3HealthCheck {
    fn name(&self) -> &'static str {
        "s3"
    }

    fn timeout(&self) -> Option<Duration> {
        self.timeout
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
    timeout: Option<Duration>,
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
            timeout: None,
        }
    }

    /// Gives up on the check after `timeout` instead of the registry default.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

#[cfg(feature = "http")]
#[async_trait]
impl HealthCheck for HttpHealthCheck {
    fn name(&self) -> &str {
        &self.name
    }

    fn timeout(&self) -> Option<Duration> {
        self.timeout
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

/// An always-healthy stub for tests and for modes that run without
/// infrastructure: no network, no real dependency. Behind the `testing`
/// feature, so a production build cannot register it by accident.
#[cfg(any(test, feature = "testing"))]
pub struct MockHealthCheck {
    name: &'static str,
}

#[cfg(any(test, feature = "testing"))]
impl MockHealthCheck {
    /// Reports healthy under `name`.
    #[must_use]
    pub fn new(name: &'static str) -> Self {
        Self { name }
    }
}

#[cfg(any(test, feature = "testing"))]
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

    /// A `tracing` writer that appends to a shared buffer.
    #[cfg(any(
        feature = "postgres",
        feature = "redis",
        feature = "s3",
        feature = "http"
    ))]
    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    #[cfg(any(
        feature = "postgres",
        feature = "redis",
        feature = "s3",
        feature = "http"
    ))]
    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[cfg(any(
        feature = "postgres",
        feature = "redis",
        feature = "s3",
        feature = "http"
    ))]
    #[test]
    fn the_logged_error_has_its_credentials_redacted() {
        let capture = Capture::default();
        std::io::Write::flush(&mut capture.clone()).unwrap();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            failure(
                "database",
                "error connecting to postgres://app:hunter2@db/x password=hunter3",
            );
        });
        let logged = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(logged.contains("health check failed"), "{logged}");
        assert!(logged.contains("postgres://[REDACTED]@db/x"), "{logged}");
        assert!(logged.contains("password=[REDACTED]"), "{logged}");
        assert!(!logged.contains("hunter"), "{logged}");
    }

    // No live service is needed for the failure paths: an unroutable address
    // is refused at once rather than hanging.

    #[cfg(feature = "redis")]
    #[tokio::test]
    async fn redis_health_check_reports_unhealthy_for_an_unreachable_url() {
        let check = RedisHealthCheck::new("redis://127.0.0.1:1/0");
        assert_eq!(check.name(), "redis");
        assert_eq!(check.timeout(), None);
        let result = check.check().await;
        assert!(!result.healthy);
        assert_eq!(result.detail.as_deref(), Some(FAILURE_DETAIL));
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn http_health_check_reports_unhealthy_for_an_unreachable_url() {
        let check = HttpHealthCheck::new("issuer", "http://127.0.0.1:1/ping");
        assert_eq!(check.name(), "issuer");
        assert_eq!(check.timeout(), None);
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
        assert_eq!(check.timeout(), None);
        let result = check.check().await;
        assert!(!result.healthy);
        assert_eq!(result.detail.as_deref(), Some(FAILURE_DETAIL));
    }

    #[cfg(feature = "redis")]
    #[tokio::test]
    async fn redis_health_check_rejects_an_invalid_url_as_unhealthy() {
        let check = RedisHealthCheck::new("not a url").with_timeout(Duration::from_secs(1));
        assert_eq!(check.timeout(), Some(Duration::from_secs(1)));
        assert!(!check.check().await.healthy);
    }

    #[cfg(feature = "http")]
    #[test]
    fn http_health_check_timeout_is_configurable() {
        let check =
            HttpHealthCheck::new("issuer", "http://x/").with_timeout(Duration::from_secs(2));
        assert_eq!(check.timeout(), Some(Duration::from_secs(2)));
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn database_health_check_timeout_is_configurable() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://u:p@127.0.0.1:1/none")
            .unwrap();
        let check = DatabaseHealthCheck::new(pool).with_timeout(Duration::from_secs(2));
        assert_eq!(check.timeout(), Some(Duration::from_secs(2)));
    }

    #[cfg(feature = "s3")]
    #[tokio::test]
    async fn s3_health_check_reports_unhealthy_for_an_unreachable_endpoint_and_has_a_timeout() {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .endpoint_url("http://127.0.0.1:1")
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "a", "b", None, None, "t",
            ))
            .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
            .build();
        let check = S3HealthCheck::new(aws_sdk_s3::Client::from_conf(config))
            .with_timeout(Duration::from_secs(3));
        assert_eq!(check.timeout(), Some(Duration::from_secs(3)));
        let result = check.check().await;
        assert!(!result.healthy);
        assert_eq!(result.detail.as_deref(), Some(FAILURE_DETAIL));
    }
}
