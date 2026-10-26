//! The OTLP log layer behind the `otel` feature.

use crate::Error;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{ExporterBuildError, LogExporter};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLogger, SdkLoggerProvider};
use tracing::Metadata;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{FilterFn, Filtered};

/// Crates whose events must never reach the exporter: its own HTTP stack and
/// the OpenTelemetry crates log while exporting, so forwarding them would
/// feed every export batch back into the next one. Matched against the first
/// path segment of an event's target, exactly (`h2` matches `h2::codec`, not
/// `h2_foo`), so every crate name is listed in full. The exporter is
/// `http-proto` over `reqwest` + `rustls` (no `tonic`).
const EXPORTER_CRATES: [&str; 21] = [
    "hyper",
    "hyper_util",
    "hyper_rustls",
    "h2",
    "reqwest",
    "rustls",
    "tokio_rustls",
    "opentelemetry",
    "opentelemetry_sdk",
    "opentelemetry_otlp",
    "opentelemetry_http",
    "opentelemetry_proto",
    "opentelemetry_appender_tracing",
    "tokio_util",
    "http_body_util",
    "rustls_platform_verifier",
    "rustls_native_certs",
    "rustls_pki_types",
    "webpki",
    "want",
    "http_body",
];

/// Whether an event may be exported: false for the exporter's own stack.
pub(crate) fn exportable(metadata: &Metadata<'_>) -> bool {
    exportable_target(metadata.target())
}

/// [`exportable`] for a bare target string.
fn exportable_target(target: &str) -> bool {
    let krate = target.split_once("::").map_or(target, |(krate, _)| krate);
    !EXPORTER_CRATES.contains(&krate)
}

/// The layer type [`log_layer`] builds: the OTLP bridge, filtered so it
/// ignores the exporter's own events.
pub(super) type LogLayer<S> = Filtered<
    OpenTelemetryTracingBridge<SdkLoggerProvider, SdkLogger>,
    FilterFn<fn(&Metadata<'_>) -> bool>,
    S,
>;

/// The layer and its provider, or `None` unless `requested`. This never defaults to exporting to `localhost:4318` the
/// way the OTLP exporter's own fallback would, since that default exists for
/// a collector that, for most deployments, is not there.
///
/// # Errors
///
/// [`Error::ExporterBuild`] when export is requested and the exporter cannot
/// be built.
pub(super) fn log_layer<S>(
    resource: &Resource,
    requested: bool,
) -> Result<Option<(LogLayer<S>, SdkLoggerProvider)>, Error>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    // Whether the environment asks for it is decided by the caller.
    layer_from(requested, default_exporter, resource)
}

/// The OTLP/HTTP log exporter, configured from the standard `OTEL_*`
/// environment variables. Building it opens no connection.
fn default_exporter() -> Result<LogExporter, ExporterBuildError> {
    LogExporter::builder().with_http().build()
}

