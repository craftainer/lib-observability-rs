//! Integration tier: exercises only the public API, as a dependent would.

use template_rust_crate::{GreetError, greet};

#[test]
fn public_api_round_trip() {
    assert_eq!(greet("Grace").unwrap(), "Hello, Grace!");
    assert_eq!(greet("").unwrap_err(), GreetError::EmptyName);
}
