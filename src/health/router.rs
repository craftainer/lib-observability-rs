//! `GET /live` and `GET /ready`.
//!
//! Generic over any state that implements [`HasHealthRegistry`], so the host
//! keeps its own state type.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::health::{HealthCheckResult, HealthRegistry};

/// Implemented by a host's application state to give [`router`] its
/// registry.
pub trait HasHealthRegistry {
    /// The registry `/ready` runs.
    fn health_registry(&self) -> &HealthRegistry;
}

#[derive(Serialize)]
struct LiveResponse {
    status: &'static str,
}

/// Dependency-free liveness probe: never touches the registry, so a
/// downstream outage cannot fail liveness.
async fn live() -> Json<LiveResponse> {
    Json(LiveResponse { status: "ok" })
}

#[derive(Serialize)]
struct ReadyResponse {
    status: &'static str,
    checks: BTreeMap<String, HealthCheckResult>,
}

/// Runs every registered check concurrently; 200 if all are healthy, 503
/// otherwise.
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

/// The `/live` and `/ready` routes, for a host to `nest` under `/health`.
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
    use crate::health::HealthCheck;
    use crate::health::checks::MockHealthCheck;
    use async_trait::async_trait;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    struct AlwaysUnhealthy;
    #[async_trait]
    impl HealthCheck for AlwaysUnhealthy {
        fn name(&self) -> &'static str {
            "flaky-dependency"
        }
        async fn check(&self) -> HealthCheckResult {
            HealthCheckResult {
                healthy: false,
                detail: Some("down".to_string()),
            }
        }
    }

    // A minimal fake implementing only `HasHealthRegistry`, so the router
    // is tested without any application state.
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
        // The one healthy check must still report healthy: one failure does
        // not taint the others.
        assert_eq!(body["checks"]["database"]["healthy"], true);
        assert_eq!(body["checks"]["flaky-dependency"]["healthy"], false);
    }
}
