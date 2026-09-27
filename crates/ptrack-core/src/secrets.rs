//! The one credential detector shared by every surface that hands stored
//! project text to an agent: the launch context, the `ptrack context` digest,
//! the MCP context tool, and the agent-event privacy boundary.
//!
//! Detection is deliberately shaped, not keyword-only. A sensitive key counts
//! only when it is assigned a value (`password=…`, `"api_key": "…"`,
//! `x-api-key: …`, `DB_PASSWORD=…`), a token counts only in a recognised
//! provider format, and a URL counts only when its userinfo carries a password.
//! Ordinary prose that merely mentions a password, a token, or a URL is left
//! alone, so a note about "the password reset flow" still reaches the agent.

use std::sync::OnceLock;

use regex::Regex;

/// The marker that replaces a whole line judged to hold a credential.
pub const REDACTED_CREDENTIAL: &str = "[REDACTED POTENTIAL CREDENTIAL]";

/// Reports whether any line of `value` may hold a credential, including any
/// part of a PEM private key block and the indented value lines of a multi-line
/// key/value assignment (`db_password:` followed by an indented value).
#[must_use]
pub fn contains_credential(value: &str) -> bool {
    if private_key_header().is_match(value) {
        return true;
    }
    let mut block = ValueBlock::default();
    value.split('\n').any(|line| {
        let hit = block.covers(line) || line_contains_credential(line);
        block.advance(line);
        hit
    })
}

/// Replaces every line that may hold a credential with
/// [`REDACTED_CREDENTIAL`]; every line of a PEM private key block, from its
/// `BEGIN` line through its `END` line, is replaced too, as are the indented
/// value lines of a multi-line key/value assignment (`db_password:` followed by
/// an indented value).
#[must_use]
pub fn redact_credential_lines(value: &str) -> String {
    let mut changed = false;
    let mut private_key = false;
    let mut block = ValueBlock::default();
    let mut lines = Vec::new();
    for line in value.split('\n') {
        if private_key_header().is_match(line) {
            private_key = true;
        }
        if private_key || line_contains_credential(line) || block.covers(line) {
            lines.push(REDACTED_CREDENTIAL);
            changed = true;
        } else {
            lines.push(line);
        }
        if private_key && private_key_footer().is_match(line) {
            private_key = false;
        }
        block.advance(line);
    }
    if changed {
        lines.join("\n")
    } else {
        value.to_owned()
    }
}

/// Tracks the indented value block that follows a key-only assignment line
/// (`db_password:` with the value on the following, deeper-indented lines) —
/// the YAML and pretty-JSON shape a line-oriented matcher would otherwise
/// miss.
#[derive(Default)]
struct ValueBlock {
    key_indent: Option<usize>,
}

impl ValueBlock {
    /// Whether `line` is an indented value line of the open block.
    fn covers(&self, line: &str) -> bool {
        let indent = line.len() - line.trim_start().len();
        self.key_indent
            .is_some_and(|key_indent| indent > key_indent && !line.trim().is_empty())
    }

    /// Moves to `line`, opening a block on a key-only assignment and closing
    /// it at the first line that cannot be part of the value.
    fn advance(&mut self, line: &str) {
        if self.covers(line) || line.trim().is_empty() {
            // Blank lines and deeper-indented lines stay inside the block.
            return;
        }
        let indent = line.len() - line.trim_start().len();
        self.key_indent =
            (assigned_key().is_match(line) && !line_contains_credential(line)).then_some(indent);
    }
}

/// Reports whether `value` holds a provider-format secret (a GitHub, GitLab,
/// Slack, Stripe, `OpenAI`, Google, or AWS key, a JWT) or a PEM private key
/// header. These are never acceptable even after span redaction.
#[must_use]
pub fn contains_high_risk_secret(value: &str) -> bool {
    high_risk().is_match(value) || private_key_header().is_match(value)
}

