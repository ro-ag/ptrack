use std::collections::BTreeSet;
use std::fmt;
use std::path::{Component, Path};
use std::sync::OnceLock;

use regex::Regex;

use crate::{
    EVENT_MODEL_VERSION, Event, EventKind, EventNotificationKind, EventObservation, EventOutcome,
    EventPhase, Timestamp,
};

const MAX_SOURCE_ID_BYTES: usize = 128;
const MAX_SUBJECT_BYTES: usize = 128;
pub(crate) const MAX_EVENT_PATH_BYTES: usize = 512;
const MAX_PATHS: usize = 16;
const MAX_SUMMARY_BYTES: usize = 2 * 1024;
const MAX_RETAINED: usize = 256;
const MAX_RETENTION_SECONDS: i64 = 30 * 24 * 60 * 60;
const MAX_CLOCK_SKEW_SECONDS: i64 = 5 * 60;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EventPrivacyPolicy {
    pub collection_enabled: bool,
    pub allow_summaries: bool,
    pub retain_last: usize,
    pub retain_for: time::Duration,
}

#[must_use]
pub const fn default_event_privacy_policy() -> EventPrivacyPolicy {
    EventPrivacyPolicy {
        collection_enabled: true,
        allow_summaries: false,
        retain_last: 128,
        retain_for: time::Duration::days(14),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventPrivacyError {
    CollectionDisabled,
    Message(String),
}

impl fmt::Display for EventPrivacyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CollectionDisabled => formatter.write_str("agent event collection is disabled"),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for EventPrivacyError {}

/// Applies the closed event schema, privacy policy, and bounded normalization.
///
/// # Errors
///
/// Returns a fixed, content-free error when evidence crosses any boundary.
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub fn normalize_event_observation(
    project_root: impl AsRef<Path>,
    observed_at: Timestamp,
    policy: EventPrivacyPolicy,
    observation: EventObservation,
) -> Result<EventObservation, EventPrivacyError> {
    validate_policy(policy)?;
    if !policy.collection_enabled {
        return Err(EventPrivacyError::CollectionDisabled);
    }
    if observed_at.is_zero() {
        return message("agent event observation time is required");
    }
    if observation.model_version != EVENT_MODEL_VERSION {
        return message("unsupported agent event model version");
    }
    if observation.source_sequence == 0
        || !observation.kind.is_valid()
        || !observation.phase.is_valid()
    {
        return message("agent event identity, kind, and phase are required");
    }
    if observation.outcome != EventOutcome::Unset && !observation.outcome.is_valid() {
        return message("unsupported agent event outcome");
    }
    if observation.notification != EventNotificationKind::Unset {
        if !observation.notification.is_valid() || !observation.recognized_notification {
            return message("unsupported agent event notification");
        }
        if !valid_notification_event(&observation) {
            return message("agent event notification does not match lifecycle evidence");
        }
    } else if observation.recognized_notification {
        return message("agent event notification is required");
    }

    let source_id = normalized_scalar(&observation.source_id, MAX_SOURCE_ID_BYTES, false)
        .filter(|value| stable_source().is_match(value) && !contains_credential_like(value))
        .ok_or_else(|| {
            EventPrivacyError::Message("invalid agent event source identity".to_owned())
        })?;
    let subject = normalized_scalar(&observation.subject, MAX_SUBJECT_BYTES, true)
        .filter(|value| value.is_empty() || stable_subject().is_match(value))
        .filter(|value| !contains_credential_like(value) && !contains_reasoning_marker(value))
        .ok_or_else(|| EventPrivacyError::Message("invalid agent event subject".to_owned()))?;
    let error_class = normalized_scalar(&observation.error_class, 64, true)
        .filter(|value| value.is_empty() || stable_error_class().is_match(value))
        .filter(|value| !contains_credential_like(value))
        .ok_or_else(|| EventPrivacyError::Message("invalid agent event error class".to_owned()))?;
    let commit_sha = observation.commit_sha.trim().to_ascii_lowercase();
    if !commit_sha.is_empty() && !stable_commit_sha().is_match(&commit_sha) {
        return message("invalid agent event commit identity");
    }
    if !commit_sha.is_empty() && observation.kind != EventKind::Commit {
        return message("agent event commit identity is not allowed for this kind");
    }
    if observation.exit_code.is_some()
        && !matches!(
            observation.kind,
            EventKind::Lifecycle | EventKind::Command | EventKind::Test
        )
    {
        return message("agent event exit code is not allowed for this kind");
    }
    if !error_class.is_empty()
        && observation.kind != EventKind::Error
        && !matches!(observation.phase, EventPhase::Blocked | EventPhase::Failed)
    {
        return message("agent event error class is not allowed for this phase");
    }
    let paths = normalize_paths(project_root.as_ref(), &observation.paths)?;
    let mut summary = observation.summary.trim().to_owned();
    if !summary.is_empty() {
        if observation.kind != EventKind::Summary
            || observation.phase != EventPhase::Completed
            || !policy.allow_summaries
        {
            return message("agent event summaries are not allowed");
        }
        if contains_reasoning_marker(&summary) {
            return message("agent event summary contains disallowed reasoning content");
        }
        if high_risk().is_match(&summary) || private_key().is_match(&summary) {
            return message("agent event summary contains disallowed credential content");
        }
        summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
        summary = redact_summary(&summary);
        if !valid_text(&summary, true) || summary.len() > MAX_SUMMARY_BYTES {
            return message("agent event summary exceeds the privacy boundary");
        }
    } else if observation.kind == EventKind::Summary {
        return message("agent summary event requires a summary");
    }
    let occurred_at = if observation.occurred_at.is_zero() {
        observed_at
    } else {
        observation.occurred_at
    };
    if occurred_at > observed_at.add_seconds(MAX_CLOCK_SKEW_SECONDS)
        || occurred_at < observed_at.add_seconds(-MAX_RETENTION_SECONDS)
    {
        return message("agent event occurrence time is outside the accepted window");
    }
    Ok(EventObservation {
        source_id,
        subject,
        paths,
        commit_sha,
        error_class,
        summary,
        occurred_at,
        ..observation
    })
}

/// Applies retention age/count bounds and canonical ordering.
///
/// # Errors
///
/// Returns an error when the supplied policy is invalid.
pub fn retain_events(
    events: &[Event],
    observed_at: Timestamp,
    policy: EventPrivacyPolicy,
) -> Result<Vec<Event>, EventPrivacyError> {
    validate_policy(policy)?;
    if !policy.collection_enabled {
        return Ok(Vec::new());
    }
    let cutoff = observed_at.add_nanoseconds(-policy.retain_for.whole_nanoseconds());
    let mut retained: Vec<Event> = events
        .iter()
        .filter(|event| event.observed_at >= cutoff)
        .cloned()
        .collect();
    retained.sort_by(|left, right| {
        left.host_sequence
            .cmp(&right.host_sequence)
            .then(left.observed_at.cmp(&right.observed_at))
            .then(left.id.cmp(&right.id))
    });
    if retained.len() > policy.retain_last {
        retained.drain(..retained.len() - policy.retain_last);
    }
    Ok(retained)
}

fn validate_policy(policy: EventPrivacyPolicy) -> Result<(), EventPrivacyError> {
    if !policy.collection_enabled {
        return Ok(());
    }
    if policy.retain_last == 0 || policy.retain_last > MAX_RETAINED {
        return message(format!(
            "agent event retention count must be between 1 and {MAX_RETAINED}"
        ));
    }
    if policy.retain_for <= time::Duration::ZERO
        || policy.retain_for > time::Duration::seconds(MAX_RETENTION_SECONDS)
    {
        return message("agent event retention age must be between 1ns and 720h0m0s");
    }
    Ok(())
}

fn valid_notification_event(value: &EventObservation) -> bool {
    if !value.subject.is_empty()
        || !value.paths.is_empty()
        || !value.commit_sha.is_empty()
        || value.exit_code.is_some()
        || !value.summary.is_empty()
    {
        return false;
    }
    match value.notification {
        EventNotificationKind::ApprovalRequested | EventNotificationKind::Question => {
            value.kind == EventKind::Lifecycle
                && value.phase == EventPhase::Waiting
                && value.outcome == EventOutcome::Unset
                && value.error_class.is_empty()
        }
        EventNotificationKind::Failure => {
            matches!(value.kind, EventKind::Lifecycle | EventKind::Error)
                && value.phase == EventPhase::Failed
                && value.outcome == EventOutcome::Failed
        }
        EventNotificationKind::Completion => {
            value.kind == EventKind::Lifecycle
                && value.phase == EventPhase::Completed
                && value.outcome == EventOutcome::Succeeded
                && value.error_class.is_empty()
        }
        EventNotificationKind::Unset => false,
    }
}

fn normalized_scalar(value: &str, maximum: usize, optional: bool) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return optional.then(String::new);
    }
    (value.len() <= maximum && valid_text(value, false)).then(|| value.to_owned())
}

