//! Integration tier: `S3HealthCheck` against the devcontainer's real
//! S3-compatible store (`S3_ENDPOINT_URL`, `RUSTFS_ACCESS_KEY`,
//! `RUSTFS_SECRET_KEY`).

use craftainer_observability::health::HealthCheck;
use craftainer_observability::health::checks::S3HealthCheck;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::test]
async fn s3_health_check_reports_healthy_against_a_real_s3() {
    let credentials = aws_sdk_s3::config::Credentials::new(
        env_or("RUSTFS_ACCESS_KEY", "rustfsadmin"),
        env_or("RUSTFS_SECRET_KEY", "rustfsadmin"),
        None,
        None,
        "observability-tests",
    );
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(env_or("S3_ENDPOINT_URL", "http://s3:9000"))
        .credentials_provider(credentials)
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .load()
        .await;
    let check = S3HealthCheck::new(aws_sdk_s3::Client::new(&config));
    assert_eq!(check.name(), "s3");
    let result = check.check().await;
    assert!(result.healthy, "{:?}", result.detail);
    assert!(result.detail.is_none());
}
