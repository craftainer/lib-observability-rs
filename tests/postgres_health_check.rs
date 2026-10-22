//! Integration tier: `DatabaseHealthCheck` against the devcontainer's real
//! Postgres (`POSTGRES_*`).

use craftainer_observability::health::HealthCheck;
use craftainer_observability::health::checks::DatabaseHealthCheck;
use sqlx::postgres::PgPoolOptions;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::test]
async fn database_health_check_reports_healthy_against_a_real_postgres() {
    let url = format!(
        "postgres://{}:{}@{}:{}/{}",
        env_or("POSTGRES_USER", "observability"),
        env_or("POSTGRES_PASSWORD", "observability"),
        env_or("POSTGRES_HOST", "postgres"),
        env_or("POSTGRES_PORT", "5432"),
        env_or("POSTGRES_DB", "observability_test"),
    );
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("the devcontainer's Postgres must be running for the integration tier");
    let check = DatabaseHealthCheck::new(pool);
    assert_eq!(check.name(), "database");
    let result = check.check().await;
    assert!(result.healthy, "{:?}", result.detail);
    assert!(result.detail.is_none());
}
