//! `configure_telemetry` installs a process-global `tracing` subscriber, so
//! it can only succeed once per process. Cargo builds every file under
//! `tests/` as its own process, so this file is the only place it is called.
//! The test environment sets no `OTEL_*` variable, so nothing is exported.

use craftainer_observability::{Error, configure_telemetry};

#[test]
fn nothing_is_exported_without_an_endpoint_and_a_second_call_is_refused() {
    let telemetry = configure_telemetry().expect("the first call installs the subscriber");
    assert!(
        !telemetry.is_exporting(),
        "no exporter without OTEL_EXPORTER_OTLP_ENDPOINT"
    );
    tracing::info!(api_token = "hidden", "configure_telemetry smoke test");
    telemetry
        .force_flush()
        .expect("nothing is exported, so nothing to flush");
    assert!(matches!(
        configure_telemetry(),
        Err(Error::SubscriberAlreadyInstalled)
    ));
    assert!(matches!(
        craftainer_observability::logs::configure_telemetry_without_export(),
        Err(Error::SubscriberAlreadyInstalled)
    ));
    telemetry.shutdown().expect("shutdown");
    telemetry.shutdown().expect("a second shutdown is harmless");
}
