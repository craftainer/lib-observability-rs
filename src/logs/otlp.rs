//! The OTLP log layer behind the `otel` feature.

use std::fmt::Debug;
use std::sync::OnceLock;
use std::time::SystemTime;

use crate::Error;
use crate::redact_keys::{REDACTED, is_secret_key, redact_text};
use opentelemetry::Key;
use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
use opentelemetry::trace::TraceContextExt as _;
use opentelemetry_otlp::{ExporterBuildError, LogExporter};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLogger, SdkLoggerProvider};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{FilterFn, Filtered};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

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

/// Collects an event's message and fields with credentials already
/// redacted: the message and string values by their text, any value by its
/// field name, error values with their whole `source()` chain.
#[derive(Default)]
struct Redacted {
    message: Option<String>,
    attributes: Vec<(String, AnyValue)>,
}

impl Redacted {
    fn text(&mut self, field: &Field, value: &str) {
        let name = field.name();
        if name == "message" {
            self.message = Some(redact_text(value));
        } else if is_secret_key(name) {
            self.attributes
                .push((name.to_owned(), AnyValue::from(REDACTED.to_owned())));
        } else {
            self.attributes
                .push((name.to_owned(), AnyValue::from(redact_text(value))));
        }
    }

    fn scalar(&mut self, field: &Field, value: AnyValue, shown: String) {
        let name = field.name();
        if name == "message" {
            self.message = Some(shown);
        } else if is_secret_key(name) {
            self.attributes
                .push((name.to_owned(), AnyValue::from(REDACTED.to_owned())));
        } else {
            self.attributes.push((name.to_owned(), value));
        }
    }
}

