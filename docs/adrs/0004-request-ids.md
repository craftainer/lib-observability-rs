# 0004. Request IDs at parity with lib-observability-py

## Status

Accepted

## Context

AX7 adds request IDs, which `lib-observability-py` (`middleware.py`) has and
this crate lacked (ADR 0002 lists them as a later item). The rule from ADR
0003 holds: match Python, defaults included, and record each deviation.
`tower-http`'s `SetRequestIdLayer` keeps an incoming ID without validating it,
so it cannot be used as is: a client could put anything, including text that
forges a log line, into the ID.

## Decision

`request_id::RequestIdLayer` is the crate's own tower layer, with no Cargo
feature. The crate gates a module behind a feature only when it needs a
backing-service client (`postgres`, `redis`, `s3`, `http`), an optional
export stack (`otel`) or a test stub (`testing`). The layer needs none of
these; it adds only `tower-layer`, `tower-service`, `pin-project-lite` and
`uuid`, which `axum` and its dependencies already compile, like the always-on
health router and `logs`.

Matched with Python:

- Header `X-Request-ID`, configurable (`with_header_name`); the same name is
  used on the request and the response.
- Valid means 1 to 128 characters from `[A-Za-z0-9._-]`, matched in full.
- An invalid or missing ID is replaced by a fresh UUID v4, 32 lowercase hex
  digits (`uuid4().hex`).
- Only the first header of that name is looked at; if it is invalid the ID is
  replaced, not searched for further.
- The ID is echoed on the response, replacing any value the inner service set.
- The log field is called `request_id`.

Deviations from Python:

- **A tracing span instead of a ContextVar.** The layer opens a `request`
  span with the field `request_id` around the inner service, so any event
  logged while handling the request carries it. The crate's JSON lines print
  span fields under `span` (the innermost span) and `spans` (all of them), not
  as a top-level `request_id` as Python's formatter does. Flattening them
  would need a custom event formatter; a log pipeline can read
  `span.request_id` or `spans[].request_id`. Code that needs the ID in a
  handler takes `Extension<RequestId>` (Python's `get_request_id()`).
- **The span is at ERROR level.** A span below the active filter is
  disabled, so an INFO span would vanish under `RUST_LOG=warn` and warnings
  and errors would lose the ID, which Python's ContextVar never does. The
  span also encloses the synchronous part of the inner service's `call`. A
  task started with `tokio::spawn` does not inherit it (a ContextVar copies
  into tasks); the host instruments such a task with `Span::current()`.
- **The validated ID is written back to the request header**, so a handler or
  inner layer that reads the header cannot see the rejected value (Python
  leaves the client's header in the ASGI scope). `tower-http` also sets it.
- **No span attribute is set.** Python sets
  `http.request.header.x-request-id` on the active OTel server span. The
  `request_id` field on the tracing span is what exists now; the OTel
  attribute belongs with the server-span layer (AX8).
- **The header name is an `http::HeaderName`**, so an invalid name cannot be
  configured. Python takes a string and fails when encoding it.
- Python also handles WebSocket scopes explicitly; in Axum a WebSocket
  upgrade is an ordinary HTTP request, so it is covered.

Public API (module `request_id`): `RequestIdLayer` (`new`, `Default`,
`with_header_name`), `RequestIdService`, `ResponseFuture`, `RequestId`
(`as_str`, `Display`, `AsRef<str>`; read it with `Extension<RequestId>`) and
`DEFAULT_HEADER_NAME`. New dependencies: `pin-project-lite`, `tower-layer`,
`tower-service`, `uuid` (feature `v4`, no default features).

## Consequences

A service adds `.layer(RequestIdLayer::new())` to its router; layers added
later wrap it, so it should sit outside a trace layer that logs the request
if that layer's own events should carry the ID. The ID reaches the logs only
through the span; a host that wants a top-level `request_id` key needs its
own formatter. The echo needs a response, so an inner service that returns
`Err` (rather than a response) gets no header from this layer; Axum routers
are infallible, and a fallible layer should sit outside this one. Two stacked
`RequestIdLayer`s each make an ID; the inner one wins in the extensions.
Python follow-up: none required. The per-request cost is a
span, a header insert and, when the client sent no valid ID, a UUID.
