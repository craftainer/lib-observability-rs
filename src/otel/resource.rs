//! What the environment asks for: which signals export, and the shared
//! `Resource`.

use opentelemetry::{Key, KeyValue};
use opentelemetry_sdk::Resource;

/// One of the three telemetry signals.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Signal {
    Logs,
    Traces,
    Metrics,
}

impl Signal {
    /// The signal's name as it appears in `OTEL_*` variable names.
    fn upper(self) -> &'static str {
        match self {
            Self::Logs => "LOGS",
            Self::Traces => "TRACES",
            Self::Metrics => "METRICS",
        }
    }
}

/// The process environment, as the lookup function the decisions take.
pub(crate) fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Whether the environment asks for OTLP export of `signal`: a non-empty
/// endpoint (the signal's own, else the general one), and neither
/// `OTEL_SDK_DISABLED=true` nor `OTEL_<SIGNAL>_EXPORTER` containing `none`,
/// as in `lib-observability-py` (which reads only the general endpoint).
pub(crate) fn signal_requested(env: &dyn Fn(&str) -> Option<String>, signal: Signal) -> bool {
    let upper = signal.upper();
    // The signal's own endpoint, even when empty, wins over the general one,
    // as in the OpenTelemetry spec (an empty value is how to opt one out).
    let endpoint = env(&format!("OTEL_EXPORTER_OTLP_{upper}_ENDPOINT"))
        .or_else(|| env("OTEL_EXPORTER_OTLP_ENDPOINT"));
    if endpoint.is_none_or(|value| value.is_empty()) {
        return false;
    }
    if env("OTEL_SDK_DISABLED").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) {
        return false;
    }
    !env(&format!("OTEL_{upper}_EXPORTER")).is_some_and(|v| {
        v.split(',')
            .any(|name| name.trim().eq_ignore_ascii_case("none"))
    })
}

/// The one `Resource` all three signals share. The SDK's detectors supply
/// `service.name` (`OTEL_SERVICE_NAME`, else `unknown_service:<exe>`) and
/// `OTEL_RESOURCE_ATTRIBUTES`; `service.version` comes from `APP_VERSION`
/// unless `OTEL_RESOURCE_ATTRIBUTES` already sets it (the variable wins).
pub(crate) fn shared_resource(env: &dyn Fn(&str) -> Option<String>) -> Resource {
    let base = Resource::builder().build();
    match env("APP_VERSION").filter(|version| !version.is_empty()) {
        Some(version) if base.get(&Key::from_static_str("service.version")).is_none() => {
            Resource::builder()
                .with_attribute(KeyValue::new("service.version", version))
                .build()
        }
        _ => base,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    pub(crate) fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    const SIGNALS: [Signal; 3] = [Signal::Logs, Signal::Traces, Signal::Metrics];

    #[test]
    fn nothing_is_requested_without_an_endpoint() {
        for signal in SIGNALS {
            assert!(!signal_requested(&env_of(&[]), signal));
            assert!(!signal_requested(
                &env_of(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "")]),
                signal
            ));
            assert!(signal_requested(
                &env_of(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318")]),
                signal
            ));
        }
    }

    #[test]
    fn a_signals_own_endpoint_wins_even_when_empty() {
        let general = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318");
        assert!(signal_requested(
            &env_of(&[("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://c/v1/traces")]),
            Signal::Traces
        ));
        assert!(!signal_requested(
            &env_of(&[("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://c/v1/traces")]),
            Signal::Metrics
        ));
        assert!(!signal_requested(
            &env_of(&[general, ("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", "")]),
            Signal::Metrics
        ));
        assert!(signal_requested(
            &env_of(&[general, ("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", "")]),
            Signal::Traces
        ));
    }

    #[test]
    fn export_can_be_switched_off() {
        let on = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318");
        for signal in SIGNALS {
            assert!(!signal_requested(
                &env_of(&[on, ("OTEL_SDK_DISABLED", " TRUE ")]),
                signal
            ));
            assert!(signal_requested(
                &env_of(&[on, ("OTEL_SDK_DISABLED", "false")]),
                signal
            ));
        }
        let off = |name: &'static str| env_of(&[on, (name, "console, None")]);
        assert!(!signal_requested(&off("OTEL_LOGS_EXPORTER"), Signal::Logs));
        assert!(signal_requested(&off("OTEL_LOGS_EXPORTER"), Signal::Traces));
        assert!(!signal_requested(
            &off("OTEL_TRACES_EXPORTER"),
            Signal::Traces
        ));
        assert!(!signal_requested(
            &off("OTEL_METRICS_EXPORTER"),
            Signal::Metrics
        ));
        assert!(signal_requested(
            &env_of(&[on, ("OTEL_TRACES_EXPORTER", "otlp")]),
            Signal::Traces
        ));
    }

    fn version_of(resource: &Resource) -> Option<String> {
        resource
            .get(&Key::from_static_str("service.version"))
            .map(|v| v.to_string())
    }

    #[test]
    fn the_resource_carries_app_version() {
        let resource = shared_resource(&env_of(&[("APP_VERSION", "1.2.3")]));
        assert_eq!(version_of(&resource).as_deref(), Some("1.2.3"));
        assert!(
            resource
                .get(&Key::from_static_str("service.name"))
                .is_some()
        );
    }

    #[test]
    fn the_resource_has_no_version_when_app_version_is_unset_or_empty() {
        assert_eq!(version_of(&shared_resource(&env_of(&[]))), None);
        assert_eq!(
            version_of(&shared_resource(&env_of(&[("APP_VERSION", "")]))),
            None
        );
    }

    #[test]
    fn the_process_environment_is_readable() {
        assert!(process_env("PATH").is_some());
        assert!(process_env("A_VARIABLE_NOBODY_SETS_8F3A").is_none());
    }
}
