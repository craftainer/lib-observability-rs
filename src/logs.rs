//! Telemetry setup: structured JSON logging, and with the `otel` feature
//! OTLP export of logs, traces and metrics.
//!
//! [`configure_telemetry`] installs the process-wide `tracing` subscriber:
//! an `EnvFilter` (from `RUST_LOG`, `info` when unset or invalid) and one JSON
//! object per event on stdout, laid out as `lib-observability-py` does (see
//! `json` below). With the `otel` feature it adds, when the environment asks
//! for them,
//!
//! - an OTLP/HTTP log exporter fed by every event, in addition to (never
//!   instead of) the JSON output;
//! - an OpenTelemetry tracer provider and a layer that exports every
//!   `tracing` span, including [`ServerSpanLayer`](crate::otel::ServerSpanLayer)'s
//!   server spans; the JSON lines of events inside a span carry its
//!   `trace_id` and `span_id`;
//! - an OpenTelemetry meter provider, for the request-duration histogram and
//!   [`instrument_db`](crate::otel) pool gauges.
//!
//! Nothing is exported, and no exporter is built, unless
//! `OTEL_EXPORTER_OTLP_ENDPOINT` (or a signal's own
//! `OTEL_EXPORTER_OTLP_{LOGS,TRACES,METRICS}_ENDPOINT`) is non-empty; see
//! the [`otel`](crate::otel) module for the variables.
//!
//! # Shutdown
//!
//! The exporters batch on background threads, and Rust has no `atexit` hook,
//! so the last batch is lost unless the host flushes it.
//! [`configure_telemetry`] returns a [`Telemetry`]: keep it alive for the life
//! of `main`, call [`Telemetry::shutdown`] before exiting (or let it drop,
//! which does the same).
//!
//! Both block the calling thread until the export finishes or the SDK's
//! timeout passes (a dead collector costs that timeout). In an async `main`,
//! call `shutdown` through `tokio::task::spawn_blocking` (or after the
//! server has stopped), not from a worker thread you cannot spare.

mod json;
#[cfg(feature = "otel")]
mod otlp;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::Error;

/// The handle to the telemetry that [`configure_telemetry`] installed. Flushes
/// and shuts down every exporter (a no-op without the `otel` feature or when
/// nothing is exported). Dropping it shuts them down, so bind it to a
/// variable that lives until the process exits:
/// `let telemetry = configure_telemetry()?;`.
#[must_use = "dropping the handle immediately shuts the exporters down"]
pub struct Telemetry {
    #[cfg(feature = "otel")]
    providers: crate::otel::Providers,
}

impl Telemetry {
    /// Whether any signal is being exported (always `false` without the
    /// `otel` feature).
    #[must_use]
    pub fn is_exporting(&self) -> bool {
        #[cfg(feature = "otel")]
        return !self.providers.is_empty();
        #[cfg(not(feature = "otel"))]
        false
    }

    /// Exports everything buffered so far, blocking until done or the SDK's
    /// timeout. Every exporter is tried; the first failure is returned.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Flush`] when an export fails or times out.
    pub fn force_flush(&self) -> Result<(), Error> {
        #[cfg(feature = "otel")]
        self.providers.force_flush()?;
        Ok(())
    }

    /// Flushes and stops every exporter. Calling it again, or dropping the
    /// handle afterwards, is harmless.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Flush`] when a final export fails or times out.
    pub fn shutdown(&self) -> Result<(), Error> {
        #[cfg(feature = "otel")]
        self.providers.shutdown()?;
        Ok(())
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        // Best effort: an error here (including "already shut down") has
        // nowhere to go.
        let _ = self.shutdown();
    }
}

/// The handle [`configure_logging`] returns: a [`Telemetry`] under its old
/// name.
#[must_use = "dropping the guard immediately shuts the exporters down"]
pub struct LoggingGuard(Telemetry);

impl LoggingGuard {
    /// See [`Telemetry::force_flush`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Flush`] when an export fails or times out.
    pub fn force_flush(&self) -> Result<(), Error> {
        self.0.force_flush()
    }

    /// See [`Telemetry::shutdown`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Flush`] when a final export fails or times out.
    pub fn shutdown(&self) -> Result<(), Error> {
        self.0.shutdown()
    }
}

