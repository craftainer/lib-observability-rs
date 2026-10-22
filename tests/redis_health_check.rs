//! Integration tier: `RedisHealthCheck` against the devcontainer's real
//! Redis-compatible server (`REDIS_URL`).

use craftainer_observability::health::HealthCheck;
use craftainer_observability::health::checks::RedisHealthCheck;

#[tokio::test]
async fn redis_health_check_reports_healthy_against_a_real_redis() {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://redis:6379/0".to_string());
    let check = RedisHealthCheck::new(url);
    assert_eq!(check.name(), "redis");
    let result = check.check().await;
    assert!(result.healthy, "{:?}", result.detail);
    assert!(result.detail.is_none());
}

#[tokio::test]
async fn redis_health_check_reuses_its_connection_across_checks() {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://redis:6379/0".to_string());
    let check = RedisHealthCheck::new(url);
    for _ in 0..3 {
        assert!(check.check().await.healthy);
    }
}
