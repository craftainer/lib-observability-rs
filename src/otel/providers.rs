//! The tracer and meter providers, the trace layer, and the handle that
//! flushes every provider.

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{ExporterBuildError, MetricExporter, SpanExporter};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use tracing::Metadata;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::Layer as _;
use tracing_subscriber::filter::{FilterFn, Filtered};

use super::SCOPE;
use crate::Error;
use crate::logs::otlp::exportable;

/// The OTLP/HTTP span exporter, configured from the standard `OTEL_*`
/// environment variables. Building it opens no connection.
pub(crate) fn default_span_exporter() -> Result<SpanExporter, ExporterBuildError> {
    SpanExporter::builder().with_http().build()
}

/// The OTLP/HTTP metric exporter, configured like [`default_span_exporter`].
pub(crate) fn default_metric_exporter() -> Result<MetricExporter, ExporterBuildError> {
    MetricExporter::builder().with_http().build()
}

/// A second shutdown is harmless, so "already shut down" is not an error.
pub(crate) fn shutdown_result(result: OTelSdkResult) -> Result<(), Error> {
    match result {
        Ok(()) | Err(OTelSdkError::AlreadyShutdown) => Ok(()),
        Err(err) => Err(Error::Flush(err.to_string())),
    }
}

/// The tracer provider, or `None` unless `requested`. The sampler comes from
/// the SDK's own reading of `OTEL_TRACES_SAMPLER` and
/// `OTEL_TRACES_SAMPLER_ARG`. The exporter is a function pointer so tests
/// need no collector (and the function has one instantiation).
///
/// # Errors
///
/// [`Error::ExporterBuild`] when export is requested and the exporter cannot
/// be built.
pub(crate) fn tracer_provider(
    requested: bool,
    exporter: fn() -> Result<SpanExporter, ExporterBuildError>,
    resource: &Resource,
) -> Result<Option<SdkTracerProvider>, Error> {
    if !requested {
        return Ok(None);
    }
    let exporter = exporter().map_err(|err| Error::ExporterBuild(format!("traces: {err}")))?;
    Ok(Some(
        SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(exporter)
            .build(),
    ))
}

/// The meter provider, or `None` unless `requested`. The export interval
/// comes from the SDK's reading of `OTEL_METRIC_EXPORT_INTERVAL`.
///
/// # Errors
///
/// [`Error::ExporterBuild`] when export is requested and the exporter cannot
/// be built.
pub(crate) fn meter_provider(
    requested: bool,
    exporter: fn() -> Result<MetricExporter, ExporterBuildError>,
    resource: &Resource,
) -> Result<Option<SdkMeterProvider>, Error> {
    if !requested {
        return Ok(None);
    }
    let exporter = exporter().map_err(|err| Error::ExporterBuild(format!("metrics: {err}")))?;
    Ok(Some(
        SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .with_reader(PeriodicReader::builder(exporter).build())
            .build(),
    ))
}

