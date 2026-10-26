//! Request IDs: a tower layer that gives every request a validated ID.
//!
//! [`RequestIdLayer`] reads the request-ID header (`X-Request-ID` by default,
//! see [`RequestIdLayer::with_header_name`]), keeps its value only when it is
//! 1 to 128 characters from `[A-Za-z0-9._-]` (so a client cannot put newlines
//! or other control characters into a log line), and otherwise replaces it
//! with a fresh UUID (32 lowercase hex digits, as `lib-observability-py`
//! does). The ID is then
//!
//! - written back to the request's header, so a handler that reads the header
//!   sees the validated value;
//! - stored in the request's extensions as a [`RequestId`];
//! - carried by a `request` tracing span, as the field `request_id`, that
//!   encloses the rest of the request, so every event logged while handling it
//!   has the ID in its span fields (the JSON log lines show it under `span`
//!   and `spans`). The span is at ERROR level so that no log filter below
//!   `off` drops it. A task spawned with `tokio::spawn` does not inherit it;
//!   instrument that task with `tracing::Span::current()`;
//! - echoed on the response under the same header name, replacing any value
//!   the inner service set.
//!
//! `tower-http`'s `SetRequestIdLayer` keeps an incoming ID without validating
//! it, which is why this crate has its own layer.
//!
//! # Examples
//!
//! ```
//! use axum::{Extension, Router, routing::get};
//! use craftainer_observability::request_id::{RequestId, RequestIdLayer};
//!
//! async fn handler(Extension(id): Extension<RequestId>) -> String {
//!     format!("request {id}")
//! }
//!
//! let app: Router = Router::new().route("/", get(handler)).layer(RequestIdLayer::new());
//! ```

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use axum::http::{HeaderName, HeaderValue, Request, Response};
use pin_project_lite::pin_project;
use tower_layer::Layer;
use tower_service::Service;
use tracing::instrument::Instrumented;
use tracing::{Instrument, error_span};

/// The default request and response header: `X-Request-ID`.
pub const DEFAULT_HEADER_NAME: HeaderName = HeaderName::from_static("x-request-id");

/// The longest request ID the layer accepts.
const MAX_LEN: usize = 128;

/// A request's ID, as [`RequestIdLayer`] stored it in the request's
/// extensions. Take it in a handler with `Extension<RequestId>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(String);

impl RequestId {
    /// The ID as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RequestId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Whether `candidate` is 1 to 128 characters from `[A-Za-z0-9._-]`.
fn is_valid(candidate: &[u8]) -> bool {
    (1..=MAX_LEN).contains(&candidate.len())
        && candidate
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A [`Layer`] that validates or creates a request ID, makes it available to
/// the rest of the request and echoes it on the response. See the module
/// docs.
#[derive(Debug, Clone)]
pub struct RequestIdLayer {
    header_name: HeaderName,
}

impl RequestIdLayer {
    /// A layer using the [`DEFAULT_HEADER_NAME`] (`X-Request-ID`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            header_name: DEFAULT_HEADER_NAME,
        }
    }

    /// Uses `header_name` for both the request and the response header.
    #[must_use]
    pub fn with_header_name(mut self, header_name: HeaderName) -> Self {
        self.header_name = header_name;
        self
    }
}

impl Default for RequestIdLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for RequestIdLayer {
    type Service = RequestIdService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestIdService {
            inner,
            header_name: self.header_name.clone(),
        }
    }
}

/// The [`Service`] that [`RequestIdLayer`] wraps an inner service in.
#[derive(Debug, Clone)]
pub struct RequestIdService<S> {
    inner: S,
    header_name: HeaderName,
}

impl<S> RequestIdService<S> {
    /// The client's ID if it is valid, otherwise a freshly generated one.
    fn request_id<B>(&self, req: &Request<B>) -> HeaderValue {
        req.headers()
            .get(&self.header_name)
            .filter(|value| is_valid(value.as_bytes()))
            .cloned()
            .unwrap_or_else(|| {
                HeaderValue::from_str(&uuid::Uuid::new_v4().simple().to_string())
                    .expect("hex digits are a header value")
            })
    }
}

impl<S, B, ResBody> Service<Request<B>> for RequestIdService<S>
where
    S: Service<Request<B>, Response = Response<ResBody>>,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let value = self.request_id(&req);
        // Valid IDs are ASCII, so this cannot fail.
        let id = value
            .to_str()
            .expect("a validated request ID is ASCII")
            .to_owned();
        req.headers_mut()
            .insert(self.header_name.clone(), value.clone());
        // ERROR, not INFO: a span below the active filter is disabled, and
        // the ID must reach every event, a warning or an error included.
        let span = error_span!("request", request_id = %id);
        req.extensions_mut().insert(RequestId(id));
        // Entered for the synchronous part of `call` (a layer or router that
        // logs there), then wrapped around the future it returns.
        let future = span.in_scope(|| self.inner.call(req));
        ResponseFuture {
            inner: future.instrument(span),
            header_name: self.header_name.clone(),
            value,
        }
    }
}

pin_project! {
    /// The future of a [`RequestIdService`] call: the inner future, run inside
    /// the request's span, whose response gets the ID header.
    pub struct ResponseFuture<F> {
        #[pin]
        inner: Instrumented<F>,
        header_name: HeaderName,
        value: HeaderValue,
    }
}

impl<F, ResBody, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
{
    type Output = Result<Response<ResBody>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let mut response = ready!(this.inner.poll(cx))?;
        response
            .headers_mut()
            .insert(this.header_name.clone(), this.value.clone());
        Poll::Ready(Ok(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_allowed_characters_up_to_128() {
        assert!(is_valid(b"abc-DEF_123.x"));
        assert!(is_valid(b"a"));
        assert!(is_valid(&[b'a'; 128]));
    }

    #[test]
    fn rejects_empty_too_long_and_other_characters() {
        assert!(!is_valid(b""));
        assert!(!is_valid(&[b'a'; 129]));
        for bad in [
            &b"has space"[..],
            b"a/b",
            b"a:b",
            b"tab\there",
            b"line\nbreak",
            b"carriage\rreturn",
            b"crlf\r\nX-Injected: 1",
            b"nul\0",
            b"caf\xc3\xa9",
        ] {
            assert!(!is_valid(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn the_request_id_renders_as_its_string() {
        let id = RequestId("abc".to_string());
        assert_eq!(id.as_str(), "abc");
        assert_eq!(id.as_ref(), "abc");
        assert_eq!(id.to_string(), "abc");
    }
}
