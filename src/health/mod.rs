//! The health-check interface and registry backing `/health/ready`.
//! `/health/live` (in `controllers::health`) stays dependency-free and
//! doesn't use this module at all -- see
//! `docs/adrs/0001-mvc-layering-with-a-generic-crud-interface.md`'s health
//! section for why liveness and readiness are split this way.

pub mod checks;

use async_trait::async_trait;

/// One check's outcome -- port of `health/base.py`'s `HealthCheckResult`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthCheckResult {
    pub healthy: bool,
    pub detail: Option<String>,
}

/// A single external dependency's liveness check. Object-safe (via
/// `async-trait`) so `HealthRegistry` can hold a heterogeneous
/// `Vec<Box<dyn HealthCheck>>` -- adding a new dependency means
/// implementing this trait and registering it, nothing else
/// (`docs/nfrs/0009-extensible-health-registry.md`).
#[async_trait]
pub trait HealthCheck: Send + Sync {
    fn name(&self) -> &str;
    async fn check(&self) -> HealthCheckResult;
}

/// Runs every registered check concurrently -- port of `health/registry.py`.
#[derive(Default)]
pub struct HealthRegistry {
    checks: Vec<Box<dyn HealthCheck>>,
}

impl HealthRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, check: Box<dyn HealthCheck>) {
        self.checks.push(check);
    }

    /// Run every check concurrently (`futures::future::join_all`-style),
    /// mirroring `asyncio.gather`. Each concrete check catches its own
    /// dependency's failure internally (`docs/nfrs/0008-health-check-
    /// isolation.md`) -- nothing here can panic the whole readiness
    /// response.
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
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
        assert!(registry.run_all().await.is_empty());
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
        assert_eq!(results.len(), 3);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "run_all took {:?}, expected concurrent execution well under 3x200ms",
            start.elapsed()
        );
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
