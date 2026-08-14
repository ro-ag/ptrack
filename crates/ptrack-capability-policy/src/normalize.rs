use std::collections::BTreeSet;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path};
use std::str::FromStr;
use std::sync::OnceLock;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use percent_encoding::percent_decode_str;
use ptrack_core::{
    CAPABILITY_MODEL_VERSION, Capability, CapabilityKind, Digest32, GitScope, HttpScope, SshScope,
};
use regex::Regex;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::wire::{
    CapabilityAuditPolicyWire, CapabilityLimitsWire, GitScopeWire, HttpScopeWire, SshScopeWire,
};

const DEFAULT_APPROVAL_SECONDS: i64 = 3_600;
const MAX_APPROVAL_SECONDS: i64 = 30 * 24 * 3_600;
const DEFAULT_TIMEOUT_SECONDS: i64 = 30;
const MAX_TIMEOUT_SECONDS: i64 = 300;
const DEFAULT_REQUEST_BYTES: i64 = 1 << 20;
const DEFAULT_RESPONSE_BYTES: i64 = 4 << 20;
const DEFAULT_OUTPUT_BYTES: i64 = 1 << 20;
const MAX_TRANSFER_BYTES: i64 = 32 << 20;
const MAX_REDIRECTS: i64 = 10;
const DEFAULT_CONCURRENT: i64 = 1;
const MAX_CONCURRENT: i64 = 8;
const DEFAULT_AUDIT_RECORDS: i64 = 100;
const MAX_AUDIT_RECORDS: i64 = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityError(String);