/// Redacts credential spans in place, keeping the surrounding prose:
/// `Bearer <token>` becomes `Bearer [redacted]`, an assigned sensitive key
/// becomes `<key>=[redacted]`, and a provider-format token becomes
/// `[redacted]`. A bare unquoted value spans its whole multi-word run
/// (`password=my secret value` is one value), stopping before any word that
/// carries its own assignment or URL shape so consecutive assignments are
/// each redacted. URLs are left to [`redact_url_userinfo`] so a caller with its
/// own URL normaliser can run it in between.
#[must_use]
pub fn redact_credential_assignments(value: &str) -> String {
    let value = bearer().replace_all(value, "Bearer [redacted]");
    let value = replace_assigned_values(&value);
    high_risk().replace_all(&value, "[redacted]").into_owned()
}

/// Replaces every [`assigned`] span with `<key>=[redacted]`, extending a bare
/// unquoted value over the continuation words of a multi-word value.
fn replace_assigned_values(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    for captures in assigned().captures_iter(value) {
        let matched = captures.get(0).expect("a match always captures itself");
        if matched.start() < cursor {
            continue;
        }
        let mut end = matched.end();
        let raw = captures.get(2).map_or("", |m| m.as_str());
        if is_bare_value(raw) {
            end = extend_bare_value_end(value, end);
        }
        output.push_str(&value[cursor..matched.start()]);
        output.push_str(captures.get(1).map_or("", |m| m.as_str()).trim());
        output.push_str("=[redacted]");
        cursor = end;
    }
    output.push_str(&value[cursor..]);
    output
}

/// A bare unquoted value: no surrounding quotes and no auth scheme, so its
/// words may continue on the following words of the same line.
fn is_bare_value(raw: &str) -> bool {
    !raw.starts_with(['"', '\'']) && !auth_scheme_prefix().is_match(raw)
}

/// Extends `end` over the whitespace-delimited words that continue a bare
/// value, stopping at the first word that carries its own `=`/`:`, quote, or
/// separator — the start of the next assignment, a URL, or trailing prose
/// punctuation.
fn extend_bare_value_end(value: &str, start: usize) -> usize {
    let mut end = start;
    loop {
        let rest = &value[end..];
        let gap = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        if gap == 0 {
            return end;
        }
        let word_start = end + gap;
        let after = &value[word_start..];
        let word_len = after
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after.len());
        let word = &after[..word_len];
        if word.is_empty() || word.contains(['=', ':', ',', ';', '"', '\'']) {
            return end;
        }
        end = word_start + word_len;
    }
}

/// Replaces the userinfo of any `scheme://user:password@` URL with
/// `[redacted]`, keeping the scheme and host.
#[must_use]
pub fn redact_url_userinfo(value: &str) -> String {
    url_userinfo()
        .replace_all(value, "${1}[redacted]@")
        .into_owned()
}

fn line_contains_credential(line: &str) -> bool {
    assigned().is_match(line)
        || bearer_token().is_match(line)
        || high_risk().is_match(line)
        || contains_url_credential(line)
}

/// Reports whether any `scheme://` authority in `value` carries a userinfo
/// with a password (`user:password@host`).
///
/// Every slice is taken with `get`, so a value that ends in `://`, or holds
/// `://` with nothing after it, simply has no authority to inspect.
pub(crate) fn contains_url_credential(value: &str) -> bool {
    let mut start = 0;
    while let Some(scheme) = value.get(start..).and_then(|rest| rest.find("://")) {
        let authority_start = start + scheme + 3;
        let Some(rest) = value.get(authority_start..) else {
            break;
        };
        let authority = rest
            .find(['/', '?', '#', ' ', '\t', '\r', '\n'])
            .map_or(rest, |end| &rest[..end]);
        if let Some(at) = authority.find('@') {
            if authority[..at].contains(':') {
                return true;
            }
            start = authority_start + at + 1;
        } else {
            start = authority_start + authority.len();
        }
    }
    false
}

