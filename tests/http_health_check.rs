//! Integration tier: `HttpHealthCheck` against a real HTTP server, the
//! devcontainer's S3 store, whose `/health` answers 200 (`S3_ENDPOINT_URL`).

use craftainer_observability::health::HealthCheck;
use craftainer_observability::health::checks::HttpHealthCheck;

fn base_url() -> String {
    std::env::var("S3_ENDPOINT_URL").unwrap_or_else(|_| "http://s3:9000".to_string())
}

#[tokio::test]
async fn http_health_check_reports_healthy_for_a_2xx_answer() {
    let check = HttpHealthCheck::new("storage", format!("{}/health", base_url()));
    assert_eq!(check.name(), "storage");
    let result = check.check().await;
    assert!(result.healthy, "{:?}", result.detail);
    assert!(result.detail.is_none());
}

#[tokio::test]
async fn http_health_check_reports_unhealthy_for_a_non_2xx_answer() {
    let check = HttpHealthCheck::new("storage", format!("{}/no-such-path", base_url()));
    let result = check.check().await;
    assert!(!result.healthy);
    assert!(result.detail.is_some());
}
