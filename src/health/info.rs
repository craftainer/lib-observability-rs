//! `GET /info`: the running build's version and commit.
//!
//! Opt-in and separate from [`router`](crate::health::router::router): a host
//! that wants the route merges [`info_router`] next to `router()`, and a host
//! that does not never exposes it. It is off by default because it discloses
//! the version (ADR 0006).
//!
//! The values come from the `APP_VERSION` and `APP_COMMIT` environment
//! variables, read on every request as `lib-observability-py` does, so the
//! router has no state and no build-time configuration. An unset, empty or
//! non-UTF-8 variable is reported as `null`. Unlike Python there is no
//! fallback to the package's own version: a crate cannot know its host's
//! version.

use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// The environment variable the version is read from.
const VERSION_VAR: &str = "APP_VERSION";
/// The environment variable the commit is read from.
const COMMIT_VAR: &str = "APP_COMMIT";

/// The `/info` response body: `{"version": ..., "commit": ...}`.
///
/// Non-exhaustive so fields can be added in minor releases; hosts never
/// construct it, the router does.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    /// `APP_VERSION`, or `None` when unset or empty.
    pub version: Option<String>,
    /// `APP_COMMIT`, or `None` when unset or empty.
    pub commit: Option<String>,
}

/// Builds the response from a variable lookup, so tests inject the
/// environment instead of mutating the process's.
fn build_info(get: impl Fn(&str) -> Option<String>) -> BuildInfo {
    let non_empty = |name: &str| get(name).filter(|value| !value.is_empty());
    BuildInfo {
        version: non_empty(VERSION_VAR),
        commit: non_empty(COMMIT_VAR),
    }
}

fn env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The `GET /info` route, for a host to merge next to
/// [`router`](crate::health::router::router) under the same prefix:
///
/// ```rust,ignore
/// let health = router().merge(info_router());
/// let app = Router::new().nest("/health", health).with_state(state);
/// ```
///
/// Generic over the host's state type, which it never touches. The route is
/// the bare `/info`: nest the router (as above) so it lands under `/health`
/// and not at the root.
pub fn info_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    info_router_with(env_lookup)
}

/// [`info_router`] over an injectable variable lookup.
fn info_router_with<S, F>(lookup: F) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    F: Fn(&str) -> Option<String> + Clone + Send + Sync + 'static,
{
    Router::new().route(
        "/info",
        get(move || {
            let lookup = lookup.clone();
            async move { Json(build_info(lookup)) }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::HealthRegistry;
    use crate::health::router::{HasHealthRegistry, router};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn env(
        pairs: &[(&str, &str)],
    ) -> impl Fn(&str) -> Option<String> + Clone + Send + Sync + 'static {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn both_set() {
        let got = build_info(env(&[("APP_VERSION", "1.2.3"), ("APP_COMMIT", "abc123")]));
        assert_eq!(got.version.as_deref(), Some("1.2.3"));
        assert_eq!(got.commit.as_deref(), Some("abc123"));
    }

    #[test]
    fn unset_is_none() {
        let got = build_info(env(&[]));
        assert_eq!(
            got,
            BuildInfo {
                version: None,
                commit: None
            }
        );
    }

    #[test]
    fn empty_is_none() {
        let got = build_info(env(&[("APP_VERSION", ""), ("APP_COMMIT", "")]));
        assert_eq!(got.version, None);
        assert_eq!(got.commit, None);
    }

    #[test]
    fn each_value_is_independent() {
        let got = build_info(env(&[("APP_VERSION", "2.0.0"), ("APP_COMMIT", "")]));
        assert_eq!(got.version.as_deref(), Some("2.0.0"));
        assert_eq!(got.commit, None);
    }

    #[test]
    fn serializes_nulls() {
        let json = serde_json::to_value(build_info(env(&[]))).unwrap();
        assert_eq!(json, serde_json::json!({"version": null, "commit": null}));
    }

    #[derive(Clone)]
    struct FakeState(Arc<HealthRegistry>);
    impl HasHealthRegistry for FakeState {
        fn health_registry(&self) -> &HealthRegistry {
            &self.0
        }
    }

    async fn get_path(app: Router, path: &str) -> axum::response::Response {
        app.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn merges_under_health_next_to_router() {
        let health = router::<FakeState>().merge(info_router());
        let app = Router::new()
            .nest("/health", health)
            .with_state(FakeState(Arc::new(HealthRegistry::new())));

        let live = get_path(app.clone(), "/health/live").await;
        assert_eq!(live.status(), StatusCode::OK);
        let ready = get_path(app.clone(), "/health/ready").await;
        assert_eq!(ready.status(), StatusCode::OK);

        let response = get_path(app, "/health/info").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let object = body.as_object().unwrap();
        // Both keys are always present; the values depend on the process's
        // environment, which this test does not mutate.
        assert!(object.contains_key("version") && object.contains_key("commit"));
        assert_eq!(object.len(), 2);
    }

    #[tokio::test]
    async fn reads_the_right_variables() {
        let app =
            info_router_with::<(), _>(env(&[("APP_VERSION", "1.2.3"), ("APP_COMMIT", "abc123")]));
        let response = get_path(app.with_state(()), "/info").await;
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"version": "1.2.3", "commit": "abc123"})
        );
    }

    #[tokio::test]
    async fn absent_unless_merged() {
        let app = router::<FakeState>().with_state(FakeState(Arc::new(HealthRegistry::new())));
        let response = get_path(app, "/info").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