impl CapabilityError {
    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CapabilityError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Preview {
    pub capability: Capability,
    pub effective_scope: String,
    pub scope_digest: Digest32,
}

/// Canonicalizes a draft and computes its Go-compatible approval digest.
///
/// # Errors
/// Returns an error when any field lies outside the explicit capability contract.
pub fn normalize(input: &Capability) -> Result<Preview, CapabilityError> {
    let mut capability = input.clone();
    let trimmed_name = capability.name.trim().to_owned();
    capability.name = trimmed_name;
    if capability.name.is_empty() || capability.name.len() > 128 || has_control(&capability.name) {
        return error("capability name must be 1-128 printable characters");
    }
    if capability.model_version == 0 {
        capability.model_version = CAPABILITY_MODEL_VERSION;
    }
    if capability.model_version != CAPABILITY_MODEL_VERSION {
        return error(format!(
            "unsupported capability model version {}",
            capability.model_version
        ));
    }
    capability.agent_profile = normalize_profile(&capability.agent_profile)?;
    if capability.approval_duration_seconds == 0 {
        capability.approval_duration_seconds = DEFAULT_APPROVAL_SECONDS;
    }
    if !(60..=MAX_APPROVAL_SECONDS).contains(&capability.approval_duration_seconds) {
        return error(format!(
            "approval duration must be between 60 and {MAX_APPROVAL_SECONDS} seconds"
        ));
    }
    normalize_limits(&mut capability)?;
    if capability.audit.retain_last == 0 {
        capability.audit.retain_last = DEFAULT_AUDIT_RECORDS;
    }
    if !(0..=MAX_AUDIT_RECORDS).contains(&capability.audit.retain_last) {
        return error(format!(
            "audit retention must be between 0 and {MAX_AUDIT_RECORDS} records"
        ));
    }

    let kind_scope = match capability.kind {
        CapabilityKind::Http => {
            let Some(scope) = capability.http.as_mut() else {
                return error("HTTP capability must contain only an HTTP scope");
            };
            if capability.git.is_some() || capability.ssh.is_some() {
                return error("HTTP capability must contain only an HTTP scope");
            }
            normalize_http(scope)?
        }
        CapabilityKind::Git => {
            let Some(scope) = capability.git.as_mut() else {
                return error("Git capability must contain only a Git scope");
            };
            if capability.http.is_some() || capability.ssh.is_some() {
                return error("Git capability must contain only a Git scope");
            }
            normalize_git(scope)?
        }
        CapabilityKind::Ssh => {
            let Some(scope) = capability.ssh.as_mut() else {
                return error("SSH capability must contain only an SSH scope");
            };
            if capability.http.is_some() || capability.git.is_some() {
                return error("SSH capability must contain only an SSH scope");
            }
            normalize_ssh(scope)?
        }
    };
    let digest = scope_digest(&capability)?;
    capability.scope_digest = digest;
    Ok(Preview {
        effective_scope: effective_approval_scope(&capability, &kind_scope),
        capability,
        scope_digest: digest,
    })
}

fn normalize_limits(capability: &mut Capability) -> Result<(), CapabilityError> {
    let limits = &mut capability.limits;
    if limits.timeout_seconds == 0 {
        limits.timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
    }
    if limits.max_request_bytes == 0 {
        limits.max_request_bytes = DEFAULT_REQUEST_BYTES;
    }
    if limits.max_response_bytes == 0 {
        limits.max_response_bytes = DEFAULT_RESPONSE_BYTES;
    }
    if limits.max_output_bytes == 0 {
        limits.max_output_bytes = DEFAULT_OUTPUT_BYTES;
    }
    if limits.max_concurrent == 0 {
        limits.max_concurrent = DEFAULT_CONCURRENT;
    }
    if !(1..=MAX_TIMEOUT_SECONDS).contains(&limits.timeout_seconds) {
        return error(format!(
            "timeout must be between 1 and {MAX_TIMEOUT_SECONDS} seconds"
        ));
    }
    for (name, value) in [
        ("request", limits.max_request_bytes),
        ("response", limits.max_response_bytes),
        ("output", limits.max_output_bytes),
    ] {
        if !(1..=MAX_TRANSFER_BYTES).contains(&value) {
            return error(format!(
                "maximum {name} bytes must be between 1 and {MAX_TRANSFER_BYTES}"
            ));
        }
    }
    if !(0..=MAX_REDIRECTS).contains(&limits.max_redirects) {
        return error(format!(
            "maximum redirects must be between 0 and {MAX_REDIRECTS}"
        ));
    }
    if !(1..=MAX_CONCURRENT).contains(&limits.max_concurrent) {
        return error(format!(
            "maximum concurrent operations must be between 1 and {MAX_CONCURRENT}"
        ));
    }
    Ok(())
}

fn normalize_http(scope: &mut HttpScope) -> Result<String, CapabilityError> {
    let base = normalize_http_url(&scope.base_url, false)
        .map_err(|error| CapabilityError::message(format!("HTTP base URL: {error}")))?;
    if !base.query.is_empty() || !base.fragment.is_empty() {
        return error("HTTP base URL cannot contain a query or fragment");
    }
    scope.base_url = base.without_query_fragment();
    let mut methods = Vec::with_capacity(scope.methods.len());
    for raw in &scope.methods {
        let method = raw.trim().to_uppercase();
        if !matches!(
            method.as_str(),
            "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
        ) {
            return error(format!("HTTP method {raw:?} is not supported"));
        }
        methods.push(method);
    }
    scope.methods = unique_sorted(methods);
    if scope.methods.is_empty() {
        return error("HTTP scope must approve at least one method");
    }
    if scope.path_prefixes.is_empty() {
        scope.path_prefixes.push(base.path.clone());
    }
    let mut paths = Vec::with_capacity(scope.path_prefixes.len());
    for raw in &scope.path_prefixes {
        let prefix = normalize_scope_path(raw)?;
        if !path_within(&base.path, &prefix) {
            return error(format!(
                "HTTP path {prefix:?} is outside base path {:?}",
                base.path
            ));
        }
        paths.push(prefix);
    }
    scope.path_prefixes = unique_sorted(paths);
    Ok(format!(
        "{} {} paths={}",
        scope.methods.join(","),
        scope.base_url,
        scope.path_prefixes.join(",")
    ))
}

#[derive(Clone, Debug)]
pub(crate) struct NormalizedHttpUrl {
    pub url: String,
    pub scheme: String,
    pub host: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

impl NormalizedHttpUrl {
    fn without_query_fragment(&self) -> String {
        let mut result = self.url.clone();
        if let Some(index) = result.find(['?', '#']) {
            result.truncate(index);
        }
        result
    }
}

#[derive(Clone, Debug)]
struct ParsedAbsoluteUrl {
    scheme: String,
    userinfo: Option<(String, Option<String>)>,
    host: String,
    port: Option<String>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
}

fn parse_absolute_url(value: &str) -> Result<ParsedAbsoluteUrl, ()> {
    let (raw_scheme, remainder) = value.split_once("://").ok_or(())?;
    if raw_scheme.is_empty()
        || !raw_scheme.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic()
                || (index > 0 && (byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.')))
        })
    {
        return Err(());
    }
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    if authority.is_empty() || authority.chars().any(char::is_whitespace) {
        return Err(());
    }
    let tail = &remainder[authority_end..];
    let (before_fragment, fragment) = tail
        .split_once('#')
        .map_or((tail, None), |(before, after)| {
            (before, Some(after.to_owned()))
        });
    let (raw_path, query) = before_fragment
        .split_once('?')
        .map_or((before_fragment, None), |(path, query)| {
            (path, Some(query.to_owned()))
        });
    let path = if raw_path.is_empty() { "/" } else { raw_path };
    if !path.starts_with('/') {
        return Err(());
    }

    let (userinfo, host_port) =
        authority
            .rsplit_once('@')
            .map_or((None, authority), |(raw, host)| {
                let (user, password) = raw
                    .split_once(':')
                    .map_or((raw.to_owned(), None), |(user, password)| {
                        (user.to_owned(), Some(password.to_owned()))
                    });
                (Some((user, password)), host)
            });
    if host_port.is_empty() || host_port.contains('@') {
        return Err(());
    }
    let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
        let (host, suffix) = bracketed.split_once(']').ok_or(())?;
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .filter(|value| !value.is_empty())
                    .ok_or(())?
                    .to_owned(),
            )
        };
        (host.to_owned(), port)
    } else {
        if host_port.matches(':').count() > 1 {
            return Err(());
        }
        host_port
            .rsplit_once(':')
            .map_or((host_port.to_owned(), None), |(host, port)| {
                (host.to_owned(), Some(port.to_owned()))
            })
    };
    if host.is_empty() || port.as_ref().is_some_and(String::is_empty) {
        return Err(());
    }
    Ok(ParsedAbsoluteUrl {
        scheme: raw_scheme.to_owned(),
        userinfo,
        host,
        port,
        path: path.to_owned(),
        query,
        fragment,
    })
}

