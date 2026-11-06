//! Credential redaction for the error text a failing check logs.
//!
//! A dependency's error often embeds its connection string
//! (`postgres://user:pass@host/db`, `password=...`), and the real error goes
//! to the logs even though `/ready` shows a fixed string. This strips the
//! credentials from that text before it is logged. It is a safety net, not a
//! parser: it over-redacts rather than leak.

use crate::redact_keys::{REDACTED, SECRET_KEY_PARTS};

/// `text` with URL userinfo (`user:pass@`) and secret key/value pairs
/// replaced by `[REDACTED]`.
pub(crate) fn redact(text: &str) -> String {
    redact_pairs(&redact_userinfo(text))
}

/// `scheme://user:pass@host` becomes `scheme://[REDACTED]@host`.
fn redact_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        // The token runs to whitespace or a quote, not to the first `/`: a
        // password may contain `/`, `?` or `#`. The last `@` in the token ends
        // the userinfo; this over-redacts a path that contains `@`, on purpose.
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(tail.len());
        match tail[..end].rfind('@') {
            Some(at_sign) => {
                out.push_str(REDACTED);
                rest = &tail[at_sign..];
            }
            None => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// `password=hunter2` becomes `password=[REDACTED]`.
fn redact_pairs(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        // Cheap pre-check: every key and token this looks for starts with one
        // of a few letters, so most positions cost one comparison.
        let found = if could_start(bytes[i]) {
            secret_value_span(bytes, i).or_else(|| bare_span(bytes, i))
        } else {
            None
        };
        match found {
            Some((value_start, value_end)) => {
                out.push_str(&text[copied..value_start]);
                out.push_str(REDACTED);
                copied = value_end;
                i = value_end;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Whether a secret key, `Bearer` or a JWT (`eyJ`) can start with `byte`.
fn could_start(byte: u8) -> bool {
    matches!(
        byte.to_ascii_lowercase(),
        b't' | b'p' | b's' | b'a' | b'c' | b'j' | b'b' | b'd' | b'e'
    )
}

/// If a secret key starts at `i`, the byte span of its value.
fn secret_value_span(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    SECRET_KEY_PARTS
        .iter()
        .find_map(|key| value_span_after(bytes, key_match_end(bytes, i, key)?))
}

/// Where `key` ends if it is written at `i`: case-insensitive, and each `_`
/// of the key may be written `_`, `-` or nothing (`api_key`, `api-key`,
/// `apikey`).
fn key_match_end(bytes: &[u8], i: usize, key: &str) -> Option<usize> {
    let mut j = i;
    for expected in key.bytes() {
        if expected == b'_' {
            j += usize::from(matches!(bytes.get(j), Some(b'_' | b'-')));
        } else if bytes.get(j)?.eq_ignore_ascii_case(&expected) {
            j += 1;
        } else {
            return None;
        }
    }
    Some(j)
}

/// A value that is not preceded by a key: `Bearer abc` (the token is the
/// span) or a JWT (`eyJ...`, the whole token).
fn bare_span(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    if i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    bare_bearer(bytes, i).or_else(|| bare_jwt(bytes, i))
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-')
}

fn bare_bearer(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    let word = bytes.get(i..i + 6)?;
    if !word.eq_ignore_ascii_case(b"bearer") {
        return None;
    }
    let spaces = bytes[i + 6..].iter().take_while(|b| **b == b' ').count();
    let start = i + 6 + spaces;
    let len = bytes[start..].iter().take_while(|b| is_token_byte(**b)).count();
    (spaces > 0 && len > 0).then_some((start, start + len))
}

fn bare_jwt(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    if bytes.get(i..i + 3)? != b"eyJ" {
        return None;
    }
    let segment = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'-'))
            .count()
    };
    let mut end = i + segment(i);
    for _ in 0..2 {
        if bytes.get(end) != Some(&b'.') {
            return None;
        }
        end += 1 + segment(end + 1);
    }
    Some((i, end))
}

/// If a `Bearer ` or `Basic ` scheme starts at `at`, where the credential
/// after it starts.
fn after_scheme(bytes: &[u8], at: usize) -> Option<usize> {
    ["bearer", "basic"].iter().find_map(|scheme| {
        let word = bytes.get(at..at + scheme.len())?;
        let spaces = bytes[at + scheme.len()..]
            .iter()
            .take_while(|b| **b == b' ')
            .count();
        let next = at + scheme.len() + spaces;
        (word.eq_ignore_ascii_case(scheme.as_bytes()) && spaces > 0 && next < bytes.len())
            .then_some(next)
    })
}

/// The byte span of the value of a secret key that ends at `key_end`, if a
/// `=` or `:` follows.
fn value_span_after(bytes: &[u8], key_end: usize) -> Option<(usize, usize)> {
    // The rest of a longer key name (`secret_key`, `session_id`).
    let mut j = key_end
        + bytes[key_end..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'-' | b'.'))
            .count();
    j += bytes[j..].iter().take_while(|b| **b == b' ').count();
    if !matches!(bytes.get(j), Some(b'=' | b':')) {
        return None;
    }
    j += 1;
    j += bytes[j..].iter().take_while(|b| **b == b' ').count();
    let start = j;
    // An authorization scheme belongs to the value: `Bearer abc` is all secret.
    let credential = after_scheme(bytes, j).unwrap_or(j);
    let end = match bytes.get(credential) {
        Some(quote @ (b'\'' | b'"')) => closing_quote(bytes, credential + 1, *quote),
        _ => {
            credential
                + bytes[credential..]
                    .iter()
                    .take_while(|b| !b.is_ascii_whitespace() && !matches!(**b, b'\'' | b'"'))
                    .count()
        }
    };
    Some((start, end))
}