impl Visit for Redacted {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.text(field, &format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.text(field, value);
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let mut text = value.to_string();
        let mut source = value.source();
        while let Some(cause) = source {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        self.text(field, &text);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.scalar(field, AnyValue::Int(value), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let any =
            i64::try_from(value).map_or_else(|_| AnyValue::from(value.to_string()), AnyValue::Int);
        self.scalar(field, any, value.to_string());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.scalar(field, AnyValue::Double(value), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.scalar(field, AnyValue::Boolean(value), value.to_string());
    }
}

/// OpenTelemetry's severity for a `tracing` level.
fn severity(level: Level) -> Severity {
    match level {
        Level::TRACE => Severity::Trace,
        Level::DEBUG => Severity::Debug,
        Level::INFO => Severity::Info,
        Level::WARN => Severity::Warn,
        Level::ERROR => Severity::Error,
    }
}

/// The `tracing` layer that turns events into OTLP log records, in place of
/// `opentelemetry-appender-tracing`'s bridge: it builds each record itself so
/// the message, the fields and the error chain are redacted (one shared key
/// list, [`crate::redact_keys`]) before anything reaches the exporter. The SDK
/// cannot replace an attribute once it is on a record, hence the own layer.
pub(super) struct RedactingBridge {
    logger: SdkLogger,
    /// The subscriber this layer is part of, to find the OpenTelemetry
    /// context of the event's span (see `JsonLayer`).
    dispatch: OnceLock<tracing::dispatcher::WeakDispatch>,
}

impl RedactingBridge {
    /// A bridge emitting through `provider`.
    pub(super) fn new(provider: &SdkLoggerProvider) -> Self {
        Self {
            logger: provider.logger(""),
            dispatch: OnceLock::new(),
        }
    }
}

impl<S> Layer<S> for RedactingBridge
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_register_dispatch(&self, subscriber: &tracing::Dispatch) {
        let _ = self.dispatch.set(subscriber.downgrade());
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut fields = Redacted::default();
        event.record(&mut fields);
        let mut record = self.logger.create_log_record();
        record.set_target(metadata.target());
        record.set_event_name(metadata.name());
        record.set_severity_number(severity(*metadata.level()));
        record.set_severity_text(metadata.level().as_str());
        let now = SystemTime::now();
        record.set_timestamp(now);
        record.set_observed_timestamp(now);
        if let Some(message) = fields.message {
            record.set_body(AnyValue::from(message));
        }
        for (key, value) in fields.attributes {
            record.add_attribute(Key::from(key), value);
        }
        let otel = ctx
            .event_span(event)
            .map(|span| span.id())
            .zip(
                self.dispatch
                    .get()
                    .and_then(tracing::dispatcher::WeakDispatch::upgrade),
            )
            .and_then(|(id, dispatch)| tracing_opentelemetry::get_otel_context(&id, &dispatch));
        if let Some(context) = otel {
            let span = context.span();
            let span_context = span.span_context();
            if span_context.is_valid() {
                record.set_trace_context(
                    span_context.trace_id(),
                    span_context.span_id(),
                    Some(span_context.trace_flags()),
                );
            }
        }
        self.logger.emit(record);
    }
}

/// The layer type [`log_layer`] builds: the redacting OTLP bridge, filtered
/// so it ignores the exporter's own events.
pub(super) type LogLayer<S> = Filtered<RedactingBridge, FilterFn<fn(&Metadata<'_>) -> bool>, S>;

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
    let layer = RedactingBridge::new(&provider).with_filter(filter);
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
        let layer = RedactingBridge::new(&provider).with_filter(filter);
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

    /// An error with a cause, to test the `source()` chain.
    #[derive(Debug)]
    struct Outer(std::io::Error);

    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "connect failed")
        }
    }

    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    /// Emits through the redacting bridge into memory and returns the records.
    fn emitted(body: impl FnOnce()) -> Vec<opentelemetry_sdk::logs::SdkLogRecord> {
        use opentelemetry_sdk::logs::{InMemoryLogExporter, SimpleLogProcessor};
        let memory = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_log_processor(SimpleLogProcessor::new(memory.clone()))
            .build();
        let subscriber = tracing_subscriber::registry().with(RedactingBridge::new(&provider));
        tracing::subscriber::with_default(subscriber, body);
        memory
            .get_emitted_logs()
            .unwrap()
            .into_iter()
            .map(|log| log.record)
            .collect()
    }

    fn attribute(record: &opentelemetry_sdk::logs::SdkLogRecord, name: &str) -> Option<AnyValue> {
        record
            .attributes_iter()
            .find(|(key, _)| key.as_str() == name)
            .map(|(_, value)| value.clone())
    }

    fn text(value: &str) -> AnyValue {
        AnyValue::String(value.to_owned().into())
    }

    #[test]
    fn the_message_the_fields_and_the_error_chain_are_redacted() {
        let error = Outer(std::io::Error::other("postgres://app:pw@db/x refused"));
        let error: &dyn std::error::Error = &error;
        let records = emitted(|| {
            tracing::warn!(
                api_key = "k1",
                session_id = 7_i64,
                note = "retry token=abc now",
                error = error,
                attempts = 3_i64,
                big = u64::MAX,
                small = 5_u64,
                ratio = 0.5,
                flag = true,
                "login password=hunter2"
            );
        });
        let record = &records[0];
        assert_eq!(
            record.body(),
            Some(&AnyValue::String("login password=[REDACTED]".into()))
        );
        assert_eq!(attribute(record, "api_key"), Some(text("[REDACTED]")));
        assert_eq!(attribute(record, "session_id"), Some(text("[REDACTED]")));
        assert_eq!(
            attribute(record, "note"),
            Some(text("retry token=[REDACTED] now"))
        );
        assert_eq!(
            attribute(record, "error"),
            Some(text("connect failed: postgres://[REDACTED]@db/x refused"))
        );
        assert_eq!(attribute(record, "attempts"), Some(AnyValue::Int(3)));
        assert_eq!(attribute(record, "small"), Some(AnyValue::Int(5)));
        assert_eq!(attribute(record, "big"), Some(text(&u64::MAX.to_string())));
        assert_eq!(attribute(record, "ratio"), Some(AnyValue::Double(0.5)));
        assert_eq!(attribute(record, "flag"), Some(AnyValue::Boolean(true)));
        assert_eq!(record.severity_number(), Some(Severity::Warn));
        assert!(record.trace_context().is_none());
    }

    #[test]
    fn a_non_string_message_and_every_level_are_recorded() {
        let records = emitted(|| {
            tracing::trace!(message = 5_i64);
            tracing::debug!("d");
            tracing::info!("i");
            tracing::error!("e");
        });
        assert_eq!(records[0].body(), Some(&AnyValue::String("5".into())));
        let levels: Vec<_> = records.iter().filter_map(opentelemetry_sdk::logs::SdkLogRecord::severity_number).collect();
        assert_eq!(
            levels,
            [
                Severity::Trace,
                Severity::Debug,
                Severity::Info,
                Severity::Error
            ]
        );
        let secret_message = emitted(|| tracing::info!(message = 5_i64, token = 1_i64));
        assert_eq!(
            attribute(&secret_message[0], "token"),
            Some(text("[REDACTED]"))
        );
    }

    #[test]
    fn a_record_inside_a_span_carries_its_trace_context() {
        use crate::otel::trace_layer;
        use opentelemetry_sdk::logs::{InMemoryLogExporter, SimpleLogProcessor};
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

        let spans = InMemorySpanExporter::default();
        let tracer = SdkTracerProvider::builder()
            .with_simple_exporter(spans.clone())
            .build();
        let memory = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_log_processor(SimpleLogProcessor::new(memory.clone()))
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(trace_layer(&tracer))
            .with(RedactingBridge::new(&provider));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("outside");
            tracing::info_span!("work").in_scope(|| tracing::info!("inside"));
        });
        let finished = spans.get_finished_spans().unwrap();
        let logs = memory.get_emitted_logs().unwrap();
        let outside = &logs[0].record;
        let inside = &logs[1].record;
        assert!(outside.trace_context().is_none());
        let context = inside.trace_context().expect("trace context");
        assert_eq!(context.trace_id, finished[0].span_context.trace_id());
        assert_eq!(context.span_id, finished[0].span_context.span_id());
        assert!(finished[0].span_context.is_valid());
    }
}
