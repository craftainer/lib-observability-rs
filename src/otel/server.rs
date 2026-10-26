//! The server-span tower layer.
//!
//! [`ServerSpanLayer`] opens one `tracing` span per request, which the trace
//! layer installed by [`configure_telemetry`](crate::logs::configure_telemetry)
//! exports as an OpenTelemetry server span, and records the
//! `http.server.request.duration` histogram (seconds). Mirrors what
//! `opentelemetry-instrumentation-fastapi` does for `lib-observability-py`:
//!
//! - the span is named `METHOD /matched/route` (axum's [`MatchedPath`], so a
//!   path parameter never makes a span name or an attribute value of its own)
//!   and just `METHOD` when no route matched. Apply the layer with
//!   `Router::layer`, which runs after routing, for the route to be known;
//! - requests whose path starts with one of the excluded prefixes
//!   (`/health/` by default) get no span and no metric, so probes do not
//!   flood the traces. `lib-observability-py`'s default is the regex
//!   `/health/.*`;
//! - a valid `traceparent` request header makes the span a child of the
//!   caller's;
//! - attributes: `http.request.method`, `url.path`, `http.route`,
//!   `http.response.status_code`, and `http.request.header.x-request-id`
//!   (a string array, read from the response, so put this layer outside
//!   [`RequestIdLayer`](crate::request_id::RequestIdLayer) for the validated
//!   ID); a 5xx response, or an error from the inner service, sets the span
//!   status to error.
//!
//! The span is at ERROR level, like the request-ID span, so that no log
//! filter below `off` drops it (`RUST_LOG=warn` would otherwise disable
//! tracing altogether).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Instant;

use axum::extract::MatchedPath;
use axum::http::{HeaderMap, HeaderName, Method, Request, Response, StatusCode};
use opentelemetry::metrics::{Histogram, Meter};
use opentelemetry::propagation::{Extractor, TextMapPropagator};
use opentelemetry::trace::TraceContextExt as _;
use opentelemetry::{Array, KeyValue, StringValue, Value, global};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use pin_project_lite::pin_project;
use tower_layer::Layer;
use tower_service::Service;
use tracing::field::Empty;
use tracing::instrument::Instrumented;
use tracing::{Instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use super::SCOPE;

/// Path prefixes that get no span and no metric unless changed.
pub const DEFAULT_EXCLUDED_PREFIXES: [&str; 1] = ["/health/"];

/// The explicit bucket boundaries (seconds) the OpenTelemetry HTTP semantic
/// conventions recommend for `http.server.request.duration`.
const DURATION_BUCKETS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

/// A [`Layer`] that gives every request a server span and a duration
/// measurement. See the module docs.
#[derive(Debug, Clone)]
pub struct ServerSpanLayer {
    duration: Histogram<f64>,
    excluded: Arc<[String]>,
    request_id_header: HeaderName,
}

impl ServerSpanLayer {
    /// A layer measuring with the process-wide meter provider, which
    /// [`configure_telemetry`](crate::logs::configure_telemetry) sets: build
    /// it after that call. Without telemetry the measurement is a no-op.
    #[must_use]
    pub fn new() -> Self {
        Self::with_meter(&global::meter(SCOPE))
    }

    /// A layer measuring with `meter`.
    #[must_use]
    pub fn with_meter(meter: &Meter) -> Self {
        Self {
            duration: meter
                .f64_histogram("http.server.request.duration")
                .with_unit("s")
                .with_description("Duration of HTTP server requests.")
                .with_boundaries(DURATION_BUCKETS.to_vec())
                .build(),
            excluded: DEFAULT_EXCLUDED_PREFIXES.map(str::to_owned).into(),
            request_id_header: crate::request_id::DEFAULT_HEADER_NAME,
        }
    }

    /// Replaces the excluded path prefixes ([`DEFAULT_EXCLUDED_PREFIXES`]).
    /// An empty list excludes nothing.
    #[must_use]
    pub fn with_excluded_prefixes<I, P>(mut self, prefixes: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<String>,
    {
        self.excluded = prefixes.into_iter().map(Into::into).collect();
        self
    }

    /// The response header the `http.request.header.*` attribute is read
    /// from; set it to the same name as
    /// [`RequestIdLayer::with_header_name`](crate::request_id::RequestIdLayer::with_header_name).
    #[must_use]
    pub fn with_request_id_header(mut self, header_name: HeaderName) -> Self {
        self.request_id_header = header_name;
        self
    }
}

impl Default for ServerSpanLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for ServerSpanLayer {
    type Service = ServerSpanService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ServerSpanService {
            inner,
            layer: self.clone(),
        }
    }
}

/// The [`Service`] that [`ServerSpanLayer`] wraps an inner service in.
#[derive(Debug, Clone)]
pub struct ServerSpanService<S> {
    inner: S,
    layer: ServerSpanLayer,
}

/// Reads request headers for the trace-context propagator.
struct HeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(HeaderName::as_str).collect()
    }
}