fn normalize_url_port(raw: Option<&str>, scheme: &str) -> Result<Option<String>, CapabilityError> {
    let Some(raw) = raw else { return Ok(None) };
    let value = raw
        .parse::<u16>()
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| CapabilityError::message("port must be between 1 and 65535"))?;
    if (scheme == "http" && raw == "80")
        || (scheme == "https" && raw == "443")
        || (scheme == "ssh" && raw == "22")
    {
        Ok(None)
    } else {
        let _ = value;
        Ok(Some(raw.to_owned()))
    }
}

fn encode_url_path(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.' | b'_' | b'~' | b'/' | b':' | b'@' | b'&' | b'=' | b'+' | b'$'
            )
        {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    encoded
}

pub(crate) fn normalize_http_url(
    raw: &str,
    allow_query: bool,
) -> Result<NormalizedHttpUrl, CapabilityError> {
    let value = raw.trim();
    if has_control(value) {
        return error("URL contains control characters");
    }
    let parsed = parse_absolute_url(value)
        .map_err(|()| CapabilityError::message("URL must be an absolute hierarchical URL"))?;
    let raw_path = &parsed.path;
    let raw_path_lower = raw_path.to_ascii_lowercase();
    if raw_path_lower.contains("%2f")
        || raw_path_lower.contains("%5c")
        || raw_path_lower.contains("%2e")
    {
        return error("encoded slash, backslash, or dot path segments are not allowed");
    }
    let scheme = parsed.scheme.to_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return error("URL scheme must be http or https");
    }
    if parsed.userinfo.is_some() {
        return error("embedded URL credentials are not allowed");
    }
    if !allow_query && parsed.query.is_some() {
        return error("URL query is not allowed in an approved scope");
    }
    let host = normalize_host(&parsed.host)?;
    let port = normalize_url_port(parsed.port.as_deref(), &scheme)?;
    let escaped = &parsed.path;
    let decoded = percent_decode_str(escaped)
        .decode_utf8()
        .map_err(|_| CapabilityError::message("URL path is ambiguous"))?;
    if decoded.contains(['\\', '%']) || has_control(&decoded) {
        return error("URL path is ambiguous");
    }
    let path = normalize_web_path(&decoded);
    let authority = format_authority_text(&host, port.as_deref());
    let query = parsed.query.unwrap_or_default();
    let fragment = parsed.fragment.unwrap_or_default();
    let mut url = format!("{scheme}://{authority}{}", encode_url_path(&path));
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query);
    }
    if !fragment.is_empty() {
        url.push('#');
        url.push_str(&fragment);
    }
    Ok(NormalizedHttpUrl {
        url,
        scheme,
        host: authority,
        path,
        query,
        fragment,
    })
}

