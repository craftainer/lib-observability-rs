//! `RequestIdLayer` behind an Axum router, driven in process. Needs no
//! backing service. The log checks use a scoped subscriber, not the global
//! one, so they cannot clash with other tests.

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request, Response, StatusCode};
use axum::routing::get;
use axum::{Extension, Router};
use craftainer_observability::request_id::{DEFAULT_HEADER_NAME, RequestId, RequestIdLayer};
use tower::ServiceExt;

async fn handler(Extension(id): Extension<RequestId>) -> String {
    id.as_str().to_owned()
}

/// Echoes the request header as the body, and sets its own response header
/// of the same name, which the layer must overwrite.
async fn header_probe(request: Request<Body>) -> Response<Body> {
    let seen = request
        .headers()
        .get("x-request-id")
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    Response::builder()
        .header("x-request-id", "from-the-handler")
        .body(Body::from(seen))
        .unwrap()
}

fn app(layer: RequestIdLayer) -> Router {
    Router::new()
        .route("/", get(handler))
        .route("/probe", get(header_probe))
        .layer(layer)
}

async fn send(
    app: Router,
    path: &str,
    header: Option<(&str, HeaderValue)>,
) -> (Response<Body>, String) {
    let mut builder = Request::builder().uri(path);
    if let Some((name, value)) = header {
        builder = builder.header(name, value);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    (Response::from_parts(parts, Body::empty()), text)
}

fn echoed(response: &Response<Body>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

fn is_fresh_uuid(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[tokio::test]
async fn a_valid_id_is_kept_echoed_and_put_in_extensions() {
    let value = HeaderValue::from_static("Req_1.2-abc");
    let (response, body) = send(
        app(RequestIdLayer::new()),
        "/",
        Some(("x-request-id", value)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(echoed(&response, "x-request-id"), "Req_1.2-abc");
    assert_eq!(body, "Req_1.2-abc");
}

#[tokio::test]
async fn the_longest_valid_id_is_kept() {
    let id = "a".repeat(128);
    let value = HeaderValue::from_str(&id).unwrap();
    let (response, _) = send(
        app(RequestIdLayer::new()),
        "/",
        Some(("x-request-id", value)),
    )
    .await;
    assert_eq!(echoed(&response, "x-request-id"), id);
}

#[tokio::test]
async fn invalid_ids_are_replaced_by_a_fresh_one() {
    let too_long = "a".repeat(129);
    let cases: Vec<HeaderValue> = vec![
        HeaderValue::from_static("has space"),
        HeaderValue::from_static("slash/colon:"),
        HeaderValue::from_static("tab\there"),
        HeaderValue::from_bytes(b"caf\xe9").unwrap(),
        HeaderValue::from_static(""),
        HeaderValue::from_str(&too_long).unwrap(),
    ];
    for value in cases {
        let (response, body) = send(
            app(RequestIdLayer::new()),
            "/",
            Some(("x-request-id", value.clone())),
        )
        .await;
        let id = echoed(&response, "x-request-id");
        assert!(is_fresh_uuid(&id), "{value:?} was not replaced: {id}");
        assert_eq!(body, id);
    }
}

#[tokio::test]
async fn a_newline_cannot_get_through() {
    // hyper rejects a raw CR or LF in a header value before it reaches the
    // layer, and `HeaderValue` cannot hold one at all.
    for raw in ["a\r\nX-Injected: 1", "a\nb", "a\rb"] {
        assert!(HeaderValue::from_str(raw).is_err());
    }
    // What can arrive is a tab or opaque bytes; the layer rejects both.
    let value = HeaderValue::from_static("abc\tX-Injected: 1");
    let (response, _) = send(
        app(RequestIdLayer::new()),
        "/",
        Some(("x-request-id", value)),
    )
    .await;
    let id = echoed(&response, "x-request-id");
    assert!(is_fresh_uuid(&id));
    assert!(!response.headers().contains_key("x-injected"));
}

#[tokio::test]
async fn a_missing_id_is_generated_and_differs_per_request() {
    let (first, body) = send(app(RequestIdLayer::default()), "/", None).await;
    let first_id = echoed(&first, "x-request-id");
    assert!(is_fresh_uuid(&first_id));
    assert_eq!(body, first_id);
    let (second, _) = send(app(RequestIdLayer::default()), "/", None).await;
    assert_ne!(echoed(&second, "x-request-id"), first_id);
}

#[tokio::test]
async fn the_header_name_is_configurable() {
    let layer = RequestIdLayer::new().with_header_name(HeaderName::from_static("x-correlation-id"));
    let value = HeaderValue::from_static("corr-1");
    let (response, body) = send(app(layer.clone()), "/", Some(("x-correlation-id", value))).await;
    assert_eq!(echoed(&response, "x-correlation-id"), "corr-1");
    assert_eq!(body, "corr-1");
    assert!(!response.headers().contains_key("x-request-id"));

    // The default header is ignored once another name is configured.
    let value = HeaderValue::from_static("ignored");
    let (response, _) = send(app(layer), "/", Some(("x-request-id", value))).await;
    assert!(is_fresh_uuid(&echoed(&response, "x-correlation-id")));
}

#[test]
fn the_default_header_name_is_x_request_id() {
    assert_eq!(DEFAULT_HEADER_NAME.as_str(), "x-request-id");
}

#[tokio::test]
async fn the_request_header_holds_the_validated_id_and_the_response_header_is_overwritten() {
    let value = HeaderValue::from_static("bad id");
    let (response, body) = send(
        app(RequestIdLayer::new()),
        "/probe",
        Some(("x-request-id", value)),
    )
    .await;
    let id = echoed(&response, "x-request-id");
    assert!(is_fresh_uuid(&id));
    assert_eq!(body, id, "the handler saw the replaced ID");
    assert_eq!(response.headers().get_all("x-request-id").iter().count(), 1);
}
