//! Validation and sanitising of event names, properties and URLs (contract I3).

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

/// The longest event name.
pub const MAX_NAME: usize = 50;
/// The longest property value, in characters.
pub const MAX_VALUE: usize = 200;
/// The longest URL path, in characters.
pub const MAX_URL: usize = 500;

/// A property value: a scalar, never a nested structure.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PropValue {
    /// A string, truncated to 200 characters.
    Str(String),
    /// A signed integer.
    Int(i64),
    /// An unsigned integer.
    Uint(u64),
    /// A finite float (a NaN or infinity drops the property).
    Float(f64),
    /// A boolean.
    Bool(bool),
}

impl From<&str> for PropValue {
    fn from(value: &str) -> Self {
        Self::Str(value.to_owned())
    }
}

impl From<String> for PropValue {
    fn from(value: String) -> Self {
        Self::Str(value)
    }
}

impl From<i64> for PropValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<i32> for PropValue {
    fn from(value: i32) -> Self {
        Self::Int(value.into())
    }
}

impl From<u64> for PropValue {
    fn from(value: u64) -> Self {
        Self::Uint(value)
    }
}

impl From<u32> for PropValue {
    fn from(value: u32) -> Self {
        Self::Uint(value.into())
    }
}

impl From<f64> for PropValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<bool> for PropValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

/// Whether `name` is a well-formed event name of at most 50 characters:
/// `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$`.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    let segment = |s: &str| {
        let mut chars = s.chars();
        chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    name.len() <= MAX_NAME && name.contains('.') && name.split('.').all(segment)
}

/// Keep allowlisted properties, truncating strings; drop (and log at debug,
/// never with a value) the rest.
pub(crate) fn clean_props(
    props: &[(&str, PropValue)],
    allowed: &HashSet<String>,
) -> BTreeMap<String, PropValue> {
    let mut out = BTreeMap::new();
    for (key, value) in props {
        if !allowed.contains(*key) {
            tracing::debug!("analytics prop dropped: key not on the allowlist");
            continue;
        }
        let value = match value {
            PropValue::Float(f) if !f.is_finite() => {
                tracing::debug!(key, "analytics prop dropped: not a finite number");
                continue;
            }
            PropValue::Str(s) => PropValue::Str(s.chars().take(MAX_VALUE).collect()),
            other => other.clone(),
        };
        out.insert((*key).to_owned(), value);
    }
    out
}

/// The path of `url` with scheme, authority, query and fragment stripped;
/// `/` when empty. Capped at 500 characters.
pub(crate) fn clean_url(url: Option<&str>) -> String {
    let url = url.unwrap_or("");
    let url = url.split(['#', '?']).next().unwrap_or("");
    // "scheme://host/path" (the text before "://" is a scheme when it has no
    // '/') or "//host/path": skip the authority.
    let after_authority = match url.split_once("://") {
        Some((scheme, rest)) if !scheme.contains('/') => Some(rest),
        _ => url.strip_prefix("//"),
    };
    let path = after_authority.map_or(url, |rest| rest.find('/').map_or("", |i| &rest[i..]));
    let mut path = path.to_owned();
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    path.chars().take(MAX_URL).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["a.b", "page.view", "checkout.step_2", "a1.b2.c3"] {
            assert!(valid_name(ok), "{ok}");
        }
        let long = format!("a.{}", "b".repeat(48));
        assert_eq!(long.len(), 50);
        assert!(valid_name(&long));
        for bad in [
            "",
            "a",
            "a.",
            ".a",
            "a..b",
            "A.b",
            "a.B",
            "1a.b",
            "a.1b",
            "a-b.c",
            "a b.c",
            "_a.b",
            "a.b\n",
            &format!("a.{}", "b".repeat(49)),
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn props_keep_only_allowlisted_finite_scalars_and_truncate() {
        let allowed: HashSet<String> = ["plan", "n", "f", "ok", "big"].map(String::from).into();
        let long = "x".repeat(300);
        let props = [
            ("plan", PropValue::from(long.as_str())),
            ("n", 3.into()),
            ("f", 1.5.into()),
            ("f", f64::NAN.into()),
            ("ok", true.into()),
            ("big", u64::MAX.into()),
            ("secret", "nope".into()),
        ];
        let out = clean_props(&props, &allowed);
        assert_eq!(out.len(), 5, "{out:?}");
        assert_eq!(out["plan"], PropValue::Str("x".repeat(200)));
        assert_eq!(out["n"], PropValue::Int(3));
        assert_eq!(out["f"], PropValue::Float(1.5));
        assert_eq!(out["big"], PropValue::Uint(u64::MAX));
        assert!(!out.contains_key("secret"));
        let infinite = clean_props(&[("f", f64::INFINITY.into())], &allowed);
        assert!(infinite.is_empty());
    }

    #[test]
    fn every_conversion_builds_the_matching_variant() {
        assert_eq!(PropValue::from("a"), PropValue::Str("a".into()));
        assert_eq!(
            PropValue::from(String::from("a")),
            PropValue::Str("a".into())
        );
        assert_eq!(PropValue::from(1_i64), PropValue::Int(1));
        assert_eq!(PropValue::from(1_i32), PropValue::Int(1));
        assert_eq!(PropValue::from(1_u64), PropValue::Uint(1));
        assert_eq!(PropValue::from(1_u32), PropValue::Uint(1));
        assert_eq!(PropValue::from(1.0), PropValue::Float(1.0));
        assert_eq!(PropValue::from(true), PropValue::Bool(true));
    }

    #[test]
    fn urls_are_reduced_to_a_path() {
        for (input, expected) in [
            (None, "/"),
            (Some(""), "/"),
            (Some("/a/b"), "/a/b"),
            (Some("/a/b?x=1#f"), "/a/b"),
            (Some("/a#f?x"), "/a"),
            (Some("?x=1"), "/"),
            (Some("a/b"), "/a/b"),
            (Some("https://example.com/p/q?s=1"), "/p/q"),
            (Some("https://example.com"), "/"),
            (Some("//example.com/p"), "/p"),
            (Some("//example.com"), "/"),
            (Some("/a://b"), "/a://b"),
        ] {
            assert_eq!(clean_url(input), expected, "{input:?}");
        }
        let long = format!("/{}", "a".repeat(600));
        assert_eq!(clean_url(Some(&long)).chars().count(), 500);
    }
}