/// Installs the process-wide `tracing` subscriber and, with the `otel`
/// feature, the exporters the environment asks for. See the module docs.
///
/// Call it once, at process startup, before anything else logs, and keep the
/// returned [`Telemetry`].
///
/// A failure to build an OTLP exporter is an error, not a silent fallback
/// (`lib-observability-py` raises as well): nothing is installed, and the host
/// may call [`configure_telemetry_without_export`] to carry on with JSON only.
///
/// # Errors
///
/// - [`Error::SubscriberAlreadyInstalled`] when a global subscriber is
///   already installed; nothing is changed, and no exporter is built.
/// - [`Error::ExporterBuild`] when the environment asks for OTLP export of a
///   signal and its exporter cannot be built.
pub fn configure_telemetry() -> Result<Telemetry, Error> {
    install(true)
}

/// [`configure_telemetry`] without any OTLP export, whatever the environment
/// says. For a host that got [`Error::ExporterBuild`] and prefers JSON-only
/// logs to none.
///
/// # Errors
///
/// [`Error::SubscriberAlreadyInstalled`] when a global subscriber is already
/// installed.
pub fn configure_telemetry_without_export() -> Result<Telemetry, Error> {
    install(false)
}

/// The old name of [`configure_telemetry`]; the same call, so it also sets up
/// traces and metrics when the environment asks for them. Note that the log
/// lines are now laid out as `lib-observability-py` does (flat, with
/// redaction); see `docs/adrs/0005-otel-traces-metrics-log-correlation.md`.
///
/// # Errors
///
/// As [`configure_telemetry`].
#[deprecated(since = "0.3.0", note = "use `configure_telemetry`")]
pub fn configure_logging() -> Result<LoggingGuard, Error> {
    install(true).map(LoggingGuard)
}

/// The old name of [`configure_telemetry_without_export`].
///
/// # Errors
///
/// As [`configure_telemetry_without_export`].
#[deprecated(since = "0.3.0", note = "use `configure_telemetry_without_export`")]
pub fn configure_logging_without_export() -> Result<LoggingGuard, Error> {
    install(false).map(LoggingGuard)
}

/// Whether a subscriber other than the no-op default is visible here.
fn subscriber_installed() -> bool {
    tracing::dispatcher::get_default(|dispatch| !dispatch.is::<tracing::subscriber::NoSubscriber>())
}

fn install(export: bool) -> Result<Telemetry, Error> {
    // Before building anything: the exporters spawn background threads.
    if subscriber_installed() {
        return Err(Error::SubscriberAlreadyInstalled);
    }
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(json::JsonLayer::stdout());
    #[cfg(feature = "otel")]
    let (registry, providers) = {
        use crate::otel;
        let env = &otel::process_env;
        let resource = otel::shared_resource(env);
        let request = |signal| export && otel::signal_requested(env, signal);
        let (log_layer, logger) = otlp::log_layer(&resource, request(otel::Signal::Logs))?.unzip();
        let (want_traces, want_metrics) = (
            request(otel::Signal::Traces),
            request(otel::Signal::Metrics),
        );
        let tracer = otel::tracer_provider(want_traces, otel::default_span_exporter, &resource)?;
        let meter = otel::meter_provider(want_metrics, otel::default_metric_exporter, &resource)?;
        let trace_layer = tracer.as_ref().map(otel::trace_layer);
        let registry = registry.with(log_layer).with(trace_layer);
        (
            registry,
            otel::Providers {
                logger,
                tracer,
                meter,
            },
        )
    };
    #[cfg(not(feature = "otel"))]
    let _ = export;
    registry
        .try_init()
        .map_err(|_| Error::SubscriberAlreadyInstalled)?;
    #[cfg(feature = "otel")]
    crate::otel::register_globals(providers.tracer.as_ref(), providers.meter.as_ref());
    Ok(Telemetry {
        #[cfg(feature = "otel")]
        providers,
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
    #[expect(
        deprecated,
        reason = "the old names still have to refuse like the new ones"
    )]
    fn install_refuses_before_building_anything_when_a_subscriber_exists() {
        let subscriber = tracing_subscriber::registry();
        tracing::subscriber::with_default(subscriber, || {
            assert!(matches!(
                configure_telemetry(),
                Err(Error::SubscriberAlreadyInstalled)
            ));
            assert!(matches!(
                configure_telemetry_without_export(),
                Err(Error::SubscriberAlreadyInstalled)
            ));
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
    fn a_handle_without_exporters_flushes_and_shuts_down_cleanly() {
        let telemetry = Telemetry {
            #[cfg(feature = "otel")]
            providers: crate::otel::Providers::default(),
        };
        assert!(!telemetry.is_exporting());
        telemetry.force_flush().unwrap();
        telemetry.shutdown().unwrap();
        telemetry.shutdown().unwrap();
        let guard = LoggingGuard(telemetry);
        guard.force_flush().unwrap();
        guard.shutdown().unwrap();
    }
}
