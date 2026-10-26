//! Connection-pool gauges for an `sqlx::PgPool` (features `otel` and
//! `postgres`).

use opentelemetry::metrics::Meter;
use opentelemetry::{KeyValue, global};
use sqlx::PgPool;

use super::SCOPE;

/// Registers observable gauges for `pool` on the process-wide meter provider
/// (set by [`configure_telemetry`](crate::logs::configure_telemetry), so call
/// this after it). They are read at each metric export, not on every query:
///
/// - `db.client.connections.usage` (`{connection}`), with `state` `idle` or
///   `used`: the same name and attribute `lib-observability-py` gets from
///   `SQLAlchemy`'s instrumentation;
/// - `db.client.connections.max` (`{connection}`): the pool's configured
///   maximum.
///
/// Call it once per pool. Without telemetry the gauges are no-ops.
pub fn instrument_db(pool: &PgPool) {
    instrument_pool(&global::meter(SCOPE), pool);
}

/// [`instrument_db`] on an explicit `meter`.
fn instrument_pool(meter: &Meter, pool: &PgPool) {
    let usage_pool = pool.clone();
    meter
        .u64_observable_gauge("db.client.connections.usage")
        .with_unit("{connection}")
        .with_description("Connections in the pool, by state.")
        .with_callback(move |observer| {
            let idle = u32::try_from(usage_pool.num_idle()).unwrap_or(u32::MAX);
            let used = usage_pool.size().saturating_sub(idle);
            observer.observe(u64::from(idle), &[KeyValue::new("state", "idle")]);
            observer.observe(u64::from(used), &[KeyValue::new("state", "used")]);
        })
        .build();
    let max_pool = pool.clone();
    meter
        .u64_observable_gauge("db.client.connections.max")
        .with_unit("{connection}")
        .with_description("The maximum number of connections the pool allows.")
        .with_callback(move |observer| {
            observer.observe(u64::from(max_pool.options().get_max_connections()), &[]);
        })
        .build();
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics as A, MetricData as D};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
    use sqlx::postgres::PgPoolOptions;

    #[tokio::test]
    async fn the_pool_gauges_report_state_and_maximum() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        // Lazy: nothing connects, so the pool is empty with a maximum of 7.
        let pool = PgPoolOptions::new()
            .max_connections(7)
            .connect_lazy("postgres://user:pw@127.0.0.1:1/none")
            .unwrap();
        instrument_pool(&provider.meter("test"), &pool);
        provider.force_flush().unwrap();

        let mut seen = Vec::new();
        for resource_metrics in exporter.get_finished_metrics().unwrap() {
            for scope in resource_metrics.scope_metrics() {
                for metric in scope.metrics() {
                    let d = metric.data();
                    let A::U64(D::Gauge(gauge)) = d else { panic!() };
                    for point in gauge.data_points() {
                        let state = point
                            .attributes()
                            .find(|kv| kv.key.as_str() == "state")
                            .map(|kv| kv.value.to_string());
                        seen.push((metric.name().to_owned(), state, point.value()));
                    }
                }
            }
        }
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("db.client.connections.max".to_owned(), None, 7),
                (
                    "db.client.connections.usage".to_owned(),
                    Some("idle".to_owned()),
                    0
                ),
                (
                    "db.client.connections.usage".to_owned(),
                    Some("used".to_owned()),
                    0
                ),
            ]
        );
    }

    #[tokio::test]
    async fn instrument_db_uses_the_global_meter() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user:pw@127.0.0.1:1/none")
            .unwrap();
        instrument_db(&pool);
    }
}
