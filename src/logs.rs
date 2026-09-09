//! Structured JSON logging -- the *only* unconditional log output shape
//! (ADR 0006), port of `telemetry.py`'s `configure_logging()`. No
//! tracing/metrics instrumentation, deliberately -- logs only.
//!
//! Optional OTLP log export (FR-0035, ADR 0006's own deferred note): when
//! `OTEL_EXPORTER_OTLP_ENDPOINT` (or its logs-specific override,
//! `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`) is set, a second `tracing_subscriber::
//! Layer` forwards every `tracing` event to an OTLP log collector over
//! HTTP, via a batch processor -- additive to, never a replacement for,
//! the JSON stdout output above. No new app setting: this reads
//! OpenTelemetry's own env vars directly, the same convention every OTLP
//! SDK uses, rather than adding an app-specific `Settings` field for
//! something already externally standardized.

use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLoggerProvider};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Attaches a JSON-formatting subscriber to every `tracing` call
/// (including `tower-http`'s `TraceLayer` request logs) -- called once,
/// at process startup, before anything else logs. Also attaches the OTLP
/// layer `otlp_log_layer` builds, when the environment asks for one.
pub fn configure_logging() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let json_layer = tracing_subscriber::fmt::layer().json();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(json_layer)
        .with(otlp_log_layer())
        .init();
}

/// `None` unless the environment sets an OTLP logs endpoint -- this app
/// never defaults to exporting logs to `localhost:4318` the way the OTLP
/// exporter's own fallback would, since that default exists for a
/// collector that, for most deployments of this app, isn't there.
/// `SdkLoggerProvider`'s handle is dropped at the end of this function;
/// that's safe -- the `Logger` the returned layer holds keeps its own
/// cloned (`Arc`-backed) reference to the same provider internals, so the
/// batch processor keeps running for the life of the process either way.
fn otlp_log_layer(
) -> Option<OpenTelemetryTracingBridge<SdkLoggerProvider, opentelemetry_sdk::logs::SdkLogger>> {
    let endpoint_configured = std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some()
        || std::env::var_os("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT").is_some();
    if !endpoint_configured {
        return None;
    }

    let exporter = match opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .build()
    {
        Ok(exporter) => exporter,
        Err(err) => {
            eprintln!("OTLP log exporter setup failed, continuing without it: {err}");
            return None;
        }
    };
    let provider = SdkLoggerProvider::builder()
        .with_log_processor(BatchLogProcessor::builder(exporter).build())
        .build();
    Some(OpenTelemetryTracingBridge::new(&provider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `std::env::set_var`/`remove_var` mutate whole-process state, which
    // Rust's test harness runs in parallel by default -- serialized the
    // same way `config.rs`'s own `ENV_LOCK` is (`tests/README.md`'s "Do").
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn otlp_log_layer_is_gated_on_the_otlp_endpoint_env_vars() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: this test only reads/sets/removes env vars it also
        // cleans up itself, under the lock above; `configure_logging` (the
        // only other caller) runs once in `main()`, never in this test
        // process.
        unsafe {
            std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
            std::env::remove_var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT");
        }
        assert!(
            otlp_log_layer().is_none(),
            "no layer should be built with neither env var set"
        );

        unsafe {
            std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4318");
        }
        let layer = otlp_log_layer();
        unsafe {
            std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        }
        assert!(
            layer.is_some(),
            "a layer should be built once OTEL_EXPORTER_OTLP_ENDPOINT is set"
        );
    }
}
