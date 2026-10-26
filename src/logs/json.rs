//! The JSON log line: one flat object per event, laid out as
//! `lib-observability-py`'s formatter does.
//!
//! ```text
//! {"timestamp":"2026-10-26T12:00:00.000001+00:00","level":"INFO","logger":"myapp::orders",
//!  "message":"created","order_id":7,"trace_id":"...","span_id":"...","request_id":"..."}
//! ```
//!
//! - `timestamp`, `level` (`TRACE` to `ERROR`, tracing's names), `logger` (the
//!   event's target) and `message` are always there.
//! - Every other event field is a top-level key. A field whose name contains
//!   `token`, `password`, `secret`, `authorization`, `claims` or `credential`
//!   (any case) has its value replaced by `[REDACTED]`. A field named like one
//!   of the keys of this list or below (`timestamp`, `level`, `logger`,
//!   `trace_id`, `span_id`, `request_id`) is dropped: the line's own value wins.
//! - `trace_id` and `span_id` (hex) are there when the event is inside a valid
//!   OpenTelemetry span (the `otel` feature, with the trace layer installed by
//!   [`configure_telemetry`](super::configure_telemetry)).
//! - `request_id` is there when an enclosing span has a `request_id` field, as
//!   the one [`RequestIdLayer`](crate::request_id::RequestIdLayer) opens.

use std::fmt::Debug;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Record};
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// Field-name fragments whose values are never logged (compared lowercase).
const REDACTED_KEY_PARTS: [&str; 6] = [
    "token",
    "password",
    "secret",
    "authorization",
    "claims",
    "credential",
];

/// The placeholder that replaces a redacted value.
const REDACTED: &str = "[REDACTED]";

/// Keys the line sets itself; an event field of this name is dropped.
const RESERVED_KEYS: [&str; 7] = [
    "timestamp",
    "level",
    "logger",
    "message",
    "trace_id",
    "span_id",
    "request_id",
];

/// The value of a span's `request_id` field, kept in the span's extensions.
struct RequestIdValue(String);

/// The `tracing` layer that writes the JSON lines to `W`.
pub(super) struct JsonLayer<W> {
    make_writer: W,
    /// The subscriber this layer is part of, to look up a span's
    /// OpenTelemetry context from inside `on_event` (where
    /// `Span::current()` is unavailable: `tracing` hides the dispatcher from
    /// code that runs inside it).
    #[cfg(feature = "otel")]
    dispatch: std::sync::OnceLock<tracing::dispatcher::WeakDispatch>,
}

impl JsonLayer<fn() -> std::io::Stdout> {
    /// A layer writing to standard output.
    pub(super) fn stdout() -> Self {
        Self::new(std::io::stdout as fn() -> std::io::Stdout)
    }
}

impl<W> JsonLayer<W> {
    /// A layer writing to whatever `make_writer` makes.
    pub(super) fn new(make_writer: W) -> Self {
        Self {
            make_writer,
            #[cfg(feature = "otel")]
            dispatch: std::sync::OnceLock::new(),
        }
    }
}

/// Whether the value under `key` must not be logged.
fn is_redacted(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    REDACTED_KEY_PARTS.iter().any(|part| lowered.contains(part))
}

/// Collects an event's fields in order: the message and the (redacted) rest.
#[derive(Default)]
struct Fields {
    message: String,
    extras: Vec<(String, Value)>,
}