/// The trace layer's type: [`trace_layer`] filtered like the logs bridge.
pub(crate) type TraceLayer<S> =
    Filtered<OpenTelemetryLayer<S, SdkTracer>, FilterFn<fn(&Metadata<'_>) -> bool>, S>;

/// The `tracing` layer that turns spans into OpenTelemetry spans of
/// `provider`. Spans of the exporter's own HTTP stack are left out, as they
/// are from the logs bridge: exporting a batch would otherwise create spans
/// that go into the next batch.
pub(crate) fn trace_layer<S>(provider: &SdkTracerProvider) -> TraceLayer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let filter: FilterFn<fn(&Metadata<'_>) -> bool> = FilterFn::new(exportable);
    tracing_opentelemetry::layer()
        .with_tracer(provider.tracer(SCOPE))
        .with_filter(filter)
}

/// Makes the providers the process-wide OpenTelemetry ones (so code that
/// uses `opentelemetry::global`, such as [`ServerSpanLayer::new`], finds
/// them) and `traceparent` the propagation format.
///
/// [`ServerSpanLayer::new`]: super::ServerSpanLayer::new
pub(crate) fn register_globals(
    tracer: Option<&SdkTracerProvider>,
    meter: Option<&SdkMeterProvider>,
) {
    if let Some(tracer) = tracer {
        global::set_tracer_provider(tracer.clone());
    }
    if let Some(meter) = meter {
        global::set_meter_provider(meter.clone());
    }
    if tracer.is_some() || meter.is_some() {
        global::set_text_map_propagator(TraceContextPropagator::new());
    }
}

/// Every provider [`configure_telemetry`](crate::logs::configure_telemetry)
/// built, so one call flushes or stops them all.
#[derive(Default)]
pub(crate) struct Providers {
    pub(crate) logger: Option<SdkLoggerProvider>,
    pub(crate) tracer: Option<SdkTracerProvider>,
    pub(crate) meter: Option<SdkMeterProvider>,
}

impl Providers {
    /// Whether nothing is exported.
    pub(crate) fn is_empty(&self) -> bool {
        self.logger.is_none() && self.tracer.is_none() && self.meter.is_none()
    }

    /// Exports everything buffered. Every provider is tried; the first error
    /// is returned.
    pub(crate) fn force_flush(&self) -> Result<(), Error> {
        let results = [
            self.logger.as_ref().map(SdkLoggerProvider::force_flush),
            self.tracer.as_ref().map(SdkTracerProvider::force_flush),
            self.meter.as_ref().map(SdkMeterProvider::force_flush),
        ];
        results
            .into_iter()
            .flatten()
            .try_for_each(|result| result.map_err(|err| Error::Flush(err.to_string())))
    }

    /// Flushes and stops everything. Every provider is stopped; the first
    /// error is returned. Repeating it is harmless.
    pub(crate) fn shutdown(&self) -> Result<(), Error> {
        let results = [
            self.logger.as_ref().map(SdkLoggerProvider::shutdown),
            self.tracer.as_ref().map(SdkTracerProvider::shutdown),
            self.meter.as_ref().map(SdkMeterProvider::shutdown),
        ];
        results.into_iter().flatten().try_for_each(shutdown_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otel::resource::tests::env_of;
    use crate::otel::shared_resource;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::logs::InMemoryLogExporter;
    use opentelemetry_sdk::metrics::InMemoryMetricExporter;
    use opentelemetry_sdk::trace::InMemorySpanExporter;
    use tracing_subscriber::layer::SubscriberExt;

    fn local_span_exporter() -> Result<SpanExporter, ExporterBuildError> {
        SpanExporter::builder()
            .with_http()
            .with_endpoint("http://localhost:4318/v1/traces")
            .build()
    }

    fn local_metric_exporter() -> Result<MetricExporter, ExporterBuildError> {
        MetricExporter::builder()
            .with_http()
            .with_endpoint("http://localhost:4318/v1/metrics")
            .build()
    }

    fn failing<T>() -> Result<T, ExporterBuildError> {
        Err(ExporterBuildError::InvalidConfig {
            name: "endpoint".to_string(),
            reason: "rejected for the test".to_string(),
        })
    }

    #[test]
    fn nothing_is_built_unless_requested() {
        let resource = shared_resource(&env_of(&[]));
        // The exporters would fail if they were called.
        assert!(matches!(
            tracer_provider(false, failing, &resource),
            Ok(None)
        ));
        assert!(matches!(
            meter_provider(false, failing, &resource),
            Ok(None)
        ));
    }

    #[tokio::test]
    async fn the_default_exporters_build_without_a_collector() {
        assert!(default_span_exporter().is_ok());
        assert!(default_metric_exporter().is_ok());
    }

    #[tokio::test]
    async fn providers_are_built_once_requested() {
        let resource = shared_resource(&env_of(&[("APP_VERSION", "9")]));
        let tracer = tracer_provider(true, local_span_exporter, &resource).unwrap();
        let meter = meter_provider(true, local_metric_exporter, &resource).unwrap();
        let providers = Providers {
            logger: None,
            tracer,
            meter,
        };
        assert!(!providers.is_empty());
        assert!(providers.tracer.is_some() && providers.meter.is_some());
        // Nothing was recorded, so the shutdown has nothing to send.
        providers.shutdown().unwrap();
        providers.shutdown().unwrap();
    }

    #[test]
    fn an_unbuildable_exporter_is_an_error_naming_the_signal() {
        let resource = shared_resource(&env_of(&[]));
        let err = tracer_provider(true, failing, &resource).unwrap_err();
        assert!(
            matches!(&err, Error::ExporterBuild(m) if m.starts_with("traces:") && m.contains("rejected")),
            "{err}"
        );
        let err = meter_provider(true, failing, &resource).unwrap_err();
        assert!(
            matches!(&err, Error::ExporterBuild(m) if m.starts_with("metrics:")),
            "{err}"
        );
    }

    #[test]
    fn only_a_real_shutdown_failure_is_an_error() {
        assert!(shutdown_result(Ok(())).is_ok());
        assert!(shutdown_result(Err(OTelSdkError::AlreadyShutdown)).is_ok());
        let err =
            shutdown_result(Err(OTelSdkError::InternalFailure("boom".to_string()))).unwrap_err();
        assert!(matches!(err, Error::Flush(m) if m.contains("boom")));
    }

    #[test]
    fn spans_reach_the_exporter_through_the_trace_layer() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(trace_layer(&provider));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("work").in_scope(|| ());
        });
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "work");
    }

    #[test]
    fn the_exporters_own_spans_are_not_exported() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(trace_layer(&provider));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!(target: "hyper::client", "from the stack").in_scope(|| ());
            tracing::info_span!(target: "myapp", "from the app").in_scope(|| ());
        });
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "from the app");
    }

    #[test]
    fn an_empty_set_of_providers_does_nothing() {
        let providers = Providers::default();
        assert!(providers.is_empty());
        providers.force_flush().unwrap();
        providers.shutdown().unwrap();
    }

    #[test]
    fn every_provider_is_flushed_and_stopped() {
        let logs = InMemoryLogExporter::default();
        let spans = InMemorySpanExporter::default();
        let metrics = InMemoryMetricExporter::default();
        let providers = Providers {
            logger: Some(
                SdkLoggerProvider::builder()
                    .with_simple_exporter(logs)
                    .build(),
            ),
            tracer: Some(
                SdkTracerProvider::builder()
                    .with_simple_exporter(spans)
                    .build(),
            ),
            meter: Some(
                SdkMeterProvider::builder()
                    .with_reader(PeriodicReader::builder(metrics).build())
                    .build(),
            ),
        };
        providers.force_flush().unwrap();
        providers.shutdown().unwrap();
        providers.shutdown().unwrap();
        // After a shutdown a flush is refused, and the error says so.
        assert!(matches!(providers.force_flush(), Err(Error::Flush(_))));
    }

    #[test]
    fn globals_are_registered() {
        let spans = InMemorySpanExporter::default();
        let tracer = SdkTracerProvider::builder()
            .with_simple_exporter(spans)
            .build();
        let meter = SdkMeterProvider::builder().build();
        register_globals(Some(&tracer), Some(&meter));
        register_globals(None, None);
    }
}