pub(crate) fn valid_text(value: &str, allow_lines: bool) -> bool {
    !value.chars().any(|character| {
        character == '\0'
            || (character < '\u{20}' && !(allow_lines && matches!(character, '\n' | '\t')))
    })
}

fn normalize_paths(
    project_root: &Path,
    paths: &[String],
) -> Result<Vec<String>, EventPrivacyError> {
    if paths.len() > MAX_PATHS {
        return message("agent event has too many paths");
    }
    let root = if project_root.is_absolute() {
        project_root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| {
                EventPrivacyError::Message("agent event project root is invalid".to_owned())
            })?
            .join(project_root)
    };
    let mut output = BTreeSet::new();
    for path in paths {
        if path.is_empty()
            || path.len() > MAX_EVENT_PATH_BYTES
            || !valid_text(path, false)
            || Path::new(path).is_absolute()
        {
            return message("agent event path is invalid");
        }
        let mut clean = Vec::new();
        for component in Path::new(path).components() {
            match component {
                Component::Normal(value) => clean.push(value.to_string_lossy().into_owned()),
                Component::CurDir => {}
                Component::ParentDir => {
                    if clean.pop().is_none() {
                        return message("agent event path is outside the project");
                    }
                }
                Component::RootDir | Component::Prefix(_) => {
                    return message("agent event path is invalid");
                }
            }
        }
        if clean.is_empty() {
            return message("agent event path is outside the project");
        }
        let portable = clean.join("/");
        let absolute = root.join(&portable);
        if !absolute.starts_with(&root) {
            return message("agent event path is outside the project");
        }
        if contains_credential_like(&portable) || contains_reasoning_marker(&portable) {
            return message("agent event path crosses the privacy boundary");
        }
        output.insert(portable);
    }
    Ok(output.into_iter().collect())
}

