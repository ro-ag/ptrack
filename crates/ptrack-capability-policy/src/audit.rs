use ptrack_core::{Capability, CapabilityAudit, CapabilityKind, Timestamp};

use crate::normalize::{
    normalize_endpoint, normalize_http_url, normalize_profile, normalize_token,
};

const MAX_DURATION_MILLIS: i64 = 24 * 60 * 60 * 1_000;
const MAX_BYTES: i64 = 1 << 40;
const MAX_REDIRECTS: i64 = 10;

/// Transient operation metadata. It deliberately cannot contain headers,
/// bodies, process output, credentials, raw stderr, or argv.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditEvent {
    pub operation: String,
    pub target: String,
    pub success: bool,
    pub error_class: String,
    pub duration_millis: i64,
    pub request_bytes: i64,
    pub response_bytes: i64,
    pub redirects: i64,
}

/// Opaque sanitized metadata accepted by the project store.
#[derive(Clone, Debug)]
pub struct SanitizedAudit {
    record: CapabilityAudit,
    retain_last: i64,
}

/// Reduces a transient event to fixed, bounded metadata.
///
/// Returns `None` when auditing is disabled for the capability.
#[must_use]
pub fn sanitize_audit(capability: &Capability, event: &AuditEvent) -> Option<SanitizedAudit> {
    if !capability.audit.enabled {
        return None;
    }
    Some(SanitizedAudit {
        record: CapabilityAudit {
            id: 0,
            capability_id: capability.id,
            agent_profile: normalize_profile(&capability.agent_profile)
                .unwrap_or_else(|_| "unknown-profile".to_owned()),
            kind: capability.kind,
            operation: sanitize_operation(capability.kind, &event.operation),
            target: sanitize_target(capability.kind, &event.target),
            success: event.success,
            error_class: sanitize_error_class(event.success, &event.error_class),
            duration_millis: event.duration_millis.clamp(0, MAX_DURATION_MILLIS),
            request_bytes: event.request_bytes.clamp(0, MAX_BYTES),
            response_bytes: event.response_bytes.clamp(0, MAX_BYTES),
            redirects: event.redirects.clamp(0, MAX_REDIRECTS),
            created_at: Timestamp::Zero,
        },
        retain_last: capability.audit.retain_last,
    })
}

impl SanitizedAudit {
    /// Supplies the storage-owned timestamp and consumes the opaque record.
    #[doc(hidden)]
    #[must_use]
    pub fn into_store_parts(mut self, now: Timestamp) -> (CapabilityAudit, i64) {
        self.record.created_at = now;
        (self.record, self.retain_last)
    }
}

fn sanitize_operation(kind: CapabilityKind, raw: &str) -> String {
    let value = raw.trim().to_lowercase();
    let allowed = match kind {
        CapabilityKind::Http => matches!(
            value.as_str(),
            "get" | "head" | "post" | "put" | "patch" | "delete" | "options" | "test"
        ),
        CapabilityKind::Git => matches!(
            value.as_str(),
            "status" | "fetch" | "pull" | "push" | "ls-remote" | "test"
        ),
        CapabilityKind::Ssh => matches!(
            value.as_str(),
            "git"
                | "remote-command"
                | "upload"
                | "download"
                | "interactive-shell"
                | "local-forward"
                | "remote-forward"
                | "test"
        ),
    };
    if allowed { value } else { "unknown".to_owned() }
}

fn sanitize_target(kind: CapabilityKind, raw: &str) -> String {
    match kind {
        CapabilityKind::Http => normalize_http_url(raw, true).map_or_else(
            |_| "invalid-http-target".to_owned(),
            |url| truncate(format!("{}://{}", url.scheme, url.host), 256),
        ),
        CapabilityKind::Git => normalize_token(raw, "Git remote", true).map_or_else(
            |_| "invalid-git-target".to_owned(),
            |name| truncate(format!("remote:{name}"), 160),
        ),
        CapabilityKind::Ssh => normalize_endpoint(raw).map_or_else(
            |_| "invalid-ssh-target".to_owned(),
            |value| truncate(value, 256),
        ),
    }
}

fn sanitize_error_class(success: bool, raw: &str) -> String {
    if success {
        return "none".to_owned();
    }
    let value = raw.trim().to_lowercase();
    if matches!(
        value.as_str(),
        "denied"
            | "dns"
            | "routing"
            | "vpn"
            | "proxy"
            | "tls"
            | "host-key"
            | "authentication"
            | "sandbox"
            | "remote-policy"
            | "timeout"
            | "transport"
            | "request-limit"
            | "response-limit"
            | "output-limit"
            | "cancelled"
            | "internal"
    ) {
        value
    } else {
        "internal".to_owned()
    }
}

fn truncate(mut value: String, maximum: usize) -> String {
    if value.len() <= maximum {
        return value;
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}
