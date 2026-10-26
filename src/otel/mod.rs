//! OpenTelemetry traces and metrics behind the `otel` feature.
//!
//! [`configure_telemetry`](crate::logs::configure_telemetry) builds the
//! providers (and the logs bridge) from the standard `OTEL_*` environment
//! variables, as `lib-observability-py`'s `configure_telemetry` does:
//!
//! - nothing is set up unless `OTEL_EXPORTER_OTLP_ENDPOINT` (or the
//!   signal's own `OTEL_EXPORTER_OTLP_{TRACES,METRICS,LOGS}_ENDPOINT`) is
//!   non-empty, and `OTEL_SDK_DISABLED` is not `true`;
//! - `OTEL_TRACES_EXPORTER`, `OTEL_METRICS_EXPORTER` and `OTEL_LOGS_EXPORTER`
//!   set to `none` switch that one signal off (the SDK does not read them for
//!   hand-built providers);
//! - the SDK itself reads `OTEL_TRACES_SAMPLER` and `OTEL_TRACES_SAMPLER_ARG`
//!   (default `parentbased_always_on`), `OTEL_METRIC_EXPORT_INTERVAL`,
//!   `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` and the exporters'
//!   endpoint, header, protocol and timeout variables;
//! - all three signals share one `Resource`: `service.name` from
//!   `OTEL_SERVICE_NAME`/`OTEL_RESOURCE_ATTRIBUTES`, `service.version` from
//!   `APP_VERSION` unless `OTEL_RESOURCE_ATTRIBUTES` already sets it.
//!
//! [`ServerSpanLayer`] is the tower layer that opens a server span per
//! request and records `http.server.request.duration`; with the `postgres`
//! feature [`instrument_db`] adds connection-pool gauges.

#[cfg(feature = "postgres")]
mod db;
mod providers;
mod resource;
mod server;

#[cfg(feature = "postgres")]
pub use db::instrument_db;
pub(crate) use providers::{
    Providers, default_metric_exporter, default_span_exporter, meter_provider, register_globals,
    trace_layer, tracer_provider,
};
pub(crate) use resource::{Signal, process_env, shared_resource, signal_requested};
pub use server::{DEFAULT_EXCLUDED_PREFIXES, ResponseFuture, ServerSpanLayer, ServerSpanService};

/// The instrumentation scope name of this crate's tracer and meter.
pub(crate) const SCOPE: &str = "craftainer-observability";
