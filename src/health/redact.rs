//! Credential redaction for text that is logged or exported: a failing
//! check's error, an OTLP log message, a span name.
//!
//! A dependency's error often embeds its connection string
//! (`postgres://user:pass@host/db`, `password=...`), and the real error goes
//! to the logs even though `/ready` shows a fixed string. This strips the
//! credentials from that text before it is logged. It is a safety net, not a
//! parser: it over-redacts rather than leak. `lib-observability-py`'s
//! `redaction.py` implements the same rules; a change here is a change there.
//!
//! The rules, tried in this order at every position that can start one:
//!
//! 1. URL userinfo (`scheme://user:pw@host`), per whitespace/quote-delimited
//!    token, from the first `://` to the last `@`.
//! 2. `Authorization` / `Proxy-Authorization`: any scheme word plus the
//!    credential, or `Digest`-style `name=value` parameters.
//! 3. A key of the shared list (see [`SECRET_KEY_PARTS`]) followed by `=` or
//!    `:`, also inside JSON that was escaped (`{\"password\": \"x\"}`); a
//!    `Bearer`/`Basic` scheme before the value belongs to it.
//! 4. A bare `Bearer x` and a bare JWT (`eyJ...`).
//!
//! Every repetition is bounded (a key's tail 128, a bearer token or a JWT part
//! 4096) and nothing backtracks, so hostile input costs linear time.

use crate::redact_keys::{REDACTED, SECRET_KEY_PARTS};

/// How far past a matched key a key name may run (`password_hash`).
const MAX_KEY_TAIL: usize = 128;
/// The longest bearer token or JWT part that is matched.
const MAX_TOKEN: usize = 4096;

/// `text` with URL userinfo, secret key/value pairs, authorization
/// credentials, bearer tokens and JWTs replaced by `[REDACTED]`.
pub(crate) fn redact(text: &str) -> String {
    if text.contains("://") {
        redact_pairs(&redact_userinfo(text))
    } else {
        redact_pairs(text)
    }
}

/// Whether `c` ends a token for the userinfo rule.
fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || c == '"' || c == '\''
}

/// `scheme://user:pass@host` becomes `scheme://[REDACTED]@host`, token by
/// token: the userinfo runs from the first `://` to the last `@` of the token
/// (a password may contain `/`, `?` or `#`, so a path containing `@` is
/// over-redacted on purpose). Per-token `find`/`rfind` keeps a long run of
/// `://` linear.
fn redact_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token_start = 0;
    for (i, c) in text.char_indices() {
        if is_delimiter(c) {
            push_token(&mut out, &text[token_start..i]);
            out.push(c);
            token_start = i + c.len_utf8();
        }
    }
    push_token(&mut out, &text[token_start..]);
    out
}

fn push_token(out: &mut String, token: &str) {
    match (token.find("://"), token.rfind('@')) {
        (Some(scheme), Some(at)) if at >= scheme + 3 => {
            out.push_str(&token[..scheme + 3]);
            out.push_str(REDACTED);
            out.push_str(&token[at..]);
        }
        _ => out.push_str(token),
    }
}

/// What to replace: `bytes[start..end]` becomes `replacement`.
struct Found {
    start: usize,
    end: usize,
    replacement: &'static str,
}

impl Found {
    fn value(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            replacement: REDACTED,
        }
    }
}

