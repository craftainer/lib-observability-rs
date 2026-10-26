//! The deprecated `logs::configure_logging` (now `configure_telemetry` under
//! its old name) installs a process-global `tracing` subscriber,
//! so it can only succeed once per process. Cargo builds every file under
//! `tests/` as its own process, so this file is the only place it is called
//! and it never collides with another test's logging.

// The point of this file is the deprecated names, which must keep working.
#![expect(deprecated, reason = "tests the deprecated shim")]

use craftainer_observability::Error;
use craftainer_observability::logs::configure_logging;

#[test]
fn configure_logging_installs_once_and_refuses_a_second_time() {
    let guard = configure_logging().expect("the first call installs the subscriber");
    tracing::info!("configure_logging smoke test");
    guard
        .force_flush()
        .expect("nothing is exported, so nothing to flush");
    assert!(matches!(
        configure_logging(),
        Err(Error::SubscriberAlreadyInstalled)
    ));
    assert!(matches!(
        craftainer_observability::logs::configure_logging_without_export(),
        Err(Error::SubscriberAlreadyInstalled)
    ));
    guard.shutdown().expect("shutdown");
}
