//! Concrete `HealthCheck` implementations -- port of `health/checks.py`.
//! Every failure is caught here (never propagated) and reported with a
//! fixed, non-leaking `detail` string; the real error goes to `tracing`
//! only (`docs/nfrs/0008-health-check-isolation.md`).

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};

use crate::health::{HealthCheck, HealthCheckResult};

const FAILURE_DETAIL: &str = "dependency check failed; see server logs";

fn failure(name: &str, err: impl std::fmt::Display) -> HealthCheckResult {
    tracing::error!(check = name, error = %err, "health check failed");
    HealthCheckResult {
        healthy: false,
        detail: Some(FAILURE_DETAIL.to_string()),
    }
}

fn ok() -> HealthCheckResult {
    HealthCheckResult {
        healthy: true,
        detail: None,
    }
}

/// `SELECT 1` against Postgres.
pub struct DatabaseHealthCheck {
    db: DatabaseConnection,
}

impl DatabaseHealthCheck {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl HealthCheck for DatabaseHealthCheck {
    fn name(&self) -> &str {
        "database"
    }

    async fn check(&self) -> HealthCheckResult {
        match self
            .db
            .execute(Statement::from_string(
                self.db.get_database_backend(),
                "SELECT 1",
            ))
            .await
        {
            Ok(_) => ok(),
            Err(err) => failure(self.name(), err),
        }
    }
}

/// `PING` against Redis/Valkey (see `docs/adrs/0013` on the Redis-protocol
/// name staying `redis`/`REDIS_URL` regardless of the server product).
pub struct RedisHealthCheck {
    redis_url: String,
}

impl RedisHealthCheck {
    pub fn new(redis_url: String) -> Self {
        Self { redis_url }
    }
}

#[async_trait]
impl HealthCheck for RedisHealthCheck {
    fn name(&self) -> &str {
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

/// `ListBuckets` against S3/RustFS.
pub struct S3HealthCheck {
    client: aws_sdk_s3::Client,
}

impl S3HealthCheck {
    pub fn new(client: aws_sdk_s3::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl HealthCheck for S3HealthCheck {
    fn name(&self) -> &str {
        "s3"
    }

    async fn check(&self) -> HealthCheckResult {
        match self.client.list_buckets().send().await {
            Ok(_) => ok(),
            Err(err) => failure(self.name(), err),
        }
    }
}

/// `GET {issuer}/.well-known/openid-configuration`.
pub struct OidcHealthCheck {
    issuer_url: String,
    http: reqwest::Client,
}

impl OidcHealthCheck {
    pub fn new(issuer_url: String) -> Self {
        Self {
            issuer_url,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl HealthCheck for OidcHealthCheck {
    fn name(&self) -> &str {
        "oidc"
    }

    async fn check(&self) -> HealthCheckResult {
        let url = format!(
            "{}/.well-known/openid-configuration",
            self.issuer_url.trim_end_matches('/')
        );
        let result: Result<(), reqwest::Error> = async {
            self.http.get(url).send().await?.error_for_status()?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => ok(),
            Err(err) => failure(self.name(), err),
        }
    }
}

/// Always-healthy stub used under `Mode::Mock` (FR-0012) -- no network,
/// no real dependency.
pub struct MockHealthCheck {
    name: &'static str,
}

impl MockHealthCheck {
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