/// `password=hunter2` becomes `password=[REDACTED]`, and the other rules.
fn redact_pairs(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        // Cheap pre-check: every key and token looked for starts with one of
        // a few letters, so most positions cost one comparison.
        let found = if could_start(bytes[i]) {
            auth_span(bytes, i)
                .or_else(|| pair_span(bytes, i))
                .or_else(|| bare_span(bytes, i))
        } else {
            None
        };
        match found {
            Some(found) => {
                out.push_str(&text[copied..found.start]);
                out.push_str(found.replacement);
                copied = found.end;
                i = found.end;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Whether a secret key, `Proxy-Authorization`, `Bearer` or a JWT (`eyJ`) can
/// start with `byte`.
fn could_start(byte: u8) -> bool {
    matches!(
        byte.to_ascii_lowercase(),
        b't' | b'p' | b's' | b'a' | b'c' | b'j' | b'b' | b'd' | b'e'
    )
}

/// Where `word` ends if it is written at `i`, case-insensitively.
fn word_end(bytes: &[u8], i: usize, word: &str) -> Option<usize> {
    let candidate = bytes.get(i..i + word.len())?;
    candidate
        .eq_ignore_ascii_case(word.as_bytes())
        .then_some(i + word.len())
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

fn spaces_at(bytes: &[u8], j: usize) -> usize {
    bytes[j..].iter().take_while(|b| **b == b' ').count()
}

/// The rest of a longer key name (`secret_key`, `session_id`), bounded.
fn skip_key_tail(bytes: &[u8], j: usize) -> usize {
    j + bytes[j..]
        .iter()
        .take(MAX_KEY_TAIL)
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'-'))
        .count()
}

/// Where the value starts after the separator that follows a key ending at
/// `j`: an optional backslash and quote (JSON, possibly escaped), spaces, `=`
/// or `:`, spaces.
fn separator_end(bytes: &[u8], mut j: usize) -> Option<usize> {
    j += usize::from(bytes.get(j) == Some(&b'\\'));
    j += usize::from(matches!(bytes.get(j), Some(b'"' | b'\'')));
    j += spaces_at(bytes, j);
    if !matches!(bytes.get(j), Some(b'=' | b':')) {
        return None;
    }
    j += 1;
    Some(j + spaces_at(bytes, j))
}

/// `Authorization: Bearer abc`, `Proxy-Authorization: Digest a="b", c=d`.
fn auth_span(bytes: &[u8], i: usize) -> Option<Found> {
    let mut j = i;
    if let Some(after_proxy) = word_end(bytes, j, "proxy") {
        j = after_proxy + usize::from(matches!(bytes.get(after_proxy), Some(b'-' | b'_')));
    }
    let key_end = word_end(bytes, j, "authorization")?;
    let start = separator_end(bytes, skip_key_tail(bytes, key_end))?;
    let end = match bytes.get(start) {
        Some(&quote @ (b'"' | b'\'')) => closing_quote(bytes, start + 1, quote),
        // Escaped JSON: the key/value rule below takes it whole.
        Some(b'\\') => return None,
        _ => credential_end(bytes, start),
    };
    Some(Found::value(start, end))
}

/// The end of an authorization value that is not quoted: an optional scheme
/// word (`Bearer`, `Token`, `Digest`, `ApiKey`), then `name=value` parameters
/// or one credential.
fn credential_end(bytes: &[u8], start: usize) -> usize {
    let word = bytes[start..]
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'-'))
        .count();
    let after_word = start + word;
    let spaces = spaces_at(bytes, after_word);
    let from = if word > 0 && bytes[start].is_ascii_alphabetic() && spaces > 0 {
        after_word + spaces
    } else {
        start
    };
    auth_parameters_end(bytes, from).unwrap_or_else(|| run_end(bytes, from))
}

/// The end of one or more `name=value` pairs (`value` quoted without escapes
/// or a run up to whitespace, a comma or a quote), each optionally followed
/// by a comma and spaces.
fn auth_parameters_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut k = from;
    let mut any = false;
    loop {
        let name = bytes[k..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'-'))
            .count();
        if name == 0 || bytes.get(k + name) != Some(&b'=') {
            break;
        }
        let value = k + name + 1;
        let quoted_end = (bytes.get(value) == Some(&b'"'))
            .then(|| bytes[value + 1..].iter().position(|b| *b == b'"'))
            .flatten()
            .map(|close| value + close + 2);
        k = quoted_end.unwrap_or_else(|| {
            value
                + bytes[value..]
                    .iter()
                    .take_while(|b| !b.is_ascii_whitespace() && !matches!(**b, b',' | b'\'' | b'"'))
                    .count()
        });
        if bytes.get(k) == Some(&b',') {
            k += 1 + spaces_at(bytes, k + 1);
        }
        any = true;
    }
    any.then_some(k)
}

/// A key of the shared list followed by `=` or `:`.
fn pair_span(bytes: &[u8], i: usize) -> Option<Found> {
    SECRET_KEY_PARTS.iter().find_map(|key| {
        let key_end = key_match_end(bytes, i, key)?;
        let start = separator_end(bytes, skip_key_tail(bytes, key_end))?;
        Some(Found::value(
            start,
            value_end(bytes, skip_scheme(bytes, start)),
        ))
    })
}

