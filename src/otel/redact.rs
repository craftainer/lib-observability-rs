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

/// `attributes` with secret values replaced and string values scrubbed.
fn redact_attributes(attributes: &mut [KeyValue]) {
    for attribute in attributes {
        if is_secret_key(attribute.key.as_str()) {
            attribute.value = Value::from(REDACTED);
        } else if let Value::String(text) = &attribute.value {
            attribute.value = Value::from(redact_text(text.as_str()));
        }
    }
}

fn redact_span(span: &mut SpanData) {
    span.name = redact_text(&span.name).into();
    redact_attributes(&mut span.attributes);
    for event in &mut span.events.events {
        event.name = redact_text(&event.name).into();
        redact_attributes(&mut event.attributes);
    }
    if let Status::Error { description } = &mut span.status {
        *description = redact_text(description).into();
    }
}

impl<E: SpanExporter> SpanExporter for RedactingSpanExporter<E> {
    fn export(&self, mut batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        batch.iter_mut().for_each(redact_span);
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
    use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    #[test]
    fn a_span_is_redacted_everywhere_before_it_is_exported() {
        let memory = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(RedactingSpanExporter(memory.clone()))
            .build();
        let tracer = provider.tracer("test");
        let mut span = tracer.start("login password=name1");
        span.set_attribute(KeyValue::new("password", "hunter2"));
        span.set_attribute(KeyValue::new("api_key", 7_i64));
        span.set_attribute(KeyValue::new("note", "retry token=abc now"));
        span.set_attribute(KeyValue::new("attempts", 3_i64));
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
        let event = &span.events.events[0];
        assert_eq!(event.name, "sent cookie=[REDACTED]");
        assert_eq!(event.attributes[0].value.to_string(), "[REDACTED]");
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
