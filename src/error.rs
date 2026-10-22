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
    }
}
