//! The request ID as a field of the tracing span around a request, seen in
//! the JSON log lines. In a file of its own, and so its own process: a
//! callsite first hit with no subscriber caches "disabled" for the whole
//! process, which would hide the events from the scoped subscriber here. One
//! test function runs every case in turn for the same reason.

use std::convert::Infallible;
use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{HeaderValue, Request, Response};
use axum::routing::get;
use axum::{Extension, Router};
use craftainer_observability::request_id::{RequestId, RequestIdLayer};
use tower::{ServiceBuilder, ServiceExt, service_fn};
use tracing::Level;

async fn handler(Extension(id): Extension<RequestId>) -> String {
    tracing::info!("handling");
    tracing::error!("failing");
    id.to_string()
}

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

/// Runs one request through `app` under a JSON subscriber at `level` and
/// returns every log line.
async fn logs_of<S>(level: Level, app: S) -> Vec<serde_json::Value>
where
    S: tower::Service<Request<Body>, Response = Response<Body>, Error = Infallible>,
{
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(level)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let request = Request::builder()
        .uri("/")
        .header("x-request-id", HeaderValue::from_static("trace-me"))
        .body(Body::empty())
        .unwrap();
    app.oneshot(request).await.unwrap();
    let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    output
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn line<'a>(lines: &'a [serde_json::Value], message: &str) -> &'a serde_json::Value {
    lines
        .iter()
        .find(|v| v["fields"]["message"] == message)
        .unwrap_or_else(|| panic!("no log line {message:?} in {lines:?}"))
}

fn app() -> Router {
    Router::new()
        .route("/", get(handler))
        .layer(RequestIdLayer::new())
}

#[tokio::test]
async fn the_id_is_a_field_of_the_span_around_the_request() {
    // Every event inside the request has it.
    let lines = logs_of(Level::INFO, app()).await;
    for message in ["handling", "failing"] {
        let entry = line(&lines, message);
        assert_eq!(entry["span"]["request_id"], "trace-me");
        assert_eq!(entry["spans"][0]["request_id"], "trace-me");
    }

    // A filter at WARN drops the INFO event but not the ID on the ERROR one.
    let lines = logs_of(Level::WARN, app()).await;
    assert!(!lines.iter().any(|v| v["fields"]["message"] == "handling"));
    assert_eq!(line(&lines, "failing")["span"]["request_id"], "trace-me");

    // An event logged while the inner service is called, before its future
    // is polled, has it too.
    let eager = ServiceBuilder::new()
        .layer(RequestIdLayer::new())
        .service(service_fn(|_: Request<Body>| {
            tracing::info!("in call");
            async { Ok::<_, Infallible>(Response::new(Body::empty())) }
        }));
    let lines = logs_of(Level::INFO, eager).await;
    assert_eq!(line(&lines, "in call")["span"]["request_id"], "trace-me");
}
