//! Vendor-neutral product analytics: [`track`] one event and let the
//! `ANALYTICS_*` environment pick where it goes (ADR 0007). Feature
//! `analytics`.
//!
//! ```
//! use craftainer_observability::analytics::{AnalyticsClient, AnalyticsConfig, RequestContext};
//!
//! // The default config has backend `none`: every call is a no-op.
//! let client = AnalyticsClient::new(AnalyticsConfig::default());
//! client.track("signup.completed", &[("plan", "pro".into())], Some("/signup?x=1"));
//!
//! // Wrap each request, so a Global Privacy Control request is not tracked.
//! // Sync code holds a guard; an async handler wraps its future.
//! let _guard = RequestContext { gpc: true, consent: false }.enter();
//! async fn handle(gpc: bool) {
//!     RequestContext { gpc, consent: false }
//!         .scope(async { craftainer_observability::analytics::track("page.view", &[], Some("/")) })
//!         .await;
//! }
//! ```
//!
//! - Event names match `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$` (at most 50
//!   characters) or the event is dropped.
//! - Properties are scalars ([`PropValue`]) whose keys are listed in
//!   `ANALYTICS_PROP_KEYS` (empty by default: nothing is sent unless the
//!   application opts each key in); others are dropped, strings are cut to 200
//!   characters. `url` is reduced to its path (query and fragment removed).
//! - Logs never contain property values, except the `log` and `otel`
//!   adapters, which emit the allowlisted ones by design.
//!
//! The process-wide functions ([`init`], [`track`], [`shutdown`]) wrap one
//! [`AnalyticsClient`]; build clients yourself for tests.

mod client;
mod config;
mod context;
mod emit;
mod umami;
mod validate;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

pub use client::AnalyticsClient;
pub use config::{AnalyticsConfig, Backend, Consent, Geo};
pub use context::{ContextGuard, RequestContext, Scoped, current as current_request_context};
pub use validate::{MAX_NAME, MAX_URL, MAX_VALUE, PropValue, valid_name};

use crate::logs::Telemetry;

/// One validated event.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Event {
    pub(crate) name: String,
    pub(crate) props: BTreeMap<String, PropValue>,
    pub(crate) url: String,
    pub(crate) timestamp: i64,
}

static CLIENT: Mutex<Option<Arc<AnalyticsClient>>> = Mutex::new(None);

/// Install the process-wide client for `config`, flushing the previous one.
/// Pass the [`Telemetry`] from `configure_telemetry` to use the `otel`
/// backend. Without `init`, the first [`track`] reads the environment
/// ([`AnalyticsConfig::from_env`]).
pub fn init(config: &AnalyticsConfig, telemetry: Option<&Telemetry>) {
    let client = Arc::new(telemetry.map_or_else(
        || AnalyticsClient::new(config.clone()),
        |t| AnalyticsClient::with_telemetry(config.clone(), t),
    ));
    let previous = CLIENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .replace(client);
    if let Some(previous) = previous {
        previous.shutdown();
    }
}

/// [`AnalyticsClient::track`] on the process-wide client.
pub fn track(name: &str, props: &[(&str, PropValue)], url: Option<&str>) {
    let client = CLIENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(|| Arc::new(AnalyticsClient::new(AnalyticsConfig::from_env())))
        .clone();
    client.track(name, props, url);
}

/// Flush and discard the process-wide client (blocking; see
/// [`AnalyticsClient::shutdown`]). Rust has no `atexit`: call it before the
/// process exits or the last queued events are lost. The next [`track`]
/// reads the environment again.
pub fn shutdown() {
    let client = CLIENT.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(client) = client {
        client.shutdown();
    }
}
