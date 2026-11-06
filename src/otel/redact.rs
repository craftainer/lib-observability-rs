//! Credential redaction in front of the span exporter.
//!
//! `tracing-opentelemetry` copies every span field (`#[instrument]` arguments
//! included) and event field into the OpenTelemetry span, so credentials that
//! the stdout JSON layer hides would otherwise reach the collector. This
//! wraps the exporter and redacts, with the crate's one key list
//! ([`crate::redact_keys`]), the span name, its attributes, its events and
//! its error status.

use std::time::Duration;

use opentelemetry::trace::Status;
use opentelemetry::{KeyValue, Value};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};

use crate::redact_keys::{REDACTED, is_secret_key, redact_text};

/// A [`SpanExporter`] that redacts every span before handing it to `E`.
#[derive(Debug)]
pub(crate) struct RedactingSpanExporter<E>(pub(crate) E);

/// Attributes that hold a URL or path: their query and fragment can carry
/// tokens, so only the path is kept.
const URL_KEYS: [&str; 4] = ["http.url", "url.full", "http.target", "url.path"];

/// `attributes` with secret values replaced, string values scrubbed, the
/// query and fragment cut from URL attributes and `url.query` dropped.
fn redact_attributes(attributes: &mut Vec<KeyValue>) {
    attributes.retain(|attribute| attribute.key.as_str() != "url.query");
    for attribute in attributes.iter_mut() {
        let key = attribute.key.as_str();
        if is_secret_key(key) {
            attribute.value = Value::from(REDACTED);
        } else if let Value::String(text) = &attribute.value {
            let text = text.as_str();
            let text = if URL_KEYS.contains(&key) {
                text.split(['?', '#']).next().unwrap_or_default()
            } else {
                text
            };
            attribute.value = Value::from(redact_text(text));
        }
    }
}

/// Runs `scrub` on `span`; if it panics, the span is exported stripped of its
/// name, attributes, events and links rather than raw (a redaction bug must
/// not leak a credential or take the export thread down).
fn scrub_or_strip(span: &mut SpanData, scrub: fn(&mut SpanData)) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scrub(span))).is_err() {
        span.name = REDACTED.into();
        span.attributes.clear();
        span.events.events.clear();
        span.links.links.clear();
    }
}

fn redact_span(span: &mut SpanData) {
    span.name = redact_text(&span.name).into();
    redact_attributes(&mut span.attributes);
    for event in &mut span.events.events {
        event.name = redact_text(&event.name).into();
        redact_attributes(&mut event.attributes);
    }
    for link in &mut span.links.links {
        redact_attributes(&mut link.attributes);
    }
    if let Status::Error { description } = &mut span.status {
        *description = redact_text(description).into();
    }
}

impl<E: SpanExporter> SpanExporter for RedactingSpanExporter<E> {
    fn export(&self, mut batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        for span in &mut batch {
            scrub_or_strip(span, redact_span);
        }
        self.0.export(batch)
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.0.shutdown_with_timeout(timeout)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.0.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.0.set_resource(resource);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{Link, Span as _, SpanContext, Tracer as _, TracerProvider as _};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    #[test]
    fn a_span_is_redacted_everywhere_before_it_is_exported() {
        let memory = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(RedactingSpanExporter(memory.clone()))
            .build();
        let tracer = provider.tracer("test");
        let link = Link::new(
            SpanContext::empty_context(),
            vec![KeyValue::new("token", "t")],
            0,
        );
        let mut span = tracer.build(
            tracer
                .span_builder("login password=name1")
                .with_links(vec![link]),
        );
        span.set_attribute(KeyValue::new("password", "hunter2"));
        span.set_attribute(KeyValue::new("api_key", 7_i64));
        span.set_attribute(KeyValue::new("note", "retry token=abc now"));
        span.set_attribute(KeyValue::new("attempts", 3_i64));
        span.set_attribute(KeyValue::new("http.target", "/x?token=abc"));
        span.set_attribute(KeyValue::new(
            "url.full",
            "https://h.example/x?token=abc#frag",
        ));
        span.set_attribute(KeyValue::new("url.path", "/x#frag"));
        span.set_attribute(KeyValue::new("http.url", "http://h.example/x?token=abc"));
        span.set_attribute(KeyValue::new("url.query", "token=abc"));
        span.add_event(
            "sent cookie=sid1",
            vec![KeyValue::new("Authorization", "Bearer x")],
        );
        span.set_status(Status::error("db down: postgres://app:pw@db/x"));
        span.end();
        provider.force_flush().unwrap();
        let spans = memory.get_finished_spans().unwrap();
        let span = &spans[0];
        assert_eq!(span.name, "login password=[REDACTED]");
        let get = |key: &str| {
            span.attributes
                .iter()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.to_string())
        };
        assert_eq!(get("password").as_deref(), Some("[REDACTED]"));
        assert_eq!(get("api_key").as_deref(), Some("[REDACTED]"));
        assert_eq!(get("note").as_deref(), Some("retry token=[REDACTED] now"));
        assert_eq!(get("attempts").as_deref(), Some("3"));
        assert_eq!(get("http.target").as_deref(), Some("/x"));
        assert_eq!(get("url.full").as_deref(), Some("https://h.example/x"));
        assert_eq!(get("url.path").as_deref(), Some("/x"));
        assert_eq!(get("http.url").as_deref(), Some("http://h.example/x"));
        assert_eq!(get("url.query"), None);
        let event = &span.events.events[0];
        assert_eq!(event.name, "sent cookie=[REDACTED]");
        assert_eq!(event.attributes[0].value.to_string(), "[REDACTED]");
        assert_eq!(
            span.links.links[0].attributes[0].value.to_string(),
            "[REDACTED]"
        );
        // A scrubber that panics leaves nothing readable behind.
        fn panics(_: &mut SpanData) {
            panic!("scrubber bug");
        }
        let mut stripped = span.clone();
        scrub_or_strip(&mut stripped, panics);
        assert_eq!(stripped.name, "[REDACTED]");
        assert!(stripped.attributes.is_empty());
        assert!(stripped.events.events.is_empty());
        assert!(stripped.links.links.is_empty());
        assert_eq!(
            span.status,
            Status::error("db down: postgres://[REDACTED]@db/x")
        );
        provider.shutdown().unwrap();
    }

    #[test]
    fn the_wrapper_forwards_the_exporter_lifecycle() {
        let mut exporter = RedactingSpanExporter(InMemorySpanExporter::default());
        exporter.force_flush().unwrap();
        exporter.set_resource(&Resource::builder_empty().build());
        exporter.shutdown().unwrap();
    }
}
