//! The one list of credential-looking key names every redaction point uses:
//! the stdout JSON layer, the OTLP log bridge, the span exporter and the
//! health-check error text. `lib-observability-py` keeps the same list.

/// The placeholder that replaces a redacted value.
pub(crate) const REDACTED: &str = "[REDACTED]";

/// Key-name fragments whose value is never exported. A key is secret when it
/// contains one of these, compared case-insensitively (`db_password`,
/// `Authorization`, `x-api-key`).
pub(crate) const SECRET_KEY_PARTS: [&str; 18] = [
    "token",
    "password",
    "passwd",
    "pwd",
    "secret",
    "authorization",
    "claims",
    "credential",
    "api_key",
    "apikey",
    "access_key",
    "cookie",
    "bearer",
    "jwt",
    "session",
    "dsn",
    "private",
    "signature",
];

/// Whether the value under `key` must not be exported.
pub(crate) fn is_secret_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SECRET_KEY_PARTS.iter().any(|part| lowered.contains(part))
}

/// `text` with URL userinfo and `key=value` secrets replaced by `[REDACTED]`.
#[cfg(any(
    feature = "otel",
    feature = "postgres",
    feature = "redis",
    feature = "s3",
    feature = "http"
))]
pub(crate) fn redact_text(text: &str) -> String {
    crate::health::redact::redact(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_by_fragment_in_any_case() {
        for key in [
            "password",
            "DB_PASSWORD",
            "x-api-key",
            "Authorization",
            "set-cookie",
            "session_id",
            "client_secret",
            "jwt",
            "dsn",
            "private_key",
            "signature",
            "bearer",
            "access_key_id",
            "claims",
            "credential",
            "pwd",
            "passwd",
            "apikey",
            "api_token",
        ] {
            assert!(is_secret_key(key), "{key}");
        }
        for key in ["order_id", "message", "http.route", "user"] {
            assert!(!is_secret_key(key), "{key}");
        }
    }

    #[cfg(any(
        feature = "otel",
        feature = "postgres",
        feature = "redis",
        feature = "s3",
        feature = "http"
    ))]
    #[test]
    fn text_is_redacted() {
        assert_eq!(redact_text("a password=x b"), "a password=[REDACTED] b");
    }
}
