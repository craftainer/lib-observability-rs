//! Observability building blocks for Axum services: health checks and the
//! `/live` and `/ready` router that serves them, and structured JSON logging
//! with optional OTLP export.
//!
//! The crate knows nothing about an application's configuration, error type
//! or state: a host passes plain values (a pool, a client, a URL) to the
//! checks it wants, implements [`health::router::HasHealthRegistry`] on its
//! state, and calls [`logs::configure_logging`] once at startup.
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
//! - `otel`: OTLP/HTTP log export from [`logs::configure_logging`].
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

pub mod error;
pub mod health;
pub mod logs;

pub use error::Error;