fn normalize_scope_path(raw: &str) -> Result<String, CapabilityError> {
    if !raw.starts_with('/') || raw.contains(['?', '#']) || has_control(raw) {
        return error(format!(
            "HTTP path prefix {raw:?} must be an absolute path without query or fragment"
        ));
    }
    let lower = raw.to_ascii_lowercase();
    if lower.contains("%2f") || lower.contains("%5c") || lower.contains("%2e") {
        return error(format!(
            "HTTP path prefix {raw:?} contains ambiguous encoding"
        ));
    }
    let decoded = percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| CapabilityError::message(format!("HTTP path prefix {raw:?} is ambiguous")))?;
    if decoded.contains(['\\', '%']) || has_control(&decoded) {
        return error(format!("HTTP path prefix {raw:?} is ambiguous"));
    }
    Ok(normalize_web_path(&decoded))
}

pub(crate) fn normalize_web_path(value: &str) -> String {
    let mut parts = Vec::new();
    for part in value.trim_start_matches('/').split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", parts.join("/"))
    }
}

pub(crate) fn path_within(parent: &str, child: &str) -> bool {
    let parent = normalize_web_path(parent);
    let child = normalize_web_path(child);
    parent == "/"
        || child == parent
        || child.starts_with(&format!("{}/", parent.trim_end_matches('/')))
}

fn normalize_git(scope: &mut GitScope) -> Result<String, CapabilityError> {
    scope.remote_name = normalize_token(&scope.remote_name, "Git remote name", true)?;
    scope.remote_url = normalize_git_remote(&scope.remote_url)?;
    let mut operations = Vec::with_capacity(scope.operations.len());
    for raw in &scope.operations {
        let operation = raw.trim().to_lowercase();
        if !matches!(
            operation.as_str(),
            "status" | "fetch" | "pull" | "push" | "ls-remote"
        ) {
            return error(format!("Git operation {raw:?} is not supported"));
        }
        operations.push(operation);
    }
    scope.operations = unique_sorted(operations);
    if scope.operations.is_empty() {
        return error("Git scope must approve at least one operation");
    }
    for branch in &scope.branches {
        validate_git_branch(branch).map_err(|reason| {
            CapabilityError::message(format!("Git branch {branch:?}: {reason}"))
        })?;
    }
    scope.branches = unique_sorted(scope.branches.clone());
    let mut refspecs = Vec::with_capacity(scope.refspecs.len());
    for refspec in &scope.refspecs {
        refspecs.push(normalize_refspec(refspec, scope)?);
    }
    scope.refspecs = unique_sorted(refspecs);
    Ok(format!(
        "remote {}={} operations={} branches={} refspecs={} allow_tags={} allow_force_with_lease={} allow_delete_refs={}",
        scope.remote_name,
        scope.remote_url,
        scope_list(&scope.operations),
        scope_list(&scope.branches),
        scope_list(&scope.refspecs),
        scope.allow_tags,
        scope.allow_force_push,
        scope.allow_delete_refs
    ))
}