/// The decision behind [`log_layer`], with the environment decision and the
/// exporter construction passed in so tests need neither process-wide state
/// nor a collector. `exporter` is a function pointer, not a generic, so the
/// function has one instantiation and coverage counts it once.
///
/// The provider is returned (not dropped) so the host can flush and shut it
/// down through [`super::Telemetry`].
fn layer_from<S>(
    requested: bool,
    exporter: fn() -> Result<LogExporter, ExporterBuildError>,
    resource: &Resource,
) -> Result<Option<(LogLayer<S>, SdkLoggerProvider)>, Error>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    if !requested {
        return Ok(None);
    }
    let exporter = exporter().map_err(|err| Error::ExporterBuild(format!("logs: {err}")))?;
    let provider = SdkLoggerProvider::builder()
        .with_resource(resource.clone())
        .with_log_processor(BatchLogProcessor::builder(exporter).build())
        .build();
    let filter: FilterFn<fn(&Metadata<'_>) -> bool> = FilterFn::new(exportable);
    let layer = OpenTelemetryTracingBridge::new(&provider).with_filter(filter);
    Ok(Some((layer, provider)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::Telemetry;
    use crate::otel::{Providers, shared_resource};
    use opentelemetry_otlp::WithExportConfig;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use tracing_subscriber::layer::SubscriberExt;

    fn resource() -> Resource {
        shared_resource(&|_| None)
    }

    fn local_exporter() -> Result<LogExporter, ExporterBuildError> {
        LogExporter::builder()
            .with_http()
            .with_endpoint("http://localhost:4318")
            .build()
    }

    /// A build failure that does not depend on which crate features are
    /// compiled in (unlike, say, requesting gzip).
    fn failing_exporter() -> Result<LogExporter, ExporterBuildError> {
        Err(ExporterBuildError::InvalidConfig {
            name: "endpoint".to_string(),
            reason: "rejected for the test".to_string(),
        })
    }

    #[test]
    fn no_layer_without_a_request() {
        let layer =
            layer_from::<tracing_subscriber::Registry>(false, default_exporter, &resource());
        assert!(matches!(layer, Ok(None)));
    }

    #[tokio::test]
    async fn log_layer_builds_the_default_exporter() {
        let layer = log_layer::<tracing_subscriber::Registry>(&resource(), true);
        assert!(matches!(layer, Ok(Some(_))));
        let layer = log_layer::<tracing_subscriber::Registry>(&resource(), false);
        assert!(matches!(layer, Ok(None)));
    }

    #[tokio::test]
    async fn the_default_exporter_builds_without_a_collector() {
        assert!(default_exporter().is_ok());
    }

    #[tokio::test]
    async fn a_layer_is_built_once_export_is_requested() {
        let layer = layer_from::<tracing_subscriber::Registry>(true, local_exporter, &resource());
        assert!(matches!(layer, Ok(Some(_))));
    }

    #[test]
    fn an_unbuildable_exporter_is_an_error_not_a_silent_no_op() {
        let err = layer_from::<tracing_subscriber::Registry>(true, failing_exporter, &resource())
            .err()
            .expect("an error");
        assert!(
            matches!(&err, Error::ExporterBuild(m) if m.starts_with("logs:") && m.contains("rejected for the test")),
            "{err}"
        );
    }

    #[test]
    fn the_exporters_own_stack_is_not_exportable() {
        for (target, expected) in [
            ("hyper::client", false),
            ("hyper", false),
            ("h2::codec", false),
            ("h2", false),
            ("reqwest::connect", false),
            ("opentelemetry_sdk::logs", false),
            ("opentelemetry_otlp", false),
            ("hyper_util::client", false),
            ("hyper_rustls::connector", false),
            ("tokio_rustls::client", false),
            // Exact crate names only: a look-alike is the application's.
            ("h2_foo", true),
            ("h2_foo::bar", true),
            ("hyperdrive", true),
            ("myapp::hyperdrive", true),
            ("hypertext", true),
            ("opentelemetry_custom", true),
            ("tonic::transport", true),
            ("myapp", true),
        ] {
            assert_eq!(exportable_target(target), expected, "{target}");
        }
    }

    /// Counts the events that reach it.
    struct Count(Arc<AtomicUsize>);

    impl<S: tracing::Subscriber> Layer<S> for Count {
        fn on_event(
            &self,
            _event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn the_filter_drops_live_events_from_the_exporters_stack() {
        let seen = Arc::new(AtomicUsize::new(0));
        let filter: FilterFn<fn(&Metadata<'_>) -> bool> = FilterFn::new(exportable);
        let subscriber =
            tracing_subscriber::registry().with(Count(seen.clone()).with_filter(filter));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "hyper::client", "from the exporter's stack");
            tracing::info!(target: "h2", "from the exporter's stack");
            tracing::info!(target: "h2_foo", "from the application");
            tracing::info!(target: "myapp", "from the application");
        });
        assert_eq!(seen.load(Ordering::SeqCst), 2);
    }

    /// The `Content-Length` of a request whose headers are complete.
    fn body_length(data: &[u8]) -> Option<usize> {
        let text = String::from_utf8_lossy(data);
        text.find("\r\n\r\n")?;
        Some(
            text.lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0),
        )
    }

    /// Whether `data` holds a whole request (headers and body).
    fn request_complete(data: &[u8]) -> bool {
        let head_end = String::from_utf8_lossy(data).find("\r\n\r\n");
        body_length(data)
            .zip(head_end)
            .is_some_and(|(length, end)| data.len() >= end + 4 + length)
    }

    /// Accepts one connection, reads one HTTP request, answers 200, and sends
    /// back the request line and the body length.
    fn one_shot_collector() -> (String, mpsc::Receiver<(String, usize)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/logs", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            while !request_complete(&data) {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0, "the client closed before sending a whole request");
                data.extend_from_slice(&buf[..n]);
            }
            let content_length = body_length(&data).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .unwrap();
            let line = String::from_utf8_lossy(&data)
                .lines()
                .next()
                .unwrap()
                .to_string();
            tx.send((line, content_length)).unwrap();
        });
        (endpoint, rx)
    }

    #[test]
    fn the_guard_flushes_the_last_batch_to_the_collector() {
        let (endpoint, received) = one_shot_collector();
        let exporter = LogExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .build()
            .unwrap();
        let provider = SdkLoggerProvider::builder()
            .with_log_processor(BatchLogProcessor::builder(exporter).build())
            .build();
        let filter: FilterFn<fn(&Metadata<'_>) -> bool> = FilterFn::new(exportable);
        let layer = OpenTelemetryTracingBridge::new(&provider).with_filter(filter);
        let guard = Telemetry {
            providers: Providers {
                logger: Some(provider),
                ..Providers::default()
            },
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "myapp", "the last batch");
        });
        guard.force_flush().expect("flush");
        let (line, length) = received
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the collector got the batch");
        assert!(line.starts_with("POST /v1/logs"), "{line}");
        assert!(length > 0);
        guard.shutdown().expect("shutdown");
        guard.shutdown().expect("a second shutdown is harmless");
        assert!(guard.force_flush().is_err());
    }
}