pub(crate) fn contains_reasoning_marker(value: &str) -> bool {
    let lower = go_unicode_lower(value);
    [
        "<thinking",
        "</thinking",
        "<analysis",
        "</analysis",
        "chain of thought",
        "chain-of-thought",
        "internal reasoning:",
        "private reasoning:",
        "step-by-step reasoning",
        "step by step reasoning",
        "thought process:",
        "hidden rationale:",
        "private deliberation:",
        "scratchpad:",
        "my reasoning",
        "reasoning was",
        "i reasoned",
        "rationale:",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

pub(crate) fn contains_credential_like(value: &str) -> bool {
    bearer().is_match(value)
        || assigned().is_match(value)
        || high_risk().is_match(value)
        || private_key().is_match(value)
}

pub(crate) fn contains_rejected_summary_credential(value: &str) -> bool {
    high_risk().is_match(value) || private_key().is_match(value)
}

pub(crate) fn redact_summary(value: &str) -> String {
    let value = bearer().replace_all(value, "Bearer [redacted]");
    let value = assigned().replace_all(&value, |captures: &regex::Captures<'_>| {
        format!(
            "{}=[redacted]",
            captures.get(1).map_or("", |value| value.as_str()).trim()
        )
    });
    http_url()
        .replace_all(&value, |captures: &regex::Captures<'_>| {
            redact_url(&captures[0])
        })
        .into_owned()
}

fn redact_url(raw: &str) -> String {
    let core = raw.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', ']', '}']);
    let trailing = &raw[core.len()..];
    redact_http_url(core).map_or_else(
        || format!("[redacted-url]{trailing}"),
        |redacted| format!("{redacted}{trailing}"),
    )
}