pub(crate) fn normalize_git_remote(raw: &str) -> Result<String, CapabilityError> {
    let value = raw.trim();
    if value.is_empty() || value.starts_with('-') || has_control(value) {
        return error("Git remote URL is invalid");
    }
    if value.contains("://") {
        let parsed = parse_absolute_url(value).map_err(|()| {
            CapabilityError::message("Git remote must be an absolute URL without query or fragment")
        })?;
        let scheme = parsed.scheme.to_lowercase();
        if parsed.query.is_some() || parsed.fragment.is_some() {
            return error("Git remote must be an absolute URL without query or fragment");
        }
        if !matches!(scheme.as_str(), "https" | "ssh") {
            return error("Git remote scheme must be https or ssh");
        }
        let user = match parsed.userinfo {
            Some((user, password)) if scheme == "ssh" && password.is_none() && !user.is_empty() => {
                Some(normalize_token(&user, "SSH user", true)?)
            }
            Some(_) => return error("embedded Git credentials are not allowed"),
            None => None,
        };
        let host = normalize_host(&parsed.host)?;
        let port = normalize_url_port(parsed.port.as_deref(), &scheme)?;
        let mut authority = format_authority_text(&host, port.as_deref());
        if let Some(user) = user {
            authority = format!("{user}@{authority}");
        }
        let path = normalize_git_path(&parsed.path)?;
        return Ok(format!("{scheme}://{authority}{}", encode_url_path(&path)));
    }
    if value.split_once(':').is_some_and(|(prefix, _)| {
        matches!(
            prefix.to_ascii_lowercase().as_str(),
            "http" | "https" | "ssh"
        )
    }) {
        return error("Git remote must use https, ssh, or SCP-style SSH syntax");
    }
    let (identity, remote_path) = value
        .split_once(':')
        .filter(|(identity, path)| !identity.is_empty() && !path.is_empty())
        .ok_or_else(|| {
            CapabilityError::message("Git remote must use https, ssh, or SCP-style SSH syntax")
        })?;
    let (user, raw_host) = identity
        .rsplit_once('@')
        .map_or((None, identity), |(user, host)| (Some(user), host));
    let host = normalize_host(raw_host)?;
    let path = normalize_git_path(remote_path)?;
    let identity = if let Some(user) = user {
        format!("{}@{host}", normalize_token(user, "SSH user", true)?)
    } else {
        host
    };
    Ok(format!("{identity}:{}", path.trim_start_matches('/')))
}

fn normalize_git_path(raw: &str) -> Result<String, CapabilityError> {
    let decoded = percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| CapabilityError::message("Git remote path is invalid"))?;
    if decoded.is_empty() || has_control(&decoded) || decoded.contains(['\\', '%']) {
        return error("Git remote path is invalid");
    }
    if decoded
        .split('/')
        .any(|segment| matches!(segment, "." | ".."))
    {
        return error("Git remote path cannot contain dot segments");
    }
    Ok(clean_posix(&decoded, decoded.starts_with('/')))
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn validate_git_ref(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.starts_with('-')
        || value.ends_with(['.', '/'])
        || value.contains("..")
        || value.contains("@{")
        || value.contains([' ', '~', '^', ':', '?', '*', '[', '\\'])
        || has_control(value)
    {
        return Err("invalid ref name");
    }
    if value.split('/').any(|part| {
        part.is_empty()
            || matches!(part, "." | "..")
            || part.starts_with('.')
            || part.ends_with(".lock")
    }) {
        return Err("invalid ref component");
    }
    Ok(())
}

fn validate_git_branch(value: &str) -> Result<(), &'static str> {
    let upper = value.to_uppercase();
    if value.starts_with('+')
        || value.starts_with("refs/")
        || value.contains(':')
        || value == "@"
        || upper == "HEAD"
        || upper.ends_with("_HEAD")
        || upper == "AUTO_MERGE"
    {
        return Err("branch must be an unqualified head name");
    }
    validate_git_ref(value)
}

fn normalize_refspec(raw: &str, scope: &GitScope) -> Result<String, CapabilityError> {
    if raw.is_empty() || raw.starts_with('-') || raw.contains('*') || has_control(raw) {
        return error(format!("Git refspec {raw:?} is invalid"));
    }
    if raw.starts_with('+') {
        return error(format!(
            "Git refspec {raw:?} uses unconditional force; use the separate force-with-lease request"
        ));
    }
    let mut parts = raw.split(':').map(str::to_owned).collect::<Vec<_>>();
    if parts.len() > 2 {
        return error(format!("Git refspec {raw:?} is invalid"));
    }
    if parts.len() == 2 && parts[0].is_empty() && !scope.allow_delete_refs {
        return error(format!(
            "Git refspec {raw:?} requires ref-deletion approval"
        ));
    }
    for part in &mut parts {
        if part.is_empty() {
            continue;
        }
        if let Some(branch) = part.strip_prefix("refs/heads/") {
            validate_git_branch(branch)
                .map_err(|_| CapabilityError::message(format!("Git refspec {raw:?} is invalid")))?;
        } else if let Some(tag) = part.strip_prefix("refs/tags/") {
            if !scope.allow_tags {
                return error(format!("Git refspec {raw:?} requires tag approval"));
            }
            validate_git_ref(tag)
                .map_err(|_| CapabilityError::message(format!("Git refspec {raw:?} is invalid")))?;
        } else if part.starts_with("refs/") {
            return error(format!("Git refspec {raw:?} uses an unsupported namespace"));
        } else {
            validate_git_branch(part)
                .map_err(|_| CapabilityError::message(format!("Git refspec {raw:?} is invalid")))?;
            *part = format!("refs/heads/{part}");
        }
    }
    Ok(parts.join(":"))
}