/// What the response future needs to finish a request's span and metric.
struct Tracing {
    span: Span,
    start: Instant,
    duration: Histogram<f64>,
    method: String,
    route: Option<String>,
    request_id_header: HeaderName,
}

/// What a response tells the span and the metric.
struct Outcome {
    status: StatusCode,
    request_id: Option<String>,
}

impl Outcome {
    /// Reads `response`; the request ID is its `header`.
    fn of<B>(response: &Response<B>, header: &HeaderName) -> Self {
        Self {
            status: response.status(),
            request_id: response
                .headers()
                .get(header)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        }
    }
}

impl Tracing {
    /// Records the outcome (`None`: the inner service failed) on the span
    /// and the histogram.
    fn finish(self, outcome: Option<Outcome>) {
        let mut attributes = vec![KeyValue::new("http.request.method", self.method)];
        if let Some(route) = self.route {
            attributes.push(KeyValue::new("http.route", route));
        }
        if let Some(outcome) = outcome {
            let status = i64::from(outcome.status.as_u16());
            self.span.record("http.response.status_code", status);
            if outcome.status.is_server_error() {
                self.span.record("otel.status_code", "ERROR");
            }
            attributes.push(KeyValue::new("http.response.status_code", status));
            if let Some(id) = outcome.request_id {
                self.span.set_attribute(
                    format!("http.request.header.{}", self.request_id_header),
                    Value::Array(Array::String(vec![StringValue::from(id)])),
                );
            }
        } else {
            self.span.record("otel.status_code", "ERROR");
            attributes.push(KeyValue::new("error.type", "_OTHER"));
        }
        self.duration
            .record(self.start.elapsed().as_secs_f64(), &attributes);
    }
}

impl ServerSpanLayer {
    /// Opens the span for a request, or `None` when its path is excluded. Kept
    /// free of the service's generics so each line is one function.
    fn begin(
        &self,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
        matched: Option<&MatchedPath>,
    ) -> Option<Tracing> {
        if self
            .excluded
            .iter()
            .any(|prefix| path.starts_with(prefix.as_str()))
        {
            return None;
        }
        let method = method.as_str().to_owned();
        let route = matched.map(|matched| matched.as_str().to_owned());
        let name = route
            .as_ref()
            .map_or_else(|| method.clone(), |route| format!("{method} {route}"));
        let route_field = route.as_deref();
        // ERROR, not INFO: see the module docs.
        let span = tracing::error_span!(
            "http.server",
            otel.name = %name,
            otel.kind = "server",
            otel.status_code = Empty,
            http.request.method = %method,
            http.route = route_field,
            url.path = %path,
            http.response.status_code = Empty,
        );
        let parent = TraceContextPropagator::new().extract(&HeaderExtractor(headers));
        if parent.span().span_context().is_valid() {
            // A span that cannot take a parent (no trace layer installed)
            // simply stays a root.
            let _ = span.set_parent(parent);
        }
        Some(Tracing {
            span,
            start: Instant::now(),
            duration: self.duration.clone(),
            method,
            route,
            request_id_header: self.request_id_header.clone(),
        })
    }
}

