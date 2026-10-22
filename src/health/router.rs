//! `GET /health/live` and `GET /health/ready` -- port of `controllers/health.py`.
//! Generic over any state `S: HasHealthRegistry` -- see this package's
//! own module doc.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::generic::controllers::HasHealthRegistry;
use crate::health::HealthCheckResult;

#[derive(Serialize)]
struct LiveResponse {
    status: &'static str,
}

/// Dependency-free liveness probe (FR-0010) -- never touches
/// `HealthRegistry`, so a downstream outage can't fail liveness.
async fn live() -> Json<LiveResponse> {
    Json(LiveResponse { status: "ok" })
}

#[derive(Serialize)]
struct ReadyResponse {
    status: &'static str,
    checks: BTreeMap<String, HealthCheckResult>,
}

/// Runs every registered check concurrently; 200 if all healthy, 503
/// otherwise (FR-0011).
async fn ready<S>(State(state): State<S>) -> impl IntoResponse
where
    S: HasHealthRegistry,
{
    let results = state.health_registry().run_all().await;
    let healthy = results.iter().all(|(_, result)| result.healthy);
    let checks: BTreeMap<String, HealthCheckResult> = results.into_iter().collect();
    let status = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = ReadyResponse {
        status: if healthy { "ok" } else { "degraded" },
        checks,
    };
    (status, Json(body))
}

pub fn router<S>() -> Router<S>
where
    S: HasHealthRegistry + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/live", get(live))
        .route("/ready", get(ready::<S>))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::checks::MockHealthCheck;
    use crate::health::{HealthCheck, HealthRegistry};
    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    struct AlwaysUnhealthy;
    #[async_trait]
    impl HealthCheck for AlwaysUnhealthy {
        fn name(&self) -> &str {
            "flaky-dependency"
        }
        async fn check(&self) -> crate::health::HealthCheckResult {
            crate::health::HealthCheckResult {
                healthy: false,
                detail: Some("down".to_string()),
            }
        }
    }

    // A minimal fake implementing only `HasHealthRegistry`, not the
    // concrete Hero-bearing `AppState` -- keeps this generic package's
    // tests from depending on `crate::hero` at all. Mirrors
    // `oidc::mod::tests::FakeState`'s own pattern for `HasOidcVerifier`.
    #[derive(Clone)]
    struct FakeState {
        health_registry: Arc<HealthRegistry>,
    }

    impl HasHealthRegistry for FakeState {
        fn health_registry(&self) -> &HealthRegistry {
            &self.health_registry
        }
    }

    fn state_with_registry(registry: HealthRegistry) -> FakeState {
        FakeState {
            health_registry: Arc::new(registry),
        }
    }

    #[tokio::test]
    async fn live_never_requires_auth_or_touches_dependencies() {
        let app = router().with_state(state_with_registry(HealthRegistry::new()));
        let response = app
            .oneshot(Request::builder().uri("/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn ready_returns_200_when_every_check_is_healthy() {
        let mut registry = HealthRegistry::new();
        registry.register(Box::new(MockHealthCheck::new("database")));
        registry.register(Box::new(MockHealthCheck::new("redis")));
        let app = router().with_state(state_with_registry(registry));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["checks"]["database"]["healthy"], true);
    }

    #[tokio::test]
    async fn ready_returns_503_when_any_single_check_is_unhealthy() {
        let mut registry = HealthRegistry::new();
        registry.register(Box::new(MockHealthCheck::new("database")));
        registry.register(Box::new(AlwaysUnhealthy));
        let app = router().with_state(state_with_registry(registry));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "degraded");
        // The one healthy check must still report healthy -- one failure
        // doesn't taint the others (NFR-0008).
        assert_eq!(body["checks"]["database"]["healthy"], true);
        assert_eq!(body["checks"]["flaky-dependency"]["healthy"], false);
    }
}
