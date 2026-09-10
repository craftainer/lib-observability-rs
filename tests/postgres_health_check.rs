//! Integration tier: `health::checks::DatabaseHealthCheck` against the
//! devcontainer stack's real Postgres. The `S3`/`Redis`/`Oidc` checks'
//! success paths stay deliberately uncovered (`docs/adrs/0010`); the
//! database check isn't one of those -- Postgres is already this tier's
//! own dependency, so its `SELECT 1` happy path is covered here.

mod common;

use common::IsolatedDb;
use template_axum::health::checks::DatabaseHealthCheck;
use template_axum::health::HealthCheck;

#[tokio::test]
async fn database_health_check_reports_healthy_against_a_real_postgres() {
    let db = IsolatedDb::new().await;
    let check = DatabaseHealthCheck::new(db.connection.clone());
    assert_eq!(check.name(), "database");
    let result = check.check().await;
    assert!(result.healthy);
    assert!(result.detail.is_none());
    db.cleanup().await;
}
