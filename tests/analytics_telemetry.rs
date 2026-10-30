//! `analytics::init` with the `Telemetry` handle, in a process of its own
//! (`configure_telemetry_without_export` installs the global subscriber).
//! Nothing is exported here, so the `otel` backend has no logger provider and
//! falls back to off; the provider-present path is a unit test.

use craftainer_observability::analytics::{self, AnalyticsConfig, Backend};
use craftainer_observability::logs::configure_telemetry_without_export;

#[test]
fn the_otel_backend_without_exported_logs_is_off() {
    let telemetry = configure_telemetry_without_export().unwrap();
    analytics::init(
        &AnalyticsConfig {
            backend: Backend::Otel,
            ..AnalyticsConfig::default()
        },
        Some(&telemetry),
    );
    analytics::track("page.view", &[], Some("/"));
    analytics::shutdown();
}