/// The index just past the quote that closes a value starting at `from`,
/// skipping backslash-escaped characters; the end of `bytes` if unterminated.
fn closing_quote(bytes: &[u8], from: usize, quote: u8) -> usize {
    let mut k = from;
    while k < bytes.len() {
        match bytes[k] {
            b'\\' => k += 2,
            b if b == quote => return k + 1,
            _ => k += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_userinfo_is_redacted() {
        assert_eq!(
            redact("error connecting to postgres://app:hunter2@db:5432/app?sslmode=x"),
            "error connecting to postgres://[REDACTED]@db:5432/app?sslmode=x"
        );
        assert_eq!(
            redact("redis://:s3cret@cache:6379/0 and https://u:p@a.example/x"),
            "redis://[REDACTED]@cache:6379/0 and https://[REDACTED]@a.example/x"
        );
    }

    #[test]
    fn urls_without_userinfo_are_untouched() {
        let text = "GET https://example.com/a failed: http://host:1/x";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn a_password_with_url_delimiters_is_fully_redacted() {
        assert_eq!(
            redact("postgres://app:pa/ss@db/x and postgres://u:p?w#z@h/d"),
            "postgres://[REDACTED]@db/x and postgres://[REDACTED]@h/d"
        );
    }

    #[test]
    fn secrets_containing_separators_are_fully_redacted() {
        assert_eq!(
            redact("password=a;b,c)d&e done"),
            "password=[REDACTED] done"
        );
        assert_eq!(redact(r"token='it\'s secret' x"), "token=[REDACTED] x");
    }

    #[test]
    fn a_trailing_scheme_separator_is_kept() {
        assert_eq!(redact("bad url ://"), "bad url ://");
    }

    #[test]
    fn key_value_secrets_are_redacted() {
        assert_eq!(
            redact("host=db user=app password=hunter2 dbname=x"),
            "host=db user=app password=[REDACTED] dbname=x"
        );
        assert_eq!(
            redact("PASSWORD: hunter2 token = 'a b c' and db_pwd=x&k=1"),
            "PASSWORD: [REDACTED] token = [REDACTED] and db_pwd=[REDACTED]"
        );
        assert_eq!(
            redact("secret_key=\"q r\" end"),
            "secret_key=[REDACTED] end"
        );
        assert_eq!(
            redact("api_key=abc apikey:def"),
            "api_key=[REDACTED] apikey:[REDACTED]"
        );
    }

    #[test]
    fn the_shared_key_list_applies_to_text() {
        assert_eq!(
            redact("cookie: sid=1 session_id=9 dsn=x jwt=a.b.c bearer=zz"),
            "cookie: [REDACTED] session_id=[REDACTED] dsn=[REDACTED] jwt=[REDACTED] bearer=[REDACTED]"
        );
    }

    #[test]
    fn keys_may_be_written_with_dashes_or_nothing() {
        assert_eq!(
            redact("api-key=a apikey=b API_KEY=c access-key=d accesskey=e x-api-key: f"),
            "api-key=[REDACTED] apikey=[REDACTED] API_KEY=[REDACTED] access-key=[REDACTED] accesskey=[REDACTED] x-api-key: [REDACTED]"
        );
    }

    #[test]
    fn a_scheme_before_the_value_is_part_of_it() {
        assert_eq!(
            redact("Authorization: Bearer abc.def next"),
            "Authorization: [REDACTED] next"
        );
        assert_eq!(
            redact(r#"authorization="Basic dXNlcjpw\"x" end"#),
            "authorization=[REDACTED] end"
        );
        assert_eq!(redact("token: bearer"), "token: [REDACTED]");
    }

    #[test]
    fn bare_bearer_values_and_jwts_are_redacted() {
        assert_eq!(redact("sent Bearer x1.y_2 ok"), "sent Bearer [REDACTED] ok");
        assert_eq!(redact("a bearer of news"), "a bearer [REDACTED] news");
        assert_eq!(redact("Bearer"), "Bearer");
        assert_eq!(redact("Bearer  "), "Bearer  ");
        assert_eq!(redact("notbearer x"), "notbearer x");
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln";
        assert_eq!(redact(&format!("got {jwt} end")), "got [REDACTED] end");
        assert_eq!(redact("eyJabc.def"), "eyJabc.def");
        assert_eq!(redact("eyJabc"), "eyJabc");
        assert_eq!(redact("eyJa.b.c"), "[REDACTED]");
    }

    #[test]
    fn an_unterminated_quote_redacts_to_the_end() {
        assert_eq!(redact("password='oops"), "password=[REDACTED]");
    }

    #[test]
    fn a_key_without_a_value_separator_is_left_alone() {
        let text = "the token was rejected; password";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn non_ascii_text_survives() {
        assert_eq!(
            redact("héllo password=ü ok"),
            "héllo password=[REDACTED] ok"
        );
    }
}