macro_rules! regex_fn {
    ($name:ident, $pattern:expr) => {
        fn $name() -> &'static Regex {
            static VALUE: OnceLock<Regex> = OnceLock::new();
            VALUE.get_or_init(|| Regex::new($pattern).expect("constant credential regex is valid"))
        }
    };
}

// A sensitive key, optionally quoted (the opening quote joins the span so a
// JSON key redacts cleanly), optionally carrying an identifier prefix
// (`DB_`, `x-`, `spring.datasource.`) and a credential-shaped suffix
// (`_ACCESS_KEY`, `Id`), assigned with `:` or `=` to a nonblank value. The
// value (capture 2) may carry an HTTP auth scheme so `Authorization: Basic …`
// is one span; it is quoted or a single bare token, so a multi-word value is
// completed by `extend_bare_value_end` in code.
regex_fn!(
    assigned,
    concat!(
        r#"(?i)(?:["'`]|(?-u:\b))([a-z0-9_.-]*?"#,
        r"(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?|authorization|cookie)",
        r#"(?:[_-]?(?:access|key|id|value|token|secret))*)["'`]?[ \t]*[:=]+>?[ \t]*"#,
        r"((?:(?:bearer|basic|token|digest|negotiate)[ \t]+)?",
        r#"(?:"[^"\n]*"|'[^'\n]*'|[^ \t\n\f\r,;]+))"#
    )
);
// A key-only assignment line (`db_password:` or `"db_password":`): the same
// sensitive key as `assigned`, alone on its line after the separator. YAML
// and pretty-JSON put the value on the following, deeper-indented line.
regex_fn!(
    assigned_key,
    concat!(
        r#"(?i)^[ \t]*(?:["'`])?([a-z0-9_.-]*?"#,
        r"(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?|authorization|cookie)",
        r#"(?:[_-]?(?:access|key|id|value|token|secret))*)["'`]?[ \t]*[:=]+>?[ \t\r]*$"#
    )
);
regex_fn!(
    auth_scheme_prefix,
    r"(?i)^(?:bearer|basic|token|digest|negotiate)[ \t]"
);
// Span redaction treats any `Bearer <word>` as a credential; detection of a
// whole line needs a token-length value so prose about bearer tokens passes.
regex_fn!(bearer, r"(?i)(?-u:\b)Bearer[ \t]+[A-Za-z0-9._~+/=-]+");
regex_fn!(
    bearer_token,
    r"(?i)(?-u:\b)Bearer[ \t]+[A-Za-z0-9._~+/=-]{16,}"
);
regex_fn!(
    high_risk,
    concat!(
        r"(?i)(?:",
        r"(?-u:\b)sk-[A-Za-z0-9_-]{16,}(?-u:\b)",
        r"|(?-u:\b)(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{12,}(?-u:\b)",
        r"|(?-u:\b)glpat-[A-Za-z0-9_-]{12,}(?-u:\b)",
        r"|(?-u:\b)github_pat_[A-Za-z0-9_]{16,}(?-u:\b)",
        r"|(?-u:\b)gh[pousr]_[A-Za-z0-9]{16,}(?-u:\b)",
        r"|(?-u:\b)(?:AKIA|ASIA)[0-9A-Z]{16}(?-u:\b)",
        r"|(?-u:\b)AIza[0-9A-Za-z_-]{20,}(?-u:\b)",
        r"|(?-u:\b)xox[baprs]-[0-9A-Za-z-]{10,}(?-u:\b)",
        r"|(?-u:\b)(?:secret|private)[_-]key[_-]?[A-Za-z0-9_-]{12,}(?-u:\b)",
        r"|(?-u:\b)eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}(?-u:\b)",
        r")"
    )
);
regex_fn!(
    private_key_header,
    r"(?i)-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----"
);
regex_fn!(
    private_key_footer,
    r"(?i)-----END (?:[A-Z0-9 ]+ )?PRIVATE KEY-----"
);
regex_fn!(
    url_userinfo,
    r"(?i)((?-u:\b)[a-z][a-z0-9+.-]*://)[^/?#\s@]*:[^/?#\s@]*@"
);
