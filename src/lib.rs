//! Observability building blocks for Axum services: health checks and the
//! `/live` and `/ready` router that serves them, structured JSON logging
//! with optional OTLP export of logs, traces and metrics
//! ([`configure_telemetry`]), and [`request_id::RequestIdLayer`], a tower
//! layer that validates, creates and echoes a request ID.
//!
//! The crate knows nothing about an application's configuration, error type
//! or state: a host passes plain values (a pool, a client, a URL) to the
//! checks it wants, implements [`health::router::HasHealthRegistry`] on its
//! state, and calls [`configure_telemetry`] once at startup.
//!
//! # Features
//!
//! Everything is off by default, so a service compiles only the clients it
//! has.
//!
//! - `postgres`: [`health::checks::DatabaseHealthCheck`].
//! - `redis`: [`health::checks::RedisHealthCheck`].
//! - `s3`: [`health::checks::S3HealthCheck`].
//! - `http`: [`health::checks::HttpHealthCheck`].
//! - `otel`: OTLP/HTTP export of logs, traces and metrics from
//!   [`configure_telemetry`], plus the `otel` module (`ServerSpanLayer`,
//!   and with `postgres` also `instrument_db`).
//! - `analytics`: [`analytics::track`] and the `ANALYTICS_*` adapters (none,
//!   log, otel with the `otel` feature, Umami).
//! - `testing`: `health::checks::MockHealthCheck`, an always-healthy stub.
//!
//! `sqlx` is built without a TLS backend; see the README's "sqlx TLS".
//!
//! # Examples
//!
//! ```
//! use async_trait::async_trait;
//! use craftainer_observability::health::{HealthCheck, HealthCheckResult, HealthRegistry};
//! use std::time::Duration;
//!
//! struct Always;
//!
//! #[async_trait]
//! impl HealthCheck for Always {
//!     fn name(&self) -> &str {
//!         "always"
//!     }
//!     async fn check(&self) -> HealthCheckResult {
//!         HealthCheckResult { healthy: true, detail: None }
//!     }
//! }
//!
//! let mut registry = HealthRegistry::new().with_default_timeout(Duration::from_secs(2));
//! registry.register(Box::new(Always)).expect("names are unique");
//! ```

#[cfg(feature = "analytics")]
pub mod analytics;
pub mod error;
pub mod health;
pub mod logs;
#[cfg(feature = "otel")]
pub mod otel;
pub mod request_id;

pub use error::Error;
pub use logs::{Telemetry, configure_telemetry};
