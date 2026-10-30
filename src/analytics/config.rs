//! Analytics settings, read from the `ANALYTICS_*` environment variables
//! (contract I3).

use std::collections::HashSet;

/// Where events go (`ANALYTICS_BACKEND`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// Discard everything.
    #[default]
    None,
    /// A structured `tracing` event per analytics event.
    Log,
    /// Umami's `/api/batch`.
    Umami,
    /// An OpenTelemetry log record per event (needs the `otel` feature).
    Otel,
}

/// The consent mode (`ANALYTICS_CONSENT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Consent {
    /// Tracking is on unless the visitor opts out (GPC).
    #[default]
    Implied,
    /// Tracking needs the visitor's consent, set on the request context.
    OptIn,
}

/// Geo handling (`ANALYTICS_GEO`). Accepted for contract parity: backend
/// events carry `ip=server`, so no location is derived either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Geo {
    /// No geo.
    #[default]
    None,
    /// Coarse geo.
    Coarse,
}

/// The resolved settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsConfig {
    /// `ANALYTICS_ENABLED` (default true).
    pub enabled: bool,
    /// `ANALYTICS_BACKEND` (default `none`).
    pub backend: Backend,
    /// `ANALYTICS_ENDPOINT`: the Umami base URL (`/api/batch` is appended).
    pub endpoint: String,
    /// `ANALYTICS_SITE_ID`.
    pub site_id: String,
    /// `ANALYTICS_HOSTNAME`, sent as `payload.hostname`.
    pub hostname: String,
    /// `ANALYTICS_CONSENT` (default `implied`).
    pub consent: Consent,
    /// `ANALYTICS_RESPECT_GPC` (default true).
    pub respect_gpc: bool,
    /// `ANALYTICS_GEO` (default `none`).
    pub geo: Geo,
    /// `ANALYTICS_PROP_KEYS`: the property keys that may be sent.
    pub prop_keys: HashSet<String>,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: Backend::None,
            endpoint: String::new(),
            site_id: String::new(),
            hostname: String::new(),
            consent: Consent::Implied,
            respect_gpc: true,
            geo: Geo::None,
            prop_keys: HashSet::new(),
        }
    }
}

fn flag(value: Option<&str>, default: bool) -> bool {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if !v.is_empty() => !matches!(v.as_str(), "0" | "false" | "no" | "off"),
        _ => default,
    }
}

/// `value` looked up in `choices`, or `default` (with a warning when it is
/// set but unknown).
fn choice<T: Copy>(name: &str, value: Option<&str>, choices: &[(&str, T)], default: T) -> T {
    let value = value.unwrap_or_default().trim().to_ascii_lowercase();
    if value.is_empty() {
        return default;
    }
    choices.iter().find(|(k, _)| *k == value).map_or_else(
        || {
            tracing::warn!("{name}={value:?} is not a known choice; using the default");
            default
        },
        |(_, v)| *v,
    )
}

impl AnalyticsConfig {
    /// Read the settings from the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(&|name| std::env::var(name).ok())
    }

    /// Read the settings through `lookup` (the testable core of
    /// [`from_env`](Self::from_env)). An unknown choice logs a warning and
    /// falls back to its default.
    #[must_use]
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        let text = |name: &str| lookup(name).unwrap_or_default().trim().to_owned();
        Self {
            enabled: flag(lookup("ANALYTICS_ENABLED").as_deref(), true),
            backend: choice(
                "ANALYTICS_BACKEND",
                lookup("ANALYTICS_BACKEND").as_deref(),
                &[
                    ("none", Backend::None),
                    ("log", Backend::Log),
                    ("umami", Backend::Umami),
                    ("otel", Backend::Otel),
                ],
                Backend::None,
            ),
            endpoint: text("ANALYTICS_ENDPOINT").trim_end_matches('/').to_owned(),
            site_id: text("ANALYTICS_SITE_ID"),
            hostname: text("ANALYTICS_HOSTNAME"),
            consent: choice(
                "ANALYTICS_CONSENT",
                lookup("ANALYTICS_CONSENT").as_deref(),
                &[("implied", Consent::Implied), ("opt-in", Consent::OptIn)],
                Consent::Implied,
            ),
            respect_gpc: flag(lookup("ANALYTICS_RESPECT_GPC").as_deref(), true),
            geo: choice(
                "ANALYTICS_GEO",
                lookup("ANALYTICS_GEO").as_deref(),
                &[("none", Geo::None), ("coarse", Geo::Coarse)],
                Geo::None,
            ),
            prop_keys: text("ANALYTICS_PROP_KEYS")
                .split(',')
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(pairs: &[(&str, &str)]) -> AnalyticsConfig {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        AnalyticsConfig::from_lookup(&move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        })
    }

    #[test]
    fn defaults() {
        assert_eq!(of(&[]), AnalyticsConfig::default());
        assert_eq!(AnalyticsConfig::from_env().backend, Backend::None);
    }

    #[test]
    fn every_variable_is_read() {
        let c = of(&[
            ("ANALYTICS_ENABLED", "false"),
            ("ANALYTICS_BACKEND", " UMAMI "),
            ("ANALYTICS_ENDPOINT", "https://u.example/"),
            ("ANALYTICS_SITE_ID", " site "),
            ("ANALYTICS_HOSTNAME", "app.example"),
            ("ANALYTICS_CONSENT", "opt-in"),
            ("ANALYTICS_RESPECT_GPC", "off"),
            ("ANALYTICS_GEO", "coarse"),
            ("ANALYTICS_PROP_KEYS", "plan, tier,,"),
        ]);
        assert!(!c.enabled);
        assert_eq!(c.backend, Backend::Umami);
        assert_eq!(c.endpoint, "https://u.example");
        assert_eq!(c.site_id, "site");
        assert_eq!(c.hostname, "app.example");
        assert_eq!(c.consent, Consent::OptIn);
        assert!(!c.respect_gpc);
        assert_eq!(c.geo, Geo::Coarse);
        assert_eq!(c.prop_keys.len(), 2);
        assert!(c.prop_keys.contains("plan") && c.prop_keys.contains("tier"));
        assert_eq!(of(&[("ANALYTICS_BACKEND", "log")]).backend, Backend::Log);
        assert_eq!(of(&[("ANALYTICS_BACKEND", "otel")]).backend, Backend::Otel);
    }

    #[test]
    fn unknown_choices_fall_back_and_flags_parse() {
        let c = of(&[
            ("ANALYTICS_BACKEND", "plausible"),
            ("ANALYTICS_CONSENT", "maybe"),
            ("ANALYTICS_GEO", "fine"),
            ("ANALYTICS_ENABLED", ""),
            ("ANALYTICS_RESPECT_GPC", "yes"),
        ]);
        assert_eq!(c, AnalyticsConfig::default());
        assert!(!of(&[("ANALYTICS_ENABLED", "0")]).enabled);
        assert!(!of(&[("ANALYTICS_ENABLED", "No")]).enabled);
    }
}