fn normalize_ssh(scope: &mut SshScope) -> Result<String, CapabilityError> {
    if scope.allow_interactive_shell {
        return error(
            "interactive SSH shells are unavailable through the capability broker transport",
        );
    }
    if !scope.alias.is_empty() {
        scope.alias = normalize_token(&scope.alias, "SSH alias", false)?;
    }
    scope.host = normalize_host(&scope.host)?;
    if scope.port == 0 {
        scope.port = 22;
    }
    if scope.user.is_empty() {
        return error("SSH user is required");
    }
    scope.user = normalize_token(&scope.user, "SSH user", true)?;
    validate_host_key(&scope.host_key)?;
    for command in &scope.remote_commands {
        if command.trim() != command
            || command.is_empty()
            || command.contains(['\r', '\n', '\0', '\u{2028}', '\u{2029}'])
        {
            return error("SSH remote commands must be exact non-empty single-line strings");
        }
    }
    scope.remote_commands = unique_sorted(scope.remote_commands.clone());
    if scope.allow_upload
        != (!scope.upload_roots.is_empty() && !scope.upload_remote_roots.is_empty())
        || (!scope.allow_upload
            && (!scope.upload_roots.is_empty() || !scope.upload_remote_roots.is_empty()))
    {
        return error("SSH local and remote upload roots must be configured with upload approval");
    }
    if scope.allow_download
        != (!scope.download_roots.is_empty() && !scope.download_remote_roots.is_empty())
        || (!scope.allow_download
            && (!scope.download_roots.is_empty() || !scope.download_remote_roots.is_empty()))
    {
        return error(
            "SSH local and remote download roots must be configured with download approval",
        );
    }
    scope.upload_roots = normalize_project_roots(&scope.upload_roots, "SSH upload root")?;
    scope.download_roots = normalize_project_roots(&scope.download_roots, "SSH download root")?;
    scope.upload_remote_roots =
        normalize_remote_roots(&scope.upload_remote_roots, "SSH remote upload root")?;
    scope.download_remote_roots =
        normalize_remote_roots(&scope.download_remote_roots, "SSH remote download root")?;
    scope.local_forward_targets =
        normalize_endpoints(&scope.local_forward_targets, "SSH local forwarding target")?;
    scope.remote_forward_targets = normalize_endpoints(
        &scope.remote_forward_targets,
        "SSH remote forwarding target",
    )?;
    let mut grants = Vec::new();
    if scope.allow_git {
        grants.push("git".to_owned());
    }
    if !scope.remote_commands.is_empty() {
        grants.push("commands".to_owned());
    }
    if scope.allow_upload {
        grants.push("upload".to_owned());
    }
    if scope.allow_download {
        grants.push("download".to_owned());
    }
    if !scope.local_forward_targets.is_empty() {
        grants.push("local-forward".to_owned());
    }
    if !scope.remote_forward_targets.is_empty() {
        grants.push("remote-forward".to_owned());
    }
    if grants.is_empty() {
        return error("SSH scope must approve at least one operation");
    }
    Ok(format!(
        "alias={:?} {}@{}:{} host-key={} grants={} commands={} upload_local_roots={} upload_remote_roots={} download_local_roots={} download_remote_roots={} local_forward_targets={} remote_forward_targets={}",
        scope.alias,
        scope.user,
        scope.host,
        scope.port,
        host_key_fingerprint(&scope.host_key),
        scope_list(&grants),
        scope_list(&scope.remote_commands),
        scope_list(&scope.upload_roots),
        scope_list(&scope.upload_remote_roots),
        scope_list(&scope.download_roots),
        scope_list(&scope.download_remote_roots),
        scope_list(&scope.local_forward_targets),
        scope_list(&scope.remote_forward_targets)
    ))
}

