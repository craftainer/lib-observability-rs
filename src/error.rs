//! The crate's error type.

/// Why an observability setup call failed.
///
/// Non-exhaustive: a later release may add variants, so a `match` needs a
/// wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A global `tracing` subscriber was already installed in this process.
    #[error("a global tracing subscriber is already installed")]
    SubscriberAlreadyInstalled,

    /// A health check with this name is already registered. `/ready` keys its
    /// response by name, so a second check under one name would hide the
    /// first one's result.
    #[error("a health check named {0:?} is already registered")]
    DuplicateCheckName(String),

    /// The OTLP log exporter could not be built (the message is the
    /// exporter's own). Returned instead of continuing without the export,
    /// so a host never believes logs are shipped when they are not. Nothing
    /// is installed in that case; the host may fall back to
    /// [`configure_logging_without_export`](crate::logs::configure_logging_without_export).
    #[error("the OTLP log exporter could not be built: {0}")]
    ExporterBuild(String),

    /// Flushing or shutting down the OTLP log export failed (the message is
    /// the SDK's own).
    #[error("flushing the OTLP log export failed: {0}")]
    Flush(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_displays_a_message() {
        assert_eq!(
            Error::SubscriberAlreadyInstalled.to_string(),
            "a global tracing subscriber is already installed"
        );
        assert_eq!(
            Error::DuplicateCheckName("db".to_string()).to_string(),
            "a health check named \"db\" is already registered"
        );
        assert_eq!(
            Error::ExporterBuild("bad".to_string()).to_string(),
            "the OTLP log exporter could not be built: bad"
        );
        assert_eq!(
            Error::Flush("late".to_string()).to_string(),
            "flushing the OTLP log export failed: late"
        );
    }
}
