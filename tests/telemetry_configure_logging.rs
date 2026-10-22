//! `telemetry::configure_logging` installs a process-global
//! `tracing_subscriber`, so it can only be called once per process
//! (`src/telemetry.rs`'s own doc comment / ADR notes). Cargo builds every
//! file under `tests/` as its own process, so this file is the only place
//! it's ever called -- it never collides with any other test's logging.

#[test]
fn configure_logging_does_not_panic_and_logging_works_afterward() {
    template_axum::telemetry::configure_logging();
    tracing::info!("configure_logging smoke test");
}
