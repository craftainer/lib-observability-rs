//! Structured JSON logging -- the *only* log output shape (ADR 0007), port
//! of `telemetry.py`'s `configure_logging()`. No tracing/metrics
//! instrumentation, deliberately -- logs only.

/// Attaches a JSON-formatting subscriber to every `tracing` call
/// (including `tower-http`'s `TraceLayer` request logs) -- called once,
/// at process startup, before anything else logs.
pub fn configure_logging() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}