impl Fields {
    fn put(&mut self, field: &Field, value: Value) {
        let name = field.name();
        if name == "message" {
            // A non-string message value is rendered as text.
            self.message = match value {
                Value::String(text) => text,
                other => other.to_string(),
            };
        } else if !RESERVED_KEYS.contains(&name) {
            let value = if is_redacted(name) {
                Value::String(REDACTED.to_owned())
            } else {
                value
            };
            self.extras.push((name.to_owned(), value));
        }
    }
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.put(field, Value::String(format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, Value::String(value.to_owned()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field, Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, Value::from(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        // NaN and infinity are not JSON numbers: they become null.
        self.put(field, Value::from(value));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.put(field, Value::String(value.to_string()));
    }
}

/// Finds a span's `request_id` field.
#[derive(Default)]
struct RequestIdField(Option<String>);

impl Visit for RequestIdField {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "request_id" {
            self.0 = Some(format!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "request_id" {
            self.0 = Some(value.to_owned());
        }
    }
}

/// A JSON object that keeps its keys in insertion order.
struct Ordered<'a>(&'a [(String, Value)]);

impl Serialize for Ordered<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// `time` as RFC 3339 in UTC with microseconds and a `+00:00` offset, the
/// shape of Python's `datetime.isoformat()`.
fn rfc3339(time: SystemTime) -> String {
    // A clock before the epoch is reported as the epoch itself.
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = i64::try_from(since.as_secs()).unwrap_or(i64::MAX / 2);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}+00:00",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60,
        since.subsec_micros()
    )
}

impl<W> JsonLayer<W> {
    /// The trace and span IDs of the OpenTelemetry span behind tracing span `id`,
    /// when valid.
    #[cfg(feature = "otel")]
    fn trace_ids(&self, id: Option<&Id>) -> Option<(String, String)> {
        use opentelemetry::trace::TraceContextExt as _;
        let dispatch = self.dispatch.get()?.upgrade()?;
        let context = tracing_opentelemetry::get_otel_context(id?, &dispatch)?;
        let span = context.span();
        let span_context = span.span_context();
        span_context.is_valid().then(|| {
            (
                span_context.trace_id().to_string(),
                span_context.span_id().to_string(),
            )
        })
    }

