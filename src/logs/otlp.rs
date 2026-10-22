//! The OTLP log layer behind the `otel` feature.

use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{ExporterBuildError, LogExporter};
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLogger, SdkLoggerProvider};
use tracing::Metadata;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{FilterFn, Filtered};

/// Targets whose events must never reach the exporter: its own HTTP stack
/// and the OpenTelemetry crates log while exporting, so forwarding them
/// would feed every export batch back into the next one.
const EXPORTER_TARGETS: [&str; 7] = [
    "hyper",
    "hyper_util",
    "h2",
    "reqwest",
    "rustls",
    "tonic",
    "opentelemetry",
];

/// Whether an event may be exported: false for the exporter's own stack.
fn exportable(metadata: &Metadata<'_>) -> bool {
    exportable_target(metadata.target())
}

/// [`exportable`] for a bare target string.
fn exportable_target(target: &str) -> bool {
    !EXPORTER_TARGETS.iter().any(|skip| {
        target
            .strip_prefix(skip)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::") || rest.starts_with('_'))
    })
}

/// The layer type [`log_layer`] builds: the OTLP bridge, filtered so it
/// ignores the exporter's own events.
pub(super) type LogLayer<S> = Filtered<
    OpenTelemetryTracingBridge<SdkLoggerProvider, SdkLogger>,
    FilterFn<fn(&Metadata<'_>) -> bool>,
    S,
>;

/// `None` unless the environment sets an OTLP logs endpoint. This never
/// defaults to exporting to `localhost:4318` the way the OTLP exporter's own
/// fallback would, since that default exists for a collector that, for most
/// deployments, is not there.
pub(super) fn log_layer<S>() -> Option<LogLayer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
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
fn layer_from<S>(
    endpoint_configured: bool,
    exporter: fn() -> Result<LogExporter, ExporterBuildError>,
) -> Option<LogLayer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
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
    let filter: FilterFn<fn(&Metadata<'_>) -> bool> = FilterFn::new(exportable);
    Some(OpenTelemetryTracingBridge::new(&provider).with_filter(filter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_otlp::{Compression, WithExportConfig, WithHttpConfig};

    #[test]
    fn no_layer_without_an_endpoint() {
        let layer = layer_from::<tracing_subscriber::Registry>(false, default_exporter);
        assert!(layer.is_none());
    }

    #[tokio::test]
    async fn the_default_exporter_builds_without_a_collector() {
        assert!(default_exporter().is_ok());
    }

    #[tokio::test]
    async fn a_layer_is_built_once_an_endpoint_is_configured() {
        let layer = layer_from::<tracing_subscriber::Registry>(true, || {
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
        let layer = layer_from::<tracing_subscriber::Registry>(true, || {
            LogExporter::builder()
                .with_http()
                .with_compression(Compression::Gzip)
                .build()
        });
        assert!(layer.is_none());
    }

    #[test]
    fn the_exporters_own_stack_is_not_exportable() {
        for (target, expected) in [
            ("hyper::client", false),
            ("hyper", false),
            ("h2::codec", false),
            ("reqwest::connect", false),
            ("opentelemetry_sdk::logs", false),
            ("hyper_util::client", false),
            ("myapp::hyperdrive", true),
            ("hypertext", true),
            ("myapp", true),
        ] {
            assert_eq!(exportable_target(target), expected, "{target}");
        }
    }

    #[tokio::test]
    async fn the_filter_runs_on_live_events() {
        use tracing_subscriber::layer::SubscriberExt;
        let layer = layer_from::<tracing_subscriber::Registry>(true, || {
            LogExporter::builder()
                .with_http()
                .with_endpoint("http://localhost:4318")
                .build()
        })
        .expect("layer");
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "hyper::client", "from the exporter's stack");
            tracing::info!(target: "myapp", "from the application");
        });
    }
}