fn redact_http_url(value: &str) -> Option<String> {
    if value.len() > MAX_SUMMARY_BYTES || contains_ascii_control(value) {
        return None;
    }
    let (without_fragment, fragment) = value.split_once('#').unwrap_or((value, ""));
    if !valid_percent_encoding(fragment) {
        return None;
    }
    let (scheme, remainder) = if let Some(remainder) = without_fragment.strip_prefix("http://") {
        ("http", remainder)
    } else {
        ("https", without_fragment.strip_prefix("https://")?)
    };
    let (authority_and_path, query, force_query) = split_query(remainder);
    let (authority, raw_path) = authority_and_path
        .split_once('/')
        .map_or((authority_and_path, ""), |(authority, _)| {
            (authority, &authority_and_path[authority.len()..])
        });
    let (userinfo, raw_host) = authority
        .rsplit_once('@')
        .map_or((None, authority), |(userinfo, host)| (Some(userinfo), host));
    if userinfo.is_some_and(|userinfo| !valid_userinfo(userinfo)) {
        return None;
    }
    let host = render_host(raw_host)?;
    if host.is_empty() {
        return None;
    }
    let path = render_path(raw_path)?;
    let mut output = format!("{scheme}://{host}{path}");
    if force_query {
        output.push('?');
    } else if query.is_some_and(|query| !query.is_empty()) {
        output.push_str("?redacted");
    }
    Some(output)
}

fn split_query(value: &str) -> (&str, Option<&str>, bool) {
    if value.ends_with('?') && value.bytes().filter(|byte| *byte == b'?').count() == 1 {
        return (&value[..value.len() - 1], None, true);
    }
    value
        .split_once('?')
        .map_or((value, None, false), |(head, query)| {
            (head, Some(query), false)
        })
}

fn valid_userinfo(value: &str) -> bool {
    valid_percent_encoding(value)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.'
                        | b'_'
                        | b':'
                        | b'~'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                        | b'%'
                        | b'@'
                )
        })
}

fn render_host(value: &str) -> Option<String> {
    let decoded = if value.starts_with('[') {
        decode_bracketed_ipv6_host(value)?
    } else {
        validate_unbracketed_host_port(value)?;
        decode_host(value)?
    };
    let mut output = String::with_capacity(decoded.len());
    for byte in decoded {
        if host_byte_allowed(byte) {
            output.push(char::from(byte));
        } else {
            push_percent_encoded(&mut output, byte);
        }
    }
    Some(output)
}

fn validate_unbracketed_host_port(value: &str) -> Option<()> {
    let mut colons = value.match_indices(':');
    let first = colons.next();
    if colons.next().is_some() {
        return None;
    }
    first.map_or(Some(()), |(index, _)| {
        value[index + 1..]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then_some(())
    })
}

fn decode_bracketed_ipv6_host(value: &str) -> Option<Vec<u8>> {
    let close = value.rfind(']')?;
    let raw_address = value.get(1..close)?;
    let port = &value[close + 1..];
    if !port.is_empty()
        && !port
            .strip_prefix(':')
            .is_some_and(|digits| digits.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let (raw_ip, raw_zone) = raw_address
        .split_once("%25")
        .map_or((raw_address, None), |(ip, zone)| (ip, Some(zone)));
    let decoded_ip = decode_host(raw_ip)?;
    let ip_text = std::str::from_utf8(&decoded_ip).ok()?;
    ip_text.parse::<std::net::Ipv6Addr>().ok()?;

    let mut output = Vec::with_capacity(value.len());
    output.push(b'[');
    output.extend_from_slice(&decoded_ip);
    if let Some(raw_zone) = raw_zone {
        let zone = decode_zone(raw_zone)?;
        if zone.is_empty() {
            return None;
        }
        output.push(b'%');
        output.extend_from_slice(&zone);
    }
    output.push(b']');
    output.extend_from_slice(port.as_bytes());
    Some(output)
}

fn decode_zone(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            if bytes[index].is_ascii() && !host_byte_allowed(bytes[index]) {
                return None;
            }
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        let decoded = decode_percent(bytes, index)?;
        if decoded != b' ' && decoded != b'%' && !host_byte_allowed(decoded) {
            return None;
        }
        output.push(decoded);
        index += 3;
    }
    Some(output)
}

fn decode_host(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            if bytes[index].is_ascii() && !host_byte_allowed(bytes[index]) {
                return None;
            }
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        let decoded = decode_percent(bytes, index)?;
        if decoded.is_ascii() && decoded != b'%' {
            return None;
        }
        output.push(decoded);
        index += 3;
    }
    Some(output)
}

fn render_path(value: &str) -> Option<String> {
    let decoded = decode_percent_encoded(value)?;
    let escaped = escape_path(&decoded);
    if value != escaped && valid_raw_path(value) {
        Some(value.to_owned())
    } else {
        Some(escaped)
    }
}

