//! Structured JSON logging, the only unconditional log output shape.
//!
//! [`configure_logging`] attaches a JSON-formatting subscriber to every
//! `tracing` call (including `tower-http`'s `TraceLayer` request logs). With
//! the `otel` feature it also forwards every event to an OTLP log collector
//! over HTTP, in addition to (never instead of) the JSON output. The export
//! is configured through OpenTelemetry's own environment variables, the
//! convention every OTLP SDK uses, rather than an application setting for
//! something already standardized:
//!
//! - `OTEL_EXPORTER_OTLP_ENDPOINT`, or the logs-specific
//!   `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`, must be non-empty to enable export
//!   (`lib-observability-py` reads only the first);
//! - `OTEL_SDK_DISABLED=true` or `OTEL_LOGS_EXPORTER=none` switches it off.
//!
//! # Shutdown
//!
//! The OTLP exporter batches records on a background thread, and Rust has no
//! `atexit` hook, so the last batch is lost unless the host flushes it.
//! [`configure_logging`] returns a [`LoggingGuard`]: keep it alive for the
//! life of `main`, call [`LoggingGuard::shutdown`] before exiting (or let it
//! drop, which does the same).

#[cfg(feature = "otel")]
mod otlp;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::Error;

/// The handle to the logging that [`configure_logging`] installed. Flushes
/// and shuts down the OTLP export (a no-op without the `otel` feature or
/// when no export is configured). Dropping it shuts the export down, so bind
/// it to a variable that lives until the process exits:
/// `let _logging = configure_logging()?;`.
#[must_use = "dropping the guard immediately shuts the log export down"]
pub struct LoggingGuard {
    #[cfg(feature = "otel")]
    provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl LoggingGuard {
    /// Exports every record buffered so far, blocking until done or the
    /// SDK's timeout.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Flush`] when the export fails or times out.
    pub fn force_flush(&self) -> Result<(), Error> {
        #[cfg(feature = "otel")]
        if let Some(provider) = &self.provider {
            provider
                .force_flush()
                .map_err(|err| Error::Flush(err.to_string()))?;
        }
        Ok(())
    }

    /// Flushes and stops the export. Calling it again, or dropping the guard
    /// afterwards, is harmless.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Flush`] when the final export fails or times out.
    pub fn shutdown(&self) -> Result<(), Error> {
        #[cfg(feature = "otel")]
        if let Some(provider) = &self.provider {
            provider
                .shutdown()
                .map_err(|err| Error::Flush(err.to_string()))?;
        }
        Ok(())
    }
}

impl Drop for LoggingGuard {
    fn drop(&mut self) {
        // Best effort: an error here (including "already shut down") has
        // nowhere to go.
        let _ = self.shutdown();
    }
}

/// Installs the process-wide `tracing` subscriber: an `EnvFilter` (from
/// `RUST_LOG`, `info` when unset or invalid), JSON lines on stdout, and with
/// the `otel` feature the OTLP log layer when the environment asks for one.
///
/// Call it once, at process startup, before anything else logs, and keep the
/// returned [`LoggingGuard`] (see the module docs).
///
/// A failure to build the OTLP exporter is an error, not a silent fallback
/// (`lib-observability-py` raises as well): nothing is installed, and the host
/// may call [`configure_logging_without_export`] to carry on with JSON only.
///
/// # Errors
///
/// - [`Error::SubscriberAlreadyInstalled`] when a global subscriber is
///   already installed; nothing is changed, and no exporter is built.
/// - [`Error::ExporterBuild`] when the environment asks for OTLP export and
///   the exporter cannot be built.
pub fn configure_logging() -> Result<LoggingGuard, Error> {
    install(true)
}

/// [`configure_logging`] without the OTLP export, whatever the environment
/// says. For a host that got [`Error::ExporterBuild`] and prefers JSON-only
/// logs to none.
///
/// # Errors
///
/// [`Error::SubscriberAlreadyInstalled`] when a global subscriber is already
/// installed.
pub fn configure_logging_without_export() -> Result<LoggingGuard, Error> {
    install(false)
}

/// Whether a subscriber other than the no-op default is visible here.
fn subscriber_installed() -> bool {
    tracing::dispatcher::get_default(|dispatch| !dispatch.is::<tracing::subscriber::NoSubscriber>())
}

fn install(export: bool) -> Result<LoggingGuard, Error> {
    // Before building anything: the OTLP provider spawns a background thread.
    if subscriber_installed() {
        return Err(Error::SubscriberAlreadyInstalled);
    }
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().json());
    #[cfg(feature = "otel")]
    let (otlp_layer, provider) = export.then(otlp::log_layer).transpose()?.flatten().unzip();
    #[cfg(not(feature = "otel"))]
    let _ = export;
    #[cfg(feature = "otel")]
    let registry = registry.with(otlp_layer);
    registry
        .try_init()
        .map_err(|_| Error::SubscriberAlreadyInstalled)?;
    Ok(LoggingGuard {
        #[cfg(feature = "otel")]
        provider,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scoped_subscriber_counts_as_installed() {
        assert!(!subscriber_installed());
        let subscriber = tracing_subscriber::registry();
        tracing::subscriber::with_default(subscriber, || assert!(subscriber_installed()));
    }

    #[test]
    fn install_refuses_before_building_anything_when_a_subscriber_exists() {
        let subscriber = tracing_subscriber::registry();
        tracing::subscriber::with_default(subscriber, || {
            assert!(matches!(
                configure_logging(),
                Err(Error::SubscriberAlreadyInstalled)
            ));
            assert!(matches!(
                configure_logging_without_export(),
                Err(Error::SubscriberAlreadyInstalled)
            ));
        });
    }

    #[test]
    fn a_guard_without_an_export_flushes_and_shuts_down_cleanly() {
        let guard = LoggingGuard {
            #[cfg(feature = "otel")]
            provider: None,
        };
        guard.force_flush().unwrap();
        guard.shutdown().unwrap();
        guard.shutdown().unwrap();
    }
}
