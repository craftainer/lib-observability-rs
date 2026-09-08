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