impl<S, B, ResBody> Service<Request<B>> for ServerSpanService<S>
where
    S: Service<Request<B>, Response = Response<ResBody>>,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let matched = req.extensions().get::<MatchedPath>();
        let tracing = self
            .layer
            .begin(req.method(), req.uri().path(), req.headers(), matched);
        let span = tracing.as_ref().map_or_else(Span::none, |t| t.span.clone());
        let future = span.in_scope(|| self.inner.call(req));
        ResponseFuture {
            inner: future.instrument(span),
            tracing,
        }
    }
}

pin_project! {
    /// The future of a [`ServerSpanService`] call: the inner future run inside
    /// the request's span, whose outcome is then recorded.
    pub struct ResponseFuture<F> {
        #[pin]
        inner: Instrumented<F>,
        tracing: Option<Tracing>,
    }
}

impl<F, ResBody, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
{
    type Output = Result<Response<ResBody>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let result = ready!(this.inner.poll(cx));
        if let Some(tracing) = this.tracing.take() {
            let outcome = result
                .as_ref()
                .ok()
                .map(|r| Outcome::of(r, &tracing.request_id_header));
            tracing.finish(outcome);
        }
        Poll::Ready(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otel::trace_layer;
    use crate::request_id::{DEFAULT_HEADER_NAME, RequestIdLayer};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderValue, StatusCode};
    use axum::routing::get;
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry::trace::{SpanKind, Status};
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics as A, MetricData as D};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
    use tower::{ServiceBuilder, ServiceExt, service_fn};
    use tracing_subscriber::layer::SubscriberExt;

    struct Telemetry {
        spans: InMemorySpanExporter,
        metrics: InMemoryMetricExporter,
        tracer: SdkTracerProvider,
        meter: SdkMeterProvider,
    }

    impl Telemetry {
        fn new() -> Self {
            let spans = InMemorySpanExporter::default();
            let metrics = InMemoryMetricExporter::default();
            Self {
                tracer: SdkTracerProvider::builder()
                    .with_simple_exporter(spans.clone())
                    .build(),
                meter: SdkMeterProvider::builder()
                    .with_reader(PeriodicReader::builder(metrics.clone()).build())
                    .build(),
                spans,
                metrics,
            }
        }

        fn layer(&self) -> ServerSpanLayer {
            ServerSpanLayer::with_meter(&self.meter.meter("test"))
        }

        /// The finished server spans (`RequestIdLayer`'s own `request` span
        /// is exported too, as an internal child span).
        fn spans(&self) -> Vec<SpanData> {
            self.all_spans()
                .into_iter()
                .filter(|span| span.span_kind == SpanKind::Server)
                .collect()
        }

        fn all_spans(&self) -> Vec<SpanData> {
            self.spans.get_finished_spans().unwrap()
        }

        /// Every `(attributes, count)` of the duration histogram.
        fn durations(&self) -> Vec<(Vec<(String, String)>, u64)> {
            self.meter.force_flush().unwrap();
            let mut points = Vec::new();
            for resource_metrics in self.metrics.get_finished_metrics().unwrap() {
                for scope in resource_metrics.scope_metrics() {
                    for metric in scope.metrics() {
                        assert_eq!(metric.name(), "http.server.request.duration");
                        assert_eq!(metric.unit(), "s");
                        let d = metric.data();
                        let A::F64(D::Histogram(h)) = d else { panic!() };
                        for point in h.data_points() {
                            let mut attributes: Vec<_> = point
                                .attributes()
                                .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                                .collect();
                            attributes.sort();
                            points.push((attributes, point.count()));
                        }
                    }
                }
            }
            points
        }
    }

    fn attr(span: &SpanData, key: &str) -> Option<String> {
        span.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.to_string())
    }

    fn app(layer: ServerSpanLayer) -> Router {
        Router::new()
            .route("/items/{id}", get(|| async { "item" }))
            .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
            .route("/health/live", get(|| async { "ok" }))
            .route("/internal/x", get(|| async { "x" }))
            .layer(RequestIdLayer::new())
            .layer(layer)
    }

    async fn send(app: Router, uri: &str, headers: &[(&str, &str)]) -> StatusCode {
        let mut request = Request::builder().uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        app.oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn a_request_gets_a_server_span_and_a_duration() {
        let t = Telemetry::new();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(trace_layer(&t.tracer)),
        );
        let status = send(
            app(t.layer()),
            "/items/42?x=1",
            &[("x-request-id", "abc-1")],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let spans = t.spans();
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "GET /items/{id}");
        assert_eq!(span.span_kind, SpanKind::Server);
        assert_eq!(attr(span, "http.route").as_deref(), Some("/items/{id}"));
        assert_eq!(attr(span, "http.request.method").as_deref(), Some("GET"));
        assert_eq!(attr(span, "url.path").as_deref(), Some("/items/42"));
        assert_eq!(
            attr(span, "http.response.status_code").as_deref(),
            Some("200")
        );
        assert_eq!(
            attr(span, "http.request.header.x-request-id").as_deref(),
            Some(r#"["abc-1"]"#)
        );
        assert_eq!(span.status, Status::Unset);
        // The request-ID layer's span hangs under it and carries the ID.
        let all = t.all_spans();
        let child = all
            .iter()
            .find(|s| s.name == "request")
            .expect("request span");
        assert_eq!(child.parent_span_id, span.span_context.span_id());
        assert_eq!(attr(child, "request_id").as_deref(), Some("abc-1"));
        assert_eq!(
            t.durations(),
            vec![(
                vec![
                    ("http.request.method".to_owned(), "GET".to_owned()),
                    ("http.response.status_code".to_owned(), "200".to_owned()),
                    ("http.route".to_owned(), "/items/{id}".to_owned()),
                ],
                1
            )]
        );
    }

    #[tokio::test]
    async fn health_probes_get_no_span_and_no_metric() {
        let t = Telemetry::new();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(trace_layer(&t.tracer)),
        );
        assert_eq!(
            send(app(t.layer()), "/health/live", &[]).await,
            StatusCode::OK
        );
        assert_eq!(t.spans().len(), 0);
        assert_eq!(t.durations().len(), 0);
    }

    #[tokio::test]
    async fn the_excluded_prefixes_can_be_replaced() {
        let t = Telemetry::new();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(trace_layer(&t.tracer)),
        );
        let layer = t.layer().with_excluded_prefixes(["/internal/"]);
        send(app(layer.clone()), "/internal/x", &[]).await;
        assert_eq!(t.spans().len(), 0);
        // The default exclusion is gone: /health/ is traced now.
        send(app(layer), "/health/live", &[]).await;
        assert_eq!(t.spans().len(), 1);
        // And an empty list excludes nothing.
        send(
            app(t.layer().with_excluded_prefixes(Vec::<String>::new())),
            "/internal/x",
            &[],
        )
        .await;
        assert_eq!(t.spans().len(), 2);
    }

    #[tokio::test]
    async fn an_unmatched_route_is_named_by_its_method_alone() {
        let t = Telemetry::new();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(trace_layer(&t.tracer)),
        );
        assert_eq!(
            send(app(t.layer()), "/nope/1234", &[]).await,
            StatusCode::NOT_FOUND
        );
        let spans = t.spans();
        assert_eq!(spans[0].name, "GET");
        assert_eq!(attr(&spans[0], "http.route"), None);
        assert_eq!(
            attr(&spans[0], "http.response.status_code").as_deref(),
            Some("404")
        );
        // The ID is minted, so the attribute is there even without a client ID.
        assert!(attr(&spans[0], "http.request.header.x-request-id").is_some());
        assert_eq!(spans[0].status, Status::Unset);
    }

    #[tokio::test]
    async fn a_server_error_marks_the_span_as_an_error() {
        let t = Telemetry::new();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(trace_layer(&t.tracer)),
        );
        send(app(t.layer()), "/boom", &[]).await;
        assert!(matches!(t.spans()[0].status, Status::Error { .. }));
    }

    #[tokio::test]
    async fn a_traceparent_header_parents_the_span() {
        let t = Telemetry::new();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(trace_layer(&t.tracer)),
        );
        let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
        let header = format!("00-{trace_id}-00f067aa0ba902b7-01");
        send(app(t.layer()), "/items/1", &[("traceparent", &header)]).await;
        send(app(t.layer()), "/items/1", &[("traceparent", "garbage")]).await;
        let spans = t.spans();
        assert_eq!(spans[0].span_context.trace_id().to_string(), trace_id);
        assert_eq!(spans[0].parent_span_id.to_string(), "00f067aa0ba902b7");
        assert_ne!(spans[1].span_context.trace_id().to_string(), trace_id);
        assert_eq!(spans[1].parent_span_id.to_string(), "0000000000000000");
    }

    #[tokio::test]
    async fn a_failing_inner_service_is_an_error_span() {
        let t = Telemetry::new();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(trace_layer(&t.tracer)),
        );
        let service =
            ServiceBuilder::new()
                .layer(t.layer())
                .service(service_fn(|_: Request<Body>| async {
                    Err::<Response<Body>, _>(std::io::Error::other("down"))
                }));
        let request = Request::builder().uri("/x").body(Body::empty()).unwrap();
        assert!(service.oneshot(request).await.is_err());
        assert!(matches!(t.spans()[0].status, Status::Error { .. }));
        let durations = t.durations();
        assert!(
            durations[0]
                .0
                .contains(&("error.type".to_owned(), "_OTHER".to_owned()))
        );
    }

    #[tokio::test]
    async fn the_request_id_header_name_is_configurable() {
        let t = Telemetry::new();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(trace_layer(&t.tracer)),
        );
        let name = HeaderName::from_static("x-correlation-id");
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(RequestIdLayer::new().with_header_name(name.clone()))
            .layer(t.layer().with_request_id_header(name));
        send(app, "/", &[("x-correlation-id", "c-9")]).await;
        assert_eq!(
            attr(&t.spans()[0], "http.request.header.x-correlation-id").as_deref(),
            Some(r#"["c-9"]"#)
        );
    }

    #[tokio::test]
    async fn without_a_trace_layer_requests_still_work() {
        let t = Telemetry::new();
        // No subscriber at all: the span is disabled, nothing is exported.
        let layer = t.layer();
        let response = app(layer)
            .oneshot(
                Request::builder()
                    .uri("/items/1")
                    .header(
                        "traceparent",
                        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(t.spans().len(), 0);
    }

    #[test]
    fn defaults() {
        let _ = ServerSpanLayer::default();
        let _ = ServerSpanLayer::new();
        assert_eq!(DEFAULT_EXCLUDED_PREFIXES, ["/health/"]);
        let _ = DEFAULT_HEADER_NAME;
    }

    #[test]
    fn headers_are_listed_for_the_propagator() {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static("x"));
        headers.insert("bad", HeaderValue::from_bytes(b"\xff").unwrap());
        let extractor = HeaderExtractor(&headers);
        assert_eq!(extractor.get("traceparent"), Some("x"));
        assert_eq!(extractor.get("bad"), None);
        assert_eq!(extractor.get("missing"), None);
        let mut keys = extractor.keys();
        keys.sort_unstable();
        assert_eq!(keys, ["bad", "traceparent"]);
    }
}
