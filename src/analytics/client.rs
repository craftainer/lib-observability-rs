//! `AnalyticsClient`: validation, the request gate and the adapters.

use std::time::{SystemTime, UNIX_EPOCH};

use super::config::{AnalyticsConfig, Backend, Consent};
use super::context::{self, RequestContext};
use super::umami::{Queue, Settings};
use super::validate::{PropValue, clean_props, clean_url, valid_name};
use super::{Event, emit};
use crate::logs::Telemetry;

/// The OpenTelemetry logger provider type (uninhabited without the `otel`
/// feature, where the `otel` backend is unavailable).
#[cfg(feature = "otel")]
pub(crate) type Provider = opentelemetry_sdk::logs::SdkLoggerProvider;
#[cfg(not(feature = "otel"))]
pub(crate) type Provider = std::convert::Infallible;

enum Sink {
    None,
    Log,
    #[cfg(feature = "otel")]
    Otel(emit::OtelSink),
    Umami(Queue),
}

/// Validates events and hands them to the configured adapter without ever
/// raising, panicking or blocking on the network.
///
/// `log` and `otel` events are emitted inline. `umami` events go through a
/// queue of 1000 drained by one background thread every 5 s (see
/// [`shutdown`](Self::shutdown)).
pub struct AnalyticsClient {
    config: AnalyticsConfig,
    sink: Sink,
}

impl AnalyticsClient {
    /// A client for `config`. The `otel` backend needs a logger provider:
    /// use [`with_telemetry`](Self::with_telemetry); here it warns and falls
    /// back to `none`.
    #[must_use]
    pub fn new(config: AnalyticsConfig) -> Self {
        Self::build(config, None, None)
    }

    /// A client for `config` whose `otel` backend emits through the logger
    /// provider of `telemetry` (the one handle
    /// [`configure_telemetry`](crate::configure_telemetry) returned), so
    /// analytics events share its resource and exporter. Without the `otel`
    /// feature, or when `telemetry` exports no logs, the `otel` backend warns
    /// and falls back to `none`.
    #[must_use]
    pub fn with_telemetry(config: AnalyticsConfig, telemetry: &Telemetry) -> Self {
        #[cfg(feature = "otel")]
        let provider = telemetry.logger_provider().cloned();
        #[cfg(not(feature = "otel"))]
        let provider = {
            let _ = telemetry;
            None
        };
        Self::build(config, provider, None)
    }

    /// The `otel` backend over an explicit logger provider (a test seam).
    #[cfg(feature = "otel")]
    #[must_use]
    pub fn with_logger_provider(
        config: AnalyticsConfig,
        provider: opentelemetry_sdk::logs::SdkLoggerProvider,
    ) -> Self {
        Self::build(config, Some(provider), None)
    }

    pub(crate) fn build(
        config: AnalyticsConfig,
        provider: Option<Provider>,
        tune: Option<&dyn Fn(&mut Settings)>,
    ) -> Self {
        let sink = if config.enabled {
            match config.backend {
                Backend::None => Sink::None,
                Backend::Log => Sink::Log,
                Backend::Otel => otel_sink(provider),
                Backend::Umami => umami_sink(&config, tune),
            }
        } else {
            Sink::None
        };
        Self { config, sink }
    }

    /// Which adapter is active (for tests).
    #[cfg(test)]
    pub(crate) fn sink_name(&self) -> &'static str {
        match self.sink {
            Sink::None => "none",
            Sink::Log => "log",
            #[cfg(feature = "otel")]
            Sink::Otel(_) => "otel",
            Sink::Umami(_) => "umami",
        }
    }

    /// Whether the current request context allows tracking.
    fn allowed(&self, context: Option<RequestContext>) -> bool {
        match context {
            None => self.config.consent != Consent::OptIn,
            Some(ctx) => {
                let refused_by_gpc = self.config.respect_gpc && ctx.gpc;
                let lacks_consent = self.config.consent == Consent::OptIn && !ctx.consent;
                !refused_by_gpc && !lacks_consent
            }
        }
    }

    /// Record one event (contract I3). An invalid name drops the event; a
    /// property off the `ANALYTICS_PROP_KEYS` allowlist drops that property;
    /// `url` is reduced to its path. Inside a [`RequestContext`] with GPC set
    /// (and `ANALYTICS_RESPECT_GPC` true), or without consent in `opt-in`
    /// mode, this does nothing. Never fails, panics or waits for the network.
    pub fn track(&self, name: &str, props: &[(&str, PropValue)], url: Option<&str>) {
        if !self.allowed(context::current()) {
            return;
        }
        if !valid_name(name) {
            tracing::warn!("analytics event dropped: invalid name");
            return;
        }
        let event = Event {
            name: name.to_owned(),
            props: clean_props(props, &self.config.prop_keys),
            url: clean_url(url),
            timestamp: unix_seconds(),
        };
        match &self.sink {
            Sink::None => {}
            Sink::Log => emit::log(&event),
            #[cfg(feature = "otel")]
            Sink::Otel(sink) => sink.emit(&event),
            Sink::Umami(queue) => queue.push(event),
        }
    }

    /// Flush what is queued and stop the background thread. It blocks until
    /// the flush is done (a dead Umami costs one 2 s timeout), so in async
    /// code call it through `tokio::task::spawn_blocking` or after the server
    /// has stopped. Calling it again, or dropping the client, is harmless;
    /// events tracked afterwards are dropped.
    pub fn shutdown(&self) {
        if let Sink::Umami(queue) = &self.sink {
            queue.shutdown();
        }
    }
}

impl Drop for AnalyticsClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(feature = "otel")]
fn otel_sink(provider: Option<Provider>) -> Sink {
    if let Some(p) = provider {
        Sink::Otel(emit::OtelSink::new(&p))
    } else {
        tracing::warn!(
            "ANALYTICS_BACKEND=otel needs OTLP log export from configure_telemetry; off"
        );
        Sink::None
    }
}

#[cfg(not(feature = "otel"))]
fn otel_sink(_provider: Option<Provider>) -> Sink {
    tracing::warn!("ANALYTICS_BACKEND=otel needs the `otel` feature; off");
    Sink::None
}

/// Seconds since the Unix epoch (0 if the clock is before it).
fn unix_seconds() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    i64::try_from(secs).unwrap_or(i64::MAX)
}

/// The Umami sink; `tune` is the tests' way to shorten the timings.
fn umami_sink(config: &AnalyticsConfig, tune: Option<&dyn Fn(&mut Settings)>) -> Sink {
    let settings = if config.endpoint.is_empty() || config.site_id.is_empty() {
        None
    } else {
        Settings::new(&config.endpoint, &config.site_id, &config.hostname)
    };
    if let Some(mut settings) = settings {
        if let Some(tune) = tune {
            tune(&mut settings);
        }
        Sink::Umami(Queue::new(settings))
    } else {
        tracing::warn!(
            "ANALYTICS_BACKEND=umami needs a valid http(s) ANALYTICS_ENDPOINT and ANALYTICS_SITE_ID; off"
        );
        Sink::None
    }
}
