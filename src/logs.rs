//! Structured JSON logging, the only unconditional log output shape.
//!
//! [`configure_logging`] attaches a JSON-formatting subscriber to every
//! `tracing` call (including `tower-http`'s `TraceLayer` request logs). With
//! the `otel` feature it also forwards every event to an OTLP log collector
//! over HTTP, in addition to (never instead of) the JSON output. The export
//! is configured through OpenTelemetry's own environment variables
//! (`OTEL_EXPORTER_OTLP_ENDPOINT`, or the logs-specific
//! `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`), the convention every OTLP SDK uses,
//! rather than an application setting for something already standardized.

#[cfg(feature = "otel")]
mod otlp;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::Error;

/// Installs the process-wide `tracing` subscriber: an `EnvFilter` (from
/// `RUST_LOG`, `info` when unset or invalid), JSON lines on stdout, and with
/// the `otel` feature the OTLP log layer when the environment asks for one.
///
/// Call it once, at process startup, before anything else logs.
///
/// # Errors
///
/// Returns [`Error::SubscriberAlreadyInstalled`] when a global subscriber is
/// already installed; nothing is changed in that case.
pub fn configure_logging() -> Result<(), Error> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().json());
    #[cfg(feature = "otel")]
    let registry = registry.with(otlp::log_layer());
    registry
        .try_init()
        .map_err(|_| Error::SubscriberAlreadyInstalled)
}