    /// Without the `otel` feature there is no trace context.
    #[cfg(not(feature = "otel"))]
    fn trace_ids(&self, id: Option<&Id>) -> Option<(String, String)> {
        let _ = (self, id);
        None
    }
}

impl<S, W> Layer<S> for JsonLayer<W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'a> MakeWriter<'a> + 'static,
{
    #[cfg(feature = "otel")]
    fn on_register_dispatch(&self, subscriber: &tracing::Dispatch) {
        let _ = self.dispatch.set(subscriber.downgrade());
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut field = RequestIdField::default();
        attrs.record(&mut field);
        if let (Some(request_id), Some(span)) = (field.0, ctx.span(id)) {
            span.extensions_mut().insert(RequestIdValue(request_id));
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let mut field = RequestIdField::default();
        values.record(&mut field);
        if let (Some(request_id), Some(span)) = (field.0, ctx.span(id)) {
            span.extensions_mut().replace(RequestIdValue(request_id));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let metadata = event.metadata();
        let mut line = vec![
            (
                "timestamp".to_owned(),
                Value::String(rfc3339(SystemTime::now())),
            ),
            ("level".to_owned(), Value::from(metadata.level().as_str())),
            ("logger".to_owned(), Value::from(metadata.target())),
            ("message".to_owned(), Value::String(fields.message)),
        ];
        line.append(&mut fields.extras);
        let event_span = ctx.event_span(event).map(|span| span.id());
        if let Some((trace_id, span_id)) = self.trace_ids(event_span.as_ref()) {
            line.push(("trace_id".to_owned(), Value::String(trace_id)));
            line.push(("span_id".to_owned(), Value::String(span_id)));
        }
        let request_id = ctx.event_scope(event).and_then(|scope| {
            scope.into_iter().find_map(|span| {
                span.extensions()
                    .get::<RequestIdValue>()
                    .map(|v| v.0.clone())
            })
        });
        if let Some(request_id) = request_id {
            line.push(("request_id".to_owned(), Value::String(request_id)));
        }
        let mut bytes = serde_json::to_vec(&Ordered(&line)).unwrap_or_default();
        bytes.push(b'\n');
        // A failing stdout has nowhere to report to.
        let _ = self.make_writer.make_writer_for(metadata).write_all(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_test_writer_flushes() {
        Capture::default().flush().unwrap();
    }

    /// Runs `body` under a subscriber with the JSON layer and returns the
    /// raw output.
    fn output_of(body: impl FnOnce()) -> String {
        let capture = Capture::default();
        let writer = capture.clone();
        let subscriber =
            tracing_subscriber::registry().with(JsonLayer::new(move || writer.clone()));
        tracing::subscriber::with_default(subscriber, body);
        String::from_utf8(capture.0.lock().unwrap().clone()).unwrap()
    }

    fn lines_of(body: impl FnOnce()) -> Vec<Value> {
        output_of(body)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn a_line_is_flat_and_ordered() {
        let output = output_of(|| tracing::warn!(order_id = 7, "created"));
        let line: Value = serde_json::from_str(output.trim_end()).unwrap();
        assert_eq!(line["level"], "WARN");
        assert_eq!(line["message"], "created");
        assert_eq!(line["order_id"], 7);
        assert!(line["logger"].as_str().unwrap().contains("json"));
        assert!(line["timestamp"].as_str().unwrap().ends_with("+00:00"));
        assert!(line.get("trace_id").is_none());
        assert!(line.get("request_id").is_none());
        let positions: Vec<usize> = ["timestamp", "level", "logger", "message", "order_id"]
            .iter()
            .map(|k| output.find(k).unwrap())
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{output}");
    }

    #[test]
    fn every_field_type_is_recorded() {
        #[derive(Debug)]
        struct Shown;
        let io_error = std::io::Error::other("disk");
        let error: &dyn std::error::Error = &io_error;
        let lines = lines_of(|| {
            tracing::info!(
                text = "t",
                signed = -3_i64,
                unsigned = 3_u64,
                flag = true,
                ratio = 0.5,
                nan = f64::NAN,
                shown = ?Shown,
                error = error,
                "m"
            );
        });
        let line = &lines[0];
        assert_eq!(line["text"], "t");
        assert_eq!(line["signed"], -3);
        assert_eq!(line["unsigned"], 3);
        assert_eq!(line["flag"], true);
        assert_eq!(line["ratio"], 0.5);
        assert_eq!(line["nan"], Value::Null);
        assert_eq!(line["shown"], "Shown");
        assert_eq!(line["error"], "disk");
    }

    #[test]
    fn a_message_that_is_not_a_string_is_rendered() {
        let lines = lines_of(|| tracing::info!(message = 5_u64));
        assert_eq!(lines[0]["message"], "5");
    }

    #[test]
    fn credentials_are_redacted_by_key() {
        let lines = lines_of(|| {
            tracing::info!(
                access_token = "abc",
                Authorization = "Bearer x",
                db_password = "p",
                client_secret = 1,
                claims = "c",
                my_Credentials = "k",
                user = "bob",
                "login"
            );
        });
        let line = &lines[0];
        for key in [
            "access_token",
            "Authorization",
            "db_password",
            "client_secret",
            "claims",
            "my_Credentials",
        ] {
            assert_eq!(line[key], "[REDACTED]", "{key}");
        }
        assert_eq!(line["user"], "bob");
        assert!(!lines[0].to_string().contains("Bearer"));
    }

    #[test]
    fn the_lines_own_keys_win_over_event_fields() {
        let lines =
            lines_of(|| tracing::info!(level = "x", request_id = "evil", logger = "y", "m"));
        assert_eq!(lines[0]["level"], "INFO");
        assert!(lines[0].get("request_id").is_none());
        assert_ne!(lines[0]["logger"], "y");
    }

    #[test]
    fn the_request_id_comes_from_the_nearest_span_that_has_one() {
        let lines = lines_of(|| {
            let outer = tracing::error_span!("request", request_id = "outer-id");
            let _outer = outer.enter();
            let plain = tracing::error_span!("plain", other = 1);
            let _plain = plain.enter();
            tracing::info!("inside");
            let inner = tracing::error_span!("request", request_id = ?"inner");
            let _alt = tracing::error_span!("request", request_id = %"shown");
            let _inner = inner.enter();
            tracing::info!("deeper");
        });
        assert_eq!(lines[0]["request_id"], "outer-id");
        assert_eq!(lines[1]["request_id"], "\"inner\"");
    }

    #[test]
    fn a_request_id_recorded_after_the_span_was_made_is_used() {
        let lines = lines_of(|| {
            let span = tracing::error_span!("request", request_id = tracing::field::Empty);
            let _entered = span.enter();
            tracing::info!("before");
            span.record("request_id", "late-id");
            tracing::info!("after");
            // Recording some other field leaves the ID alone.
            span.record("other", 1);
            tracing::info!("still");
        });
        assert!(lines[0].get("request_id").is_none());
        assert_eq!(lines[1]["request_id"], "late-id");
        assert_eq!(lines[2]["request_id"], "late-id");
    }

    #[test]
    fn a_span_without_a_request_id_adds_nothing() {
        let lines = lines_of(|| {
            let span = tracing::error_span!("plain", count = 1);
            let _entered = span.enter();
            tracing::info!("x");
        });
        assert!(lines[0].get("request_id").is_none());
    }

    #[test]
    fn timestamps_are_rfc3339_in_utc() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00.000000+00:00");
        // 2024-02-29T23:59:58.123456Z, a leap day.
        let leap = UNIX_EPOCH + Duration::new(1_709_251_198, 123_456_000);
        assert_eq!(rfc3339(leap), "2024-02-29T23:59:58.123456+00:00");
        // 2026-10-26T12:34:56Z, after the leap-year handling.
        let later = UNIX_EPOCH + Duration::from_secs(1_793_018_096);
        assert_eq!(rfc3339(later), "2026-10-26T12:34:56.000000+00:00");
        // A leap day in a century year that is a leap year.
        let feb = UNIX_EPOCH + Duration::from_hours(264_384);
        assert_eq!(rfc3339(feb), "2000-02-29T00:00:00.000000+00:00");
        assert_eq!(
            rfc3339(UNIX_EPOCH - Duration::from_secs(5)),
            "1970-01-01T00:00:00.000000+00:00"
        );
    }

    /// The whole stack: server span, request ID, a log line inside.
    #[cfg(feature = "otel")]
    #[tokio::test]
    async fn a_line_inside_a_request_carries_the_trace_and_request_ids() {
        use crate::otel::{ServerSpanLayer, trace_layer};
        use crate::request_id::RequestIdLayer;
        use axum::Router;
        use axum::body::Body;
        use axum::http::Request;
        use axum::routing::get;
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::SdkMeterProvider;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
        use tower::ServiceExt as _;

        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let capture = Capture::default();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::registry()
            .with(JsonLayer::new(move || writer.clone()))
            .with(trace_layer(&provider));
        let _guard = tracing::subscriber::set_default(subscriber);
        let app = Router::new()
            .route(
                "/orders/{id}",
                get(|| async {
                    tracing::info!(api_token = "s3cr3t", "handling");
                    "ok"
                }),
            )
            .layer(RequestIdLayer::new())
            .layer(ServerSpanLayer::with_meter(
                &SdkMeterProvider::builder().build().meter("test"),
            ));
        let request = Request::builder()
            .uri("/orders/5")
            .header("x-request-id", "req-7")
            .body(Body::empty())
            .unwrap();
        app.oneshot(request).await.unwrap();

        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        let line: Value = output
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|l| l["message"] == "handling")
            .expect("the handler's line");
        assert_eq!(line["request_id"], "req-7");
        assert_eq!(line["api_token"], "[REDACTED]");
        // The IDs are those of the span the handler ran in: the request-ID
        // layer's, a child of the server span in the same trace.
        let spans = exporter.get_finished_spans().unwrap();
        let server = spans.iter().find(|s| s.name == "GET /orders/{id}").unwrap();
        let child = spans.iter().find(|s| s.name == "request").unwrap();
        assert_eq!(line["trace_id"], server.span_context.trace_id().to_string());
        assert_eq!(line["span_id"], child.span_context.span_id().to_string());
        assert_eq!(line["trace_id"].as_str().unwrap().len(), 32);
        assert_eq!(line["span_id"].as_str().unwrap().len(), 16);
    }

    /// Outside any span there is no trace context to report.
    #[cfg(feature = "otel")]
    #[test]
    fn a_line_outside_a_span_has_no_trace_ids() {
        let lines = lines_of(|| tracing::info!("plain"));
        assert!(lines[0].get("trace_id").is_none());
        assert!(lines[0].get("span_id").is_none());
    }
}
