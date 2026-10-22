//! Health checks and the registry that runs them.
//!
//! [`router::router`] serves `/live` and `/ready`. `/live` is dependency-free
//! and never touches the registry, so a downstream outage cannot fail
//! liveness; `/ready` runs every registered [`HealthCheck`] concurrently.
//!
//! Every check runs under a timeout ([`DEFAULT_CHECK_TIMEOUT`], 5 seconds,
//! the default of `lib-observability-py`'s HTTP check), so one hung
//! dependency reports unhealthy instead of stalling `/ready`. Set it for the
//! whole registry with [`HealthRegistry::with_default_timeout`] and for one
//! check with [`HealthCheck::timeout`] (the ready-made checks have a
//! `with_timeout` builder for it).

pub mod checks;
#[cfg(any(
    feature = "postgres",
    feature = "redis",
    feature = "s3",
    feature = "http"
))]
mod redact;
pub mod router;

use std::time::Duration;

use async_trait::async_trait;

use crate::Error;

/// The timeout a check gets when neither it nor the registry sets one.
pub const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// The fixed, client-safe note for a check that exceeded its timeout.
const TIMEOUT_DETAIL: &str = "health check timed out";

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

    /// How long [`HealthRegistry::run_all`] waits for this check before
    /// reporting it unhealthy. `None` (the default) uses the registry's
    /// [`default_timeout`](HealthRegistry::default_timeout).
    fn timeout(&self) -> Option<Duration> {
        None
    }
}

/// Runs every registered check concurrently, each under a timeout.
pub struct HealthRegistry {
    checks: Vec<Box<dyn HealthCheck>>,
    default_timeout: Duration,
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self {
            checks: Vec::new(),
            default_timeout: DEFAULT_CHECK_TIMEOUT,
        }
    }
}

impl HealthRegistry {
    /// An empty registry with the [`DEFAULT_CHECK_TIMEOUT`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the timeout for every check that does not set its own.
    #[must_use]
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// The timeout for a check that does not set its own.
    #[must_use]
    pub fn default_timeout(&self) -> Duration {
        self.default_timeout
    }

    /// Adds a check.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DuplicateCheckName`] when a check with the same name
    /// is already registered; the registry is unchanged in that case.
    pub fn register(&mut self, check: Box<dyn HealthCheck>) -> Result<(), Error> {
        if self.checks.iter().any(|c| c.name() == check.name()) {
            return Err(Error::DuplicateCheckName(check.name().to_string()));
        }
        self.checks.push(check);
        Ok(())
    }

    /// Runs every check concurrently and returns `(name, result)` in
    /// registration order. A check that exceeds its timeout is reported
    /// unhealthy with a fixed detail, and its future is dropped.
    pub async fn run_all(&self) -> Vec<(String, HealthCheckResult)> {
        let futures = self.checks.iter().map(|check| async move {
            let limit = check.timeout().unwrap_or(self.default_timeout);
            let result = match tokio::time::timeout(limit, check.check()).await {
                Ok(result) => result,
                Err(_elapsed) => {
                    let timeout_ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX);
                    tracing::error!(check = check.name(), timeout_ms, "health check timed out");
                    HealthCheckResult {
                        healthy: false,
                        detail: Some(TIMEOUT_DETAIL.to_string()),
                    }
                }
            };
            (check.name().to_string(), result)
        });
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
        timeout: Option<Duration>,
    }

    #[async_trait]
    impl HealthCheck for StubCheck {
        fn name(&self) -> &str {
            self.name
        }

        fn timeout(&self) -> Option<Duration> {
            self.timeout
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
        registry
            .register(Box::new(StubCheck {
                name: "failing",
                healthy: false,
                delay: Duration::ZERO,
                calls: Arc::new(AtomicUsize::new(0)),
                timeout: None,
            }))
            .unwrap();
        registry
            .register(Box::new(StubCheck {
                name: "healthy",
                healthy: true,
                delay: Duration::ZERO,
                calls: Arc::new(AtomicUsize::new(0)),
                timeout: None,
            }))
            .unwrap();

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
            registry
                .register(Box::new(StubCheck {
                    name,
                    healthy: true,
                    delay: Duration::from_millis(200),
                    calls: Arc::new(AtomicUsize::new(0)),
                    timeout: None,
                }))
                .unwrap();
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
        registry
            .register(Box::new(StubCheck {
                name: "only",
                healthy: true,
                delay: Duration::ZERO,
                calls: calls.clone(),
                timeout: None,
            }))
            .unwrap();
        registry.run_all().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    fn stub(
        name: &'static str,
        delay: Duration,
        timeout: Option<Duration>,
    ) -> Box<dyn HealthCheck> {
        Box::new(StubCheck {
            name,
            healthy: true,
            delay,
            calls: Arc::new(AtomicUsize::new(0)),
            timeout,
        })
    }

    #[test]
    fn the_default_timeout_is_five_seconds_and_configurable() {
        assert_eq!(DEFAULT_CHECK_TIMEOUT, Duration::from_secs(5));
        let registry = HealthRegistry::new();
        assert_eq!(registry.default_timeout(), Duration::from_secs(5));
        let registry = registry.with_default_timeout(Duration::from_secs(1));
        assert_eq!(registry.default_timeout(), Duration::from_secs(1));
    }

    #[tokio::test]
    async fn a_hung_check_reports_unhealthy_without_stalling_the_others() {
        let mut registry = HealthRegistry::new().with_default_timeout(Duration::from_millis(100));
        registry
            .register(stub("hung", Duration::from_secs(3600), None))
            .unwrap();
        registry
            .register(stub("fine", Duration::ZERO, None))
            .unwrap();
        let start = std::time::Instant::now();
        let results = registry.run_all().await;
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(results[0].0, "hung");
        assert!(!results[0].1.healthy);
        assert_eq!(results[0].1.detail.as_deref(), Some(TIMEOUT_DETAIL));
        assert!(results[1].1.healthy);
    }

    #[tokio::test]
    async fn a_checks_own_timeout_overrides_the_registry_default() {
        // The registry allows an hour; the check allows 50ms and sleeps longer.
        let mut registry = HealthRegistry::new().with_default_timeout(Duration::from_secs(3600));
        registry
            .register(stub(
                "short",
                Duration::from_secs(3600),
                Some(Duration::from_millis(50)),
            ))
            .unwrap();
        // The reverse: the registry allows 50ms, the check allows an hour and
        // answers in 200ms, so it stays healthy.
        let mut lenient = HealthRegistry::new().with_default_timeout(Duration::from_millis(50));
        lenient
            .register(stub(
                "long",
                Duration::from_millis(200),
                Some(Duration::from_secs(3600)),
            ))
            .unwrap();
        assert!(!registry.run_all().await[0].1.healthy);
        assert!(lenient.run_all().await[0].1.healthy);
    }

    #[test]
    fn a_duplicate_name_is_rejected_and_leaves_the_registry_unchanged() {
        let mut registry = HealthRegistry::new();
        registry.register(stub("db", Duration::ZERO, None)).unwrap();
        let err = registry
            .register(stub("db", Duration::ZERO, None))
            .unwrap_err();
        assert!(matches!(err, Error::DuplicateCheckName(name) if name == "db"));
        assert_eq!(registry.checks.len(), 1);
        registry
            .register(stub("cache", Duration::ZERO, None))
            .unwrap();
        assert_eq!(registry.checks.len(), 2);
    }
}
