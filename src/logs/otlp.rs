//! The OTLP log layer behind the `otel` feature.

use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{ExporterBuildError, LogExporter};
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLogger, SdkLoggerProvider};

/// The layer type [`log_layer`] builds.
pub(super) type LogLayer = OpenTelemetryTracingBridge<SdkLoggerProvider, SdkLogger>;

/// `None` unless the environment sets an OTLP logs endpoint. This never
/// defaults to exporting to `localhost:4318` the way the OTLP exporter's own
/// fallback would, since that default exists for a collector that, for most
/// deployments, is not there.
pub(super) fn log_layer() -> Option<LogLayer> {
    let endpoint_configured = std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some()
        || std::env::var_os("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT").is_some();
    layer_from(endpoint_configured, default_exporter)
}

/// The OTLP/HTTP log exporter, configured from the standard `OTEL_*`
/// environment variables. Building it opens no connection.
fn default_exporter() -> Result<LogExporter, ExporterBuildError> {
    LogExporter::builder().with_http().build()
}

/// The decision behind [`log_layer`], with the environment lookup and the
/// exporter construction passed in so tests need neither process-wide state
/// nor a collector. `exporter` is a function pointer, not a generic, so the
/// function has one instantiation and coverage counts it once.
///
/// A build failure prints one line to stderr and yields `None`: logging must
/// come up without the export rather than not at all.
///
/// The `SdkLoggerProvider` handle is dropped at the end of this function;
/// that is safe, because the `SdkLogger` the returned layer holds keeps its
/// own cloned (`Arc`-backed) reference to the same provider internals, so the
/// batch processor keeps running for the life of the process either way.
fn layer_from(
    endpoint_configured: bool,
    exporter: fn() -> Result<LogExporter, ExporterBuildError>,
) -> Option<LogLayer> {
    if !endpoint_configured {
        return None;
    }
    let exporter = match exporter() {
        Ok(exporter) => exporter,
        Err(err) => {
            eprintln!("OTLP log exporter setup failed, continuing without it: {err}");
            return None;
        }
    };
    let provider = SdkLoggerProvider::builder()
        .with_log_processor(BatchLogProcessor::builder(exporter).build())
        .build();
    Some(OpenTelemetryTracingBridge::new(&provider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_otlp::{Compression, WithExportConfig, WithHttpConfig};

    #[test]
    fn no_layer_without_an_endpoint() {
        let layer = layer_from(false, default_exporter);
        assert!(layer.is_none());
    }

    #[tokio::test]
    async fn the_default_exporter_builds_without_a_collector() {
        assert!(default_exporter().is_ok());
    }

    #[tokio::test]
    async fn a_layer_is_built_once_an_endpoint_is_configured() {
        let layer = layer_from(true, || {
            LogExporter::builder()
                .with_http()
                .with_endpoint("http://localhost:4318")
                .build()
        });
        assert!(layer.is_some());
    }

    #[test]
    fn an_unbuildable_exporter_fails_closed_rather_than_panicking() {
        // Gzip is not compiled in (no `gzip-http` feature), so the builder
        // fails before any network call is made.
        let layer = layer_from(true, || {
            LogExporter::builder()
                .with_http()
                .with_compression(Compression::Gzip)
                .build()
        });
        assert!(layer.is_none());
    }
}