fn normalize_project_roots(values: &[String], label: &str) -> Result<Vec<String>, CapabilityError> {
    values
        .iter()
        .map(|value| {
            normalize_project_path(value)
                .map_err(|error| CapabilityError::message(format!("{label}: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(unique_sorted)
}

fn normalize_remote_roots(values: &[String], label: &str) -> Result<Vec<String>, CapabilityError> {
    values
        .iter()
        .map(|value| {
            normalize_remote_path(value)
                .map_err(|error| CapabilityError::message(format!("{label}: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(unique_sorted)
}

fn normalize_endpoints(values: &[String], label: &str) -> Result<Vec<String>, CapabilityError> {
    values
        .iter()
        .map(|value| {
            normalize_endpoint(value)
                .map_err(|error| CapabilityError::message(format!("{label}: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(unique_sorted)
}

/// Normalizes an absolute, shell-safe POSIX path used by SSH transfers.
///
/// # Errors
///
/// Returns an error when the path is relative, spans the remote filesystem
/// root, contains controls or shell-significant characters, or is otherwise
/// not a canonical approved transfer path.
pub fn normalize_remote_path(raw: &str) -> Result<String, CapabilityError> {
    if !raw.starts_with('/') || has_control(raw) || raw.contains('\\') {
        return error("remote path must be an absolute POSIX path");
    }
    if !raw
        .chars()
        .all(|character| is_letter_or_digit(character) || "/._-".contains(character))
    {
        return error("remote path contains shell-significant characters");
    }
    let clean = clean_posix(raw, true);
    if clean == "/" {
        return error("remote root cannot be the entire host filesystem");
    }
    Ok(clean)
}

pub(crate) fn normalize_project_path(raw: &str) -> Result<String, CapabilityError> {
    let path = Path::new(raw);
    if raw.is_empty() || path.is_absolute() || has_control(raw) {
        return error("path must be project-relative");
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return error("path escapes the project");
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return error("path must be project-relative");
            }
        }
    }
    Ok(if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    })
}

pub(crate) fn normalize_endpoint(raw: &str) -> Result<String, CapabilityError> {
    let socket = raw.parse::<SocketAddr>();
    if let Ok(value) = socket {
        if value.port() == 0 {
            return error("endpoint port must be between 1 and 65535");
        }
        return Ok(value.to_string());
    }
    let (host, port) = raw
        .rsplit_once(':')
        .ok_or_else(|| CapabilityError::message("endpoint must be host:port"))?;
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| CapabilityError::message("endpoint port must be between 1 and 65535"))?;
    let host = normalize_host(host.trim_matches(['[', ']']))?;
    Ok(format_authority(&host, Some(port)))
}

fn validate_host_key(raw: &str) -> Result<(), CapabilityError> {
    let fields = raw.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 2 || !(fields[0].starts_with("ssh-") || fields[0].starts_with("ecdsa-")) {
        return error("SSH host key must contain exactly a key type and base64 public key");
    }
    STANDARD
        .decode(fields[1])
        .map_err(|_| CapabilityError::message("SSH host key is not valid base64"))?;
    Ok(())
}

fn host_key_fingerprint(raw: &str) -> String {
    let Some(value) = raw.split_whitespace().nth(1) else {
        return "invalid".to_owned();
    };
    let Ok(decoded) = STANDARD.decode(value) else {
        return "invalid".to_owned();
    };
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(decoded)))
}

pub(crate) fn normalize_host(raw: &str) -> Result<String, CapabilityError> {
    let host = raw
        .trim()
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_lowercase();
    if host.is_empty()
        || host.starts_with('-')
        || has_control(&host)
        || host.contains(['/', '\\', '@'])
    {
        return error(format!("invalid host {raw:?}"));
    }
    if let Ok(ip) = IpAddr::from_str(&host) {
        return Ok(ip.to_string());
    }
    if host
        .chars()
        .all(|character| character.is_ascii_digit() || character == '.')
        || (host.starts_with("0x")
            && host[2..]
                .chars()
                .all(|character| character.is_ascii_hexdigit()))
    {
        return error(format!("invalid host {raw:?}"));
    }
    if host.len() > 253 {
        return error("host name is too long");
    }
    for label in host.split('.') {
        let has_ascii_alphanumeric = label
            .chars()
            .any(|character| character.is_ascii_alphanumeric());
        let has_non_ascii = !label.is_ascii();
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || (has_ascii_alphanumeric && has_non_ascii)
            || !label
                .chars()
                .all(|character| is_letter_or_digit(character) || character == '-')
        {
            return error(format!("invalid host {raw:?}"));
        }
    }
    Ok(host)
}

pub(crate) fn normalize_profile(raw: &str) -> Result<String, CapabilityError> {
    let value = raw.trim();
    if value.is_empty() || value.len() > 64 || value.starts_with('-') || has_control(value) {
        return error("agent profile must be an explicit 1-64 character profile ID");
    }
    if !value
        .chars()
        .all(|character| is_letter_or_digit(character) || "._-".contains(character))
    {
        return error(format!("invalid agent profile {raw:?}"));
    }
    Ok(value.to_owned())
}

pub(crate) fn normalize_token(
    raw: &str,
    label: &str,
    allow_slash: bool,
) -> Result<String, CapabilityError> {
    let value = raw.trim();
    if value.is_empty() || value.len() > 128 || value.starts_with('-') || has_control(value) {
        return error(format!("{label} is invalid"));
    }
    if !value.chars().all(|character| {
        is_letter_or_digit(character)
            || "._-".contains(character)
            || (allow_slash && character == '/')
    }) {
        return error(format!("{label} {raw:?} is invalid"));
    }
    Ok(value.to_owned())
}

fn is_letter_or_digit(character: char) -> bool {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    VALUE
        .get_or_init(|| Regex::new(r"^[\p{L}\p{Nd}]$").expect("static Unicode regex"))
        .is_match(&character.to_string())
}

fn unique_sorted(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn has_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn effective_approval_scope(capability: &Capability, scope: &str) -> String {
    format!(
        "model_version={} kind={} profile={} approval_duration_seconds={}\nlimits timeout_seconds={} max_request_bytes={} max_response_bytes={} max_output_bytes={} max_redirects={} max_concurrent={}\naudit enabled={} retain_last={}\nscope {scope}",
        capability.model_version,
        capability.kind.as_str(),
        capability.agent_profile,
        capability.approval_duration_seconds,
        capability.limits.timeout_seconds,
        capability.limits.max_request_bytes,
        capability.limits.max_response_bytes,
        capability.limits.max_output_bytes,
        capability.limits.max_redirects,
        capability.limits.max_concurrent,
        capability.audit.enabled,
        capability.audit.retain_last,
    )
}

fn scope_list(values: &[String]) -> String {
    go_json(&values).unwrap_or_else(|_| "[]".to_owned())
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct DigestEnvelope {
    ModelVersion: u64,
    Kind: String,
    AgentProfile: String,
    ApprovalDurationSeconds: i64,
    Limits: CapabilityLimitsWire,
    Audit: CapabilityAuditPolicyWire,
    HTTP: Option<HttpScopeWire>,
    Git: Option<GitScopeWire>,
    SSH: Option<SshScopeWire>,
}

fn scope_digest(capability: &Capability) -> Result<Digest32, CapabilityError> {
    let encoded = canonical_scope_json(capability)?;
    Ok(Digest32(Sha256::digest(encoded.as_bytes()).into()))
}

pub(crate) fn canonical_scope_json(capability: &Capability) -> Result<String, CapabilityError> {
    let envelope = DigestEnvelope {
        ModelVersion: capability.model_version,
        Kind: capability.kind.as_str().to_owned(),
        AgentProfile: capability.agent_profile.clone(),
        ApprovalDurationSeconds: capability.approval_duration_seconds,
        Limits: CapabilityLimitsWire::from(&capability.limits),
        Audit: CapabilityAuditPolicyWire::from(&capability.audit),
        HTTP: capability.http.as_ref().map(HttpScopeWire::from),
        Git: capability.git.as_ref().map(GitScopeWire::from),
        SSH: capability.ssh.as_ref().map(SshScopeWire::from),
    };
    go_json(&envelope)
}

pub(crate) fn go_json(value: &impl Serialize) -> Result<String, CapabilityError> {
    let encoded = serde_json::to_string(value)
        .map_err(|_| CapabilityError::message("capability scope cannot be encoded"))?;
    Ok(encoded
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029"))
}

fn clean_posix(raw: &str, absolute: bool) -> String {
    let mut parts = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

fn format_authority(host: &str, port: Option<u16>) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    port.map_or(host.clone(), |value| format!("{host}:{value}"))
}

fn format_authority_text(host: &str, port: Option<&str>) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    port.map_or(host.clone(), |value| format!("{host}:{value}"))
}

fn error<T>(message: impl Into<String>) -> Result<T, CapabilityError> {
    Err(CapabilityError::message(message))
}
