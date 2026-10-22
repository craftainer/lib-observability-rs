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
//!
//! # Examples
//!
//! ```
//! use craftainer_observability::health::checks::MockHealthCheck;
//! use craftainer_observability::health::HealthRegistry;
//!
//! let mut registry = HealthRegistry::new();
//! registry.register(Box::new(MockHealthCheck::new("database")));
//! ```

pub mod error;
pub mod health;
pub mod logs;

pub use error::Error;
