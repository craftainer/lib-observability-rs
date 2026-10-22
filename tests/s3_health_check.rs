//! Integration tier: `health::checks::S3HealthCheck`'s happy path against
//! the devcontainer stack's real RustFS/S3 -- this and its Redis/Oidc
//! siblings were `docs/adrs/0010`'s last "connects to a real backend from
//! an `expect()`-guarded path" gap; RustFS is already a live dependency
//! of this stack, so there is no reason left to leave it uncovered.

mod common;

use template_axum::health::checks::S3HealthCheck;
use template_axum::health::HealthCheck;

#[tokio::test]
async fn s3_health_check_reports_healthy_against_a_real_s3() {
    let settings = common::dev_settings(None);
    let s3_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(&settings.s3_endpoint_url)
        .credentials_provider(aws_sdk_s3::config::Credentials::new(
            &settings.s3_access_key,
            &settings.s3_secret_key,
            None,
            None,
            "template-axum",
        ))
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .load()
        .await;
    let client = aws_sdk_s3::Client::new(&s3_config);
    let check = S3HealthCheck::new(client);
    assert_eq!(check.name(), "s3");
    let result = check.check().await;
    assert!(result.healthy, "{:?}", result.detail);
    assert!(result.detail.is_none());
}
