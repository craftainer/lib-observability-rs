//! The in-process adapters: `log` (a `tracing` event) and `otel` (an
//! OpenTelemetry log record).

use super::Event;
#[cfg(feature = "otel")]
use super::validate::PropValue;

/// The target of `log` events.
pub(crate) const LOG_TARGET: &str = "craftainer_observability::analytics::events";
/// The target (instrumentation scope) of `otel` events.
#[cfg(feature = "otel")]
pub(crate) const OTEL_TARGET: &str = "craftainer_observability::analytics::otel";

/// One `tracing` INFO event with the fixed fields `analytics_event`,
/// `analytics_url` and `analytics_props` (the allowlisted properties as one
/// JSON object in a string). `tracing` field names are static, so the keys of
/// the allowlist cannot be fields of their own here; the `otel` adapter, which
/// is not bound by that, emits one attribute per property.
pub(crate) fn log(event: &Event) {
    let props = serde_json::to_string(&event.props).unwrap_or_default();
    tracing::info!(
        target: LOG_TARGET,
        analytics_event = %event.name,
        analytics_url = %event.url,
        analytics_props = %props,
        "analytics_event"
    );
}

/// Emits OpenTelemetry log records through a logger of a provider shared
/// with the rest of the process's telemetry.
#[cfg(feature = "otel")]
pub(crate) struct OtelSink {
    logger: opentelemetry_sdk::logs::SdkLogger,
}

#[cfg(feature = "otel")]
impl OtelSink {
    pub(crate) fn new(provider: &opentelemetry_sdk::logs::SdkLoggerProvider) -> Self {
        use opentelemetry::logs::LoggerProvider;
        Self {
            logger: provider.logger(crate::otel::SCOPE),
        }
    }

    /// A record with the attributes `analytics_event`, `analytics_url` and
    /// one primitive `analytics_prop_<key>` per property (`OTel` attributes
    /// must be primitives, never a map).
    pub(crate) fn emit(&self, event: &Event) {
        use opentelemetry::logs::{AnyValue, LogRecord, Logger, Severity};
        let mut record = self.logger.create_log_record();
        record.set_event_name("analytics_event");
        record.set_target(OTEL_TARGET);
        record.set_timestamp(std::time::SystemTime::now());
        record.set_severity_number(Severity::Info);
        record.set_severity_text("INFO");
        record.set_body("analytics_event".into());
        record.add_attribute("analytics_event", event.name.clone());
        record.add_attribute("analytics_url", event.url.clone());
        for (key, value) in &event.props {
            let value = match value {
                PropValue::Str(s) => AnyValue::from(s.clone()),
                PropValue::Int(i) => AnyValue::from(*i),
                PropValue::Uint(u) => {
                    i64::try_from(*u).map_or_else(|_| AnyValue::from(u.to_string()), AnyValue::from)
                }
                PropValue::Float(f) => AnyValue::from(*f),
                PropValue::Bool(b) => AnyValue::from(*b),
            };
            record.add_attribute(format!("analytics_prop_{key}"), value);
        }
        self.logger.emit(record);
    }
}
