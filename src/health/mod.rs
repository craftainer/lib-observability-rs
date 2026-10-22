//! Health checks and the registry that runs them.
//!
//! [`router::router`] serves `/live` and `/ready`. `/live` is dependency-free
//! and never touches the registry, so a downstream outage cannot fail
//! liveness; `/ready` runs every registered [`HealthCheck`] concurrently.

pub mod checks;
pub mod router;

use async_trait::async_trait;

/// One check's outcome.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HealthCheckResult {
    /// Whether the dependency answered as expected.
    pub healthy: bool,
    /// A short, client-safe note. The ready-made failing checks use a fixed
    /// string and log the real error instead, so it never leaks to a client.
    pub detail: Option<String>,
}

/// A single external dependency's check.
///
/// Object-safe (via `async-trait`) so [`HealthRegistry`] can hold a
/// heterogeneous list: adding a dependency means implementing this trait and
/// registering the value, nothing else. An implementation must catch its own
/// dependency's failure and report it as an unhealthy [`HealthCheckResult`]
/// rather than panic, so one broken dependency cannot take the readiness
/// response down.
#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// The key this check's result appears under in `/ready`'s response.
    fn name(&self) -> &str;

    /// Probes the dependency.
    async fn check(&self) -> HealthCheckResult;
}

/// Runs every registered check concurrently.
#[derive(Default)]
pub struct HealthRegistry {
    checks: Vec<Box<dyn HealthCheck>>,
}

impl HealthRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a check.
    pub fn register(&mut self, check: Box<dyn HealthCheck>) {
        self.checks.push(check);
    }

    /// Runs every check concurrently and returns `(name, result)` in
    /// registration order.
    pub async fn run_all(&self) -> Vec<(String, HealthCheckResult)> {
        let futures = self
            .checks
            .iter()
            .map(|check| async move { (check.name().to_string(), check.check().await) });
        futures::future::join_all(futures).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// A check whose outcome and (simulated) latency are set at
    /// construction -- lets tests assert both isolation (one failure
    /// doesn't block/fail another) and concurrency (total wall time is
    /// close to the slowest single check, not their sum) without touching
    /// a real dependency.
    struct StubCheck {
        name: &'static str,
        healthy: bool,
        delay: Duration,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl HealthCheck for StubCheck {
        fn name(&self) -> &str {
            self.name
        }

        async fn check(&self) -> HealthCheckResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            HealthCheckResult {
                healthy: self.healthy,
                detail: if self.healthy {
                    None
                } else {
                    Some("stub failure".to_string())
                },
            }
        }
    }

    #[tokio::test]
    async fn empty_registry_reports_no_checks() {
        let registry = HealthRegistry::new();
        assert_eq!(registry.run_all().await.len(), 0);
    }

    #[tokio::test]
    async fn a_failing_check_does_not_block_or_alter_others() {
        let mut registry = HealthRegistry::new();
        registry.register(Box::new(StubCheck {
            name: "failing",
            healthy: false,
            delay: Duration::ZERO,
            calls: Arc::new(AtomicUsize::new(0)),
        }));
        registry.register(Box::new(StubCheck {
            name: "healthy",
            healthy: true,
            delay: Duration::ZERO,
            calls: Arc::new(AtomicUsize::new(0)),
        }));

        let results = registry.run_all().await;
        assert_eq!(results.len(), 2);
        let failing = results.iter().find(|(name, _)| name == "failing").unwrap();
        let healthy = results.iter().find(|(name, _)| name == "healthy").unwrap();
        assert!(!failing.1.healthy);
        assert_eq!(failing.1.detail.as_deref(), Some("stub failure"));
        assert!(healthy.1.healthy);
        assert!(healthy.1.detail.is_none());
    }

    #[tokio::test]
    async fn checks_run_concurrently_not_sequentially() {
        // Three checks that each sleep 200ms: sequential execution takes
        // ~600ms, concurrent execution ~200ms. A generous 500ms ceiling
        // distinguishes the two without being a flaky exact-timing assert.
        let mut registry = HealthRegistry::new();
        for name in ["a", "b", "c"] {
            registry.register(Box::new(StubCheck {
                name,
                healthy: true,
                delay: Duration::from_millis(200),
                calls: Arc::new(AtomicUsize::new(0)),
            }));
        }
        let start = std::time::Instant::now();
        let results = registry.run_all().await;
        let elapsed = start.elapsed();
        assert_eq!(results.len(), 3);
        assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
    }

    #[tokio::test]
    async fn every_registered_check_is_invoked_exactly_once() {
        let mut registry = HealthRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        registry.register(Box::new(StubCheck {
            name: "only",
            healthy: true,
            delay: Duration::ZERO,
            calls: calls.clone(),
        }));
        registry.run_all().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
