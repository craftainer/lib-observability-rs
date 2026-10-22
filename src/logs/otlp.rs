//! The OTLP log layer behind the `otel` feature.

use crate::Error;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{ExporterBuildError, LogExporter};
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
const EXPORTER_CRATES: [&str; 17] = [
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
];

/// Whether an event may be exported: false for the exporter's own stack.
fn exportable(metadata: &Metadata<'_>) -> bool {
    exportable_target(metadata.target())
}

/// [`exportable`] for a bare target string.
fn exportable_target(target: &str) -> bool {
    let krate = target.split_once("::").map_or(target, |(krate, _)| krate);
    !EXPORTER_CRATES.contains(&krate)
}

/// Whether the environment asks for OTLP log export: a non-empty endpoint
/// (general or logs-specific), and neither `OTEL_SDK_DISABLED=true` nor
/// `OTEL_LOGS_EXPORTER` containing `none`, as in `lib-observability-py`.
fn export_requested(env: &dyn Fn(&str) -> Option<String>) -> bool {
    let non_empty = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    if !(non_empty("OTEL_EXPORTER_OTLP_ENDPOINT") || non_empty("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"))
    {
        return false;
    }
    if env("OTEL_SDK_DISABLED").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) {
        return false;
    }
    !env("OTEL_LOGS_EXPORTER").is_some_and(|v| {
        v.split(',')
            .any(|name| name.trim().eq_ignore_ascii_case("none"))
    })
}

/// The layer type [`log_layer`] builds: the OTLP bridge, filtered so it
/// ignores the exporter's own events.
pub(super) type LogLayer<S> = Filtered<
    OpenTelemetryTracingBridge<SdkLoggerProvider, SdkLogger>,
    FilterFn<fn(&Metadata<'_>) -> bool>,
    S,
>;

/// The layer and its provider, or `None` unless the environment asks for
/// OTLP log export. This never defaults to exporting to `localhost:4318` the
/// way the OTLP exporter's own fallback would, since that default exists for
/// a collector that, for most deployments, is not there.
///
/// # Errors
///
/// [`Error::ExporterBuild`] when export is requested and the exporter cannot
/// be built.
pub(super) fn log_layer<S>() -> Result<Option<(LogLayer<S>, SdkLoggerProvider)>, Error>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let requested = export_requested(&|name| std::env::var(name).ok());
    layer_from(requested, default_exporter)
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
/// down through [`super::LoggingGuard`].
fn layer_from<S>(
    requested: bool,
    exporter: fn() -> Result<LogExporter, ExporterBuildError>,
) -> Result<Option<(LogLayer<S>, SdkLoggerProvider)>, Error>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    if !requested {
        return Ok(None);
    }
    let exporter = exporter().map_err(|err| Error::ExporterBuild(err.to_string()))?;
    let provider = SdkLoggerProvider::builder()
        .with_log_processor(BatchLogProcessor::builder(exporter).build())
        .build();
    let filter: FilterFn<fn(&Metadata<'_>) -> bool> = FilterFn::new(exportable);
    let layer = OpenTelemetryTracingBridge::new(&provider).with_filter(filter);
    Ok(Some((layer, provider)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::LoggingGuard;
    use opentelemetry_otlp::WithExportConfig;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use tracing_subscriber::layer::SubscriberExt;

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

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn export_needs_a_non_empty_endpoint() {
        assert!(!export_requested(&env_of(&[])));
        assert!(!export_requested(&env_of(&[(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            ""
        )])));
        assert!(export_requested(&env_of(&[(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "http://c:4318"
        )])));
        assert!(export_requested(&env_of(&[(
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            "http://c:4318/v1/logs"
        )])));
    }

    #[test]
    fn export_can_be_switched_off() {
        let on = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318");
        assert!(!export_requested(&env_of(&[
            on,
            ("OTEL_SDK_DISABLED", " TRUE ")
        ])));
        assert!(export_requested(&env_of(&[
            on,
            ("OTEL_SDK_DISABLED", "false")
        ])));
        assert!(!export_requested(&env_of(&[
            on,
            ("OTEL_LOGS_EXPORTER", "console, None")
        ])));
        assert!(export_requested(&env_of(&[
            on,
            ("OTEL_LOGS_EXPORTER", "otlp")
        ])));
    }

    #[test]
    fn no_layer_without_a_request() {
        let layer = layer_from::<tracing_subscriber::Registry>(false, default_exporter);
        assert!(matches!(layer, Ok(None)));
    }

    #[test]
    fn the_environment_decides_in_log_layer() {
        // Normally no OTLP variable is set in the test environment; if one
        // is, a layer is expected.
        let requested = export_requested(&|name| std::env::var(name).ok());
        // Only built when the ambient environment asks for nothing, so the
        // test never starts an exporter thread aimed at a real collector.
        let layer = (!requested).then(|| log_layer::<tracing_subscriber::Registry>().unwrap());
        assert!(layer.is_none_or(|layer| layer.is_none()));
    }

    #[tokio::test]
    async fn the_default_exporter_builds_without_a_collector() {
        assert!(default_exporter().is_ok());
    }

    #[tokio::test]
    async fn a_layer_is_built_once_export_is_requested() {
        let layer = layer_from::<tracing_subscriber::Registry>(true, local_exporter);
        assert!(matches!(layer, Ok(Some(_))));
    }

    #[test]
    fn an_unbuildable_exporter_is_an_error_not_a_silent_no_op() {
        let err = layer_from::<tracing_subscriber::Registry>(true, failing_exporter)
            .err()
            .expect("an error");
        assert!(
            matches!(&err, Error::ExporterBuild(m) if m.contains("rejected for the test")),
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
        let guard = LoggingGuard {
            provider: Some(provider),
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
