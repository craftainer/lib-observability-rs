//! Credential redaction for the error text a failing check logs.
//!
//! A dependency's error often embeds its connection string
//! (`postgres://user:pass@host/db`, `password=...`), and the real error goes
//! to the logs even though `/ready` shows a fixed string. This strips the
//! credentials from that text before it is logged. It is a safety net, not a
//! parser: it over-redacts rather than leak.

const REDACTED: &str = "[REDACTED]";

/// Keys whose value is redacted when written `key=value` or `key: value`
/// (matched case-insensitively, also as the tail of a longer key such as
/// `db_password`). A key that another key is a prefix of comes first.
const SECRET_KEYS: [&str; 9] = [
    "secret_key",
    "secret",
    "password",
    "passwd",
    "pwd",
    "token",
    "api_key",
    "apikey",
    "access_key",
];

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
        match secret_value_span(bytes, i) {
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

/// If a secret key starts at `i`, the byte span of its value.
fn secret_value_span(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    let key = SECRET_KEYS.iter().find(|key| {
        bytes
            .get(i..i + key.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(key.as_bytes()))
    })?;
    let mut j = i + key.len();
    j += bytes[j..].iter().take_while(|b| **b == b' ').count();
    if !matches!(bytes.get(j), Some(b'=' | b':')) {
        return None;
    }
    j += 1;
    j += bytes[j..].iter().take_while(|b| **b == b' ').count();
    let start = j;
    let end = match bytes.get(j) {
        Some(quote @ (b'\'' | b'"')) => closing_quote(bytes, j + 1, *quote),
        _ => {
            start
                + bytes[start..]
                    .iter()
                    .take_while(|b| !b.is_ascii_whitespace() && !matches!(**b, b'&' | b'\'' | b'"'))
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
        assert_eq!(redact("password=a;b,c)d done"), "password=[REDACTED] done");
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
            "PASSWORD: [REDACTED] token = [REDACTED] and db_pwd=[REDACTED]&k=1"
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