fn escape_path(value: &[u8]) -> String {
    let mut output = String::with_capacity(value.len());
    for &byte in value {
        if path_byte_allowed(byte) {
            output.push(char::from(byte));
        } else {
            push_percent_encoded(&mut output, byte);
        }
    }
    output
}

fn valid_raw_path(value: &str) -> bool {
    valid_percent_encoding(value)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.'
                        | b'_'
                        | b'~'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                        | b':'
                        | b'@'
                        | b'/'
                        | b'['
                        | b']'
                        | b'%'
                )
        })
}

fn decode_percent_encoded(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            output.push(decode_percent(bytes, index)?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    Some(output)
}

fn valid_percent_encoding(value: &str) -> bool {
    decode_percent_encoded(value).is_some()
}

fn decode_percent(bytes: &[u8], index: usize) -> Option<u8> {
    let high = hex_value(*bytes.get(index + 1)?)?;
    let low = hex_value(*bytes.get(index + 2)?)?;
    Some((high << 4) | low)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn push_percent_encoded(output: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    output.push('%');
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

const fn host_byte_allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'"'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b'-'
                | b'.'
                | b':'
                | b';'
                | b'<'
                | b'='
                | b'>'
                | b'['
                | b']'
                | b'_'
                | b'~'
        )
}

const fn path_byte_allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'$' | b'&'
                | b'+'
                | b','
                | b'-'
                | b'.'
                | b'/'
                | b':'
                | b';'
                | b'='
                | b'@'
                | b'_'
                | b'~'
        )
}

fn contains_ascii_control(value: &str) -> bool {
    value.bytes().any(|byte| byte < b' ' || byte == 0x7f)
}

fn go_unicode_lower(value: &str) -> String {
    value
        .chars()
        .map(|character| character.to_lowercase().next().unwrap_or(character))
        .collect()
}

fn message<T>(value: impl Into<String>) -> Result<T, EventPrivacyError> {
    Err(EventPrivacyError::Message(value.into()))
}

macro_rules! regex_fn {
    ($name:ident, $pattern:literal) => {
        fn $name() -> &'static Regex {
            static VALUE: OnceLock<Regex> = OnceLock::new();
            VALUE.get_or_init(|| Regex::new($pattern).expect("constant privacy regex is valid"))
        }
    };
}

regex_fn!(stable_source, r"^[A-Za-z0-9][A-Za-z0-9._:-]*$");
regex_fn!(stable_subject, r"^[A-Za-z0-9][A-Za-z0-9._:/@+-]*$");
regex_fn!(stable_error_class, r"^[a-z][a-z0-9_.-]*$");
regex_fn!(stable_commit_sha, r"^[0-9a-fA-F]{7,64}$");
regex_fn!(bearer, r"(?i)(?-u:\b)Bearer[ \t]+[A-Za-z0-9._~+/=-]+");
regex_fn!(
    assigned,
    r#"(?i)(?-u:\b)(token|password|passwd|secret|api[_-]?key|authorization|cookie)[ \t]*[:=][ \t]*(?:"[^"]*"|'[^']*'|[^ \t\n\f\r,;]+)"#
);
regex_fn!(
    high_risk,
    r"(?i)(?:(?-u:\b)sk-[A-Za-z0-9_-]{16,}(?-u:\b)|(?-u:\b)(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{12,}(?-u:\b)|(?-u:\b)glpat-[A-Za-z0-9_-]{12,}(?-u:\b)|(?-u:\b)github_pat_[A-Za-z0-9_]{16,}(?-u:\b)|(?-u:\b)gh[pousr]_[A-Za-z0-9]{16,}(?-u:\b)|(?-u:\b)AKIA[0-9A-Z]{16}(?-u:\b)|(?-u:\b)AIza[0-9A-Za-z_-]{20,}(?-u:\b)|(?-u:\b)xox[baprs]-[0-9A-Za-z-]{10,}(?-u:\b)|(?-u:\b)(?:secret|private)[_-]key[_-]?[A-Za-z0-9_-]{12,}(?-u:\b)|(?-u:\b)eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}(?-u:\b))"
);
regex_fn!(
    private_key,
    r"(?i)-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----"
);
regex_fn!(http_url, r"https?://[^ \t\n\f\r]+");
