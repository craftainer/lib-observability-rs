//! Example library crate: a tiny, fully tested public API to replace.

use std::fmt;

/// Why [`greet`] refused its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreetError {
    /// The name was empty or only whitespace.
    EmptyName,
}

impl fmt::Display for GreetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => f.write_str("name must not be empty"),
        }
    }
}

impl std::error::Error for GreetError {}

/// Builds a greeting for `name`.
///
/// # Errors
///
/// Returns [`GreetError::EmptyName`] when `name` is empty or only whitespace.
///
/// # Examples
///
/// ```
/// assert_eq!(template_rust_crate::greet("Ada").unwrap(), "Hello, Ada!");
/// ```
pub fn greet(name: &str) -> Result<String, GreetError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(GreetError::EmptyName);
    }
    Ok(format!("Hello, {name}!"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_a_name() {
        assert_eq!(greet("  Ada ").unwrap(), "Hello, Ada!");
    }

    #[test]
    fn rejects_blank_names() {
        assert_eq!(greet(" ").unwrap_err(), GreetError::EmptyName);
    }

    #[test]
    fn error_displays_a_message() {
        assert_eq!(GreetError::EmptyName.to_string(), "name must not be empty");
    }
}