/// Past a `Bearer ` or `Basic ` scheme at `at`, which belongs to the value.
fn skip_scheme(bytes: &[u8], at: usize) -> usize {
    ["bearer", "basic"]
        .iter()
        .find_map(|scheme| {
            let after = word_end(bytes, at, scheme)?;
            let spaces = spaces_at(bytes, after);
            (spaces > 0).then_some(after + spaces)
        })
        .unwrap_or(at)
}

/// The end of the value starting at `j`: a quoted string (backslash escapes
/// honoured, unterminated runs to the end), the same inside JSON escaped once
/// more (`\"x\"`), or a run up to whitespace or a quote.
fn value_end(bytes: &[u8], j: usize) -> usize {
    match bytes.get(j) {
        Some(&quote @ (b'\'' | b'"')) => closing_quote(bytes, j + 1, quote),
        Some(b'\\') if matches!(bytes.get(j + 1), Some(b'"' | b'\'')) => {
            let quote = bytes[j + 1];
            bytes[j + 2..]
                .iter()
                .position(|b| *b == quote)
                .map_or(bytes.len(), |close| j + 2 + close + 1)
        }
        _ => run_end(bytes, j),
    }
}

/// The end of the run of bytes from `j` that are not whitespace or quotes.
fn run_end(bytes: &[u8], j: usize) -> usize {
    j + bytes[j..]
        .iter()
        .take_while(|b| !b.is_ascii_whitespace() && !matches!(**b, b'\'' | b'"'))
        .count()
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

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// A value that no key precedes: `Bearer abc` or a JWT.
fn bare_span(bytes: &[u8], i: usize) -> Option<Found> {
    if i > 0 && is_word_byte(bytes[i - 1]) {
        return None;
    }
    bare_bearer(bytes, i).or_else(|| bare_jwt(bytes, i))
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-')
}

/// `Bearer abc` becomes `Bearer [REDACTED]` (one space, as in
/// `lib-observability-py`).
fn bare_bearer(bytes: &[u8], i: usize) -> Option<Found> {
    let after = word_end(bytes, i, "bearer")?;
    let start = after + spaces_at(bytes, after);
    let len = bytes[start..]
        .iter()
        .take(MAX_TOKEN)
        .take_while(|b| is_token_byte(**b))
        .count();
    (start > after && len > 0).then_some(Found {
        start: after,
        end: start + len,
        replacement: " [REDACTED]",
    })
}

/// A JWT: `eyJ` and at least one more character, a dot, a part, a dot and a
/// possibly empty part, each part at most [`MAX_TOKEN`] long.
fn bare_jwt(bytes: &[u8], i: usize) -> Option<Found> {
    if bytes.get(i..i + 3)? != b"eyJ" {
        return None;
    }
    let part = |from: usize| {
        bytes[from..]
            .iter()
            .take(MAX_TOKEN)
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(**b, b'_' | b'-'))
            .count()
    };
    let head = part(i + 3);
    if head == 0 || bytes.get(i + 3 + head) != Some(&b'.') {
        return None;
    }
    let second_from = i + 3 + head + 1;
    let second = part(second_from);
    if second == 0 || bytes.get(second_from + second) != Some(&b'.') {
        return None;
    }
    let third_from = second_from + second + 1;
    Some(Found::value(i, third_from + part(third_from)))
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

    #[test]
    fn authorization_hides_any_scheme_and_credential() {
        assert_eq!(
            redact("proxy-authorization: Basic dXNlcjpw next"),
            "proxy-authorization: [REDACTED] next"
        );
        assert_eq!(
            redact("Proxy_Authorization=Token abc123 x"),
            "Proxy_Authorization=[REDACTED] x"
        );
        assert_eq!(
            redact("ProxyAuthorization: ApiKey k1"),
            "ProxyAuthorization: [REDACTED]"
        );
        assert_eq!(redact("Authorization: abc"), "Authorization: [REDACTED]");
        assert_eq!(
            redact("Authorization: 123 abc"),
            "Authorization: [REDACTED] abc"
        );
        assert_eq!(
            redact(r#"Authorization: "Bearer a b" x"#),
            "Authorization: [REDACTED] x"
        );
    }

    #[test]
    fn digest_parameters_are_hidden() {
        assert_eq!(
            redact(r#"Authorization: Digest username="bob", realm="r",nonce=abc, uri=/x next"#),
            "Authorization: [REDACTED] next"
        );
        let unterminated = redact(r#"Authorization: Digest a="rest"#);
        assert!(
            unterminated.starts_with("Authorization: [REDACTED]"),
            "{unterminated}"
        );
    }

    #[test]
    fn pairs_inside_escaped_json_are_hidden() {
        assert_eq!(
            redact(r#"{"password":"x","user":"bob"}"#),
            r#"{"password":[REDACTED],"user":"bob"}"#
        );
        assert_eq!(
            redact(r#"{\"password\": \"x\", \"user\": \"bob\"}"#),
            r#"{\"password\": [REDACTED], \"user\": \"bob\"}"#
        );
        assert_eq!(
            redact(r"{\'token\': \'abc\'}"),
            r"{\'token\': [REDACTED]}"
        );
        assert_eq!(
            redact(r#"{\"authorization\": \"Bearer abc\"}"#),
            r#"{\"authorization\": [REDACTED]}"#
        );
        assert_eq!(
            redact(r#"\"password\": \"abc"#),
            r#"\"password\": [REDACTED]"#
        );
        assert_eq!(redact(r"password=\x done"), "password=[REDACTED] done");
    }

    #[test]
    fn a_scheme_before_a_pair_value_belongs_to_it() {
        assert_eq!(redact("token: Bearer abc d"), "token: [REDACTED] d");
        assert_eq!(redact("password=basic  xyz d"), "password=[REDACTED] d");
        assert_eq!(redact("token: basix d"), "token: [REDACTED] d");
    }

    #[test]
    fn key_tails_are_bounded() {
        let long = format!("password{}=x", "a".repeat(200));
        assert_eq!(redact(&long), long);
        let near = format!("password{}=x", "a".repeat(100));
        assert_eq!(
            redact(&near),
            format!("password{}=[REDACTED]", "a".repeat(100))
        );
    }

    #[test]
    fn userinfo_runs_from_the_first_scheme_separator_to_the_last_at() {
        assert_eq!(redact("a://b@c://d@e"), "a://[REDACTED]@e");
        assert_eq!(redact("x://@h"), "x://[REDACTED]@h");
        assert_eq!(redact("user@host ://"), "user@host ://");
    }

    #[test]
    fn jwt_shapes() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.";
        assert_eq!(redact(&format!("a {jwt} b")), "a [REDACTED] b");
        for text in [
            "ey",
            "eyJ.a.b",
            "eyJa..c",
            "eyJabc.def",
            "xeyJa.b.c",
            "e_eyJa.b.c",
        ] {
            assert_eq!(redact(text), text, "{text}");
        }
    }

    #[test]
    fn hostile_inputs_finish_and_still_redact() {
        let n = 100_000;
        assert_eq!(redact(&"a".repeat(n * 4)), "a".repeat(n * 4));
        assert_eq!(redact(&"://".repeat(n)), "://".repeat(n));
        assert_eq!(redact(&"://@".repeat(n)), "://[REDACTED]@");
        // Each `eyJ` scans at most one bounded part, so this stays cheap.
        assert_eq!(redact(&"eyJ-".repeat(5_000)), "eyJ-".repeat(5_000));
        assert_eq!(redact(&"eyJ.".repeat(n)), "eyJ.".repeat(n));
        assert!(redact(&"Bearer ".repeat(n)).contains("[REDACTED]"));
        assert_eq!(
            redact(&format!("Bearer {}", "x".repeat(n))),
            format!("Bearer [REDACTED]{}", "x".repeat(n - MAX_TOKEN))
        );
        assert_eq!(redact(&"password=".repeat(n)), "password=[REDACTED]");
        assert_eq!(
            redact(&"passwordpassword".repeat(n)),
            "passwordpassword".repeat(n)
        );
        assert_eq!(
            redact(&format!("token{}", " ".repeat(n))),
            format!("token{}", " ".repeat(n))
        );
        assert!(redact(&"authorization: ".repeat(n)).contains("[REDACTED]"));
    }
}
