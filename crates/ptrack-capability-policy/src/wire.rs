use std::fmt;
use std::str::FromStr;

use ptrack_core::{
    Capability, CapabilityAudit, CapabilityAuditPolicy, CapabilityKind, CapabilityLimits, Digest32,
    GitScope, HttpScope, SshScope, Timestamp,
};
use serde::{Deserialize, Deserializer, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireError(String);

impl WireError {
    fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WireError {}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CapabilityLimitsWire {
    pub timeout_seconds: i64,
    pub max_request_bytes: i64,
    pub max_response_bytes: i64,
    pub max_output_bytes: i64,
    pub max_redirects: i64,
    pub max_concurrent: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CapabilityAuditPolicyWire {
    pub enabled: bool,
    pub retain_last: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpScopeWire {
    pub base_url: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub methods: Vec<String>,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub path_prefixes: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitScopeWire {
    pub remote_name: String,
    pub remote_url: String,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub operations: Vec<String>,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub branches: Vec<String>,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub refspecs: Vec<String>,
    pub allow_tags: bool,
    pub allow_force_push: bool,
    pub allow_delete_refs: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct SshScopeWire {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub host_key: String,
    pub allow_git: bool,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub remote_commands: Vec<String>,
    pub allow_upload: bool,
    pub allow_download: bool,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub upload_roots: Vec<String>,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub download_roots: Vec<String>,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub upload_remote_roots: Vec<String>,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub download_remote_roots: Vec<String>,
    pub allow_interactive_shell: bool,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub local_forward_targets: Vec<String>,
    #[serde(deserialize_with = "deserialize_null_default")]
    pub remote_forward_targets: Vec<String>,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Exact capability JSON shape shared by Settings, CLI, and broker adapters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityWire {
    pub id: u64,
    pub model_version: u64,
    pub revision: u64,
    pub name: String,
    pub kind: String,
    pub agent_profile: String,
    pub enabled: bool,
    pub approval_duration_seconds: i64,
    pub approved_at: String,
    pub expires_at: String,
    pub scope_digest: String,
    pub limits: CapabilityLimitsWire,
    pub audit: CapabilityAuditPolicyWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpScopeWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<GitScopeWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshScopeWire>,
    pub created_at: String,
    pub updated_at: String,
}

/// Operator-authored draft shape. Identity, lifecycle, limits, and audit
/// fields may be omitted and receive Go-compatible zero values before policy
/// normalization supplies safe defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CapabilityDraftWire {
    pub id: u64,
    pub model_version: u64,
    pub revision: u64,
    pub name: String,
    pub kind: String,
    pub agent_profile: String,
    pub approval_duration_seconds: i64,
    pub limits: Option<CapabilityLimitsWire>,
    pub audit: Option<CapabilityAuditPolicyWire>,
    pub http: Option<HttpScopeWire>,
    pub git: Option<GitScopeWire>,
    pub ssh: Option<SshScopeWire>,
}

/// Exact metadata-only audit JSON shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityAuditWire {
    pub id: u64,
    pub capability_id: u64,
    pub agent_profile: String,
    pub kind: String,
    pub operation: String,
    pub target: String,
    pub success: bool,
    pub error_class: String,
    pub duration_millis: i64,
    pub request_bytes: i64,
    pub response_bytes: i64,
    pub redirects: i64,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewWire {
    pub capability: CapabilityWire,
    pub effective_scope: String,
    pub scope_digest: String,
}

impl TryFrom<&Capability> for CapabilityWire {
    type Error = WireError;

    fn try_from(value: &Capability) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            model_version: value.model_version,
            revision: value.revision,
            name: value.name.clone(),
            kind: value.kind.as_str().to_owned(),
            agent_profile: value.agent_profile.clone(),
            enabled: value.enabled,
            approval_duration_seconds: value.approval_duration_seconds,
            approved_at: format_timestamp(value.approved_at)?,
            expires_at: format_timestamp(value.expires_at)?,
            scope_digest: encode_digest(value.scope_digest),
            limits: CapabilityLimitsWire::from(&value.limits),
            audit: CapabilityAuditPolicyWire::from(&value.audit),
            http: value.http.as_ref().map(HttpScopeWire::from),
            git: value.git.as_ref().map(GitScopeWire::from),
            ssh: value.ssh.as_ref().map(SshScopeWire::from),
            created_at: format_timestamp(value.created_at)?,
            updated_at: format_timestamp(value.updated_at)?,
        })
    }
}

impl TryFrom<CapabilityDraftWire> for Capability {
    type Error = WireError;

    fn try_from(value: CapabilityDraftWire) -> Result<Self, Self::Error> {
        let kind = CapabilityKind::from_str(&value.kind).map_err(|_| {
            WireError::message(format!("unsupported capability kind {:?}", value.kind))
        })?;
        Ok(Self {
            id: value.id,
            model_version: value.model_version,
            revision: value.revision,
            name: value.name,
            kind,
            agent_profile: value.agent_profile,
            enabled: false,
            approval_duration_seconds: value.approval_duration_seconds,
            approved_at: Timestamp::Zero,
            expires_at: Timestamp::Zero,
            scope_digest: Digest32::EMPTY,
            limits: value.limits.map_or(
                CapabilityLimits {
                    timeout_seconds: 0,
                    max_request_bytes: 0,
                    max_response_bytes: 0,
                    max_output_bytes: 0,
                    max_redirects: 0,
                    max_concurrent: 0,
                },
                Into::into,
            ),
            audit: value.audit.map_or(
                CapabilityAuditPolicy {
                    enabled: false,
                    retain_last: 0,
                },
                Into::into,
            ),
            http: value.http.map(Into::into),
            git: value.git.map(Into::into),
            ssh: value.ssh.map(Into::into),
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
        })
    }
}

impl TryFrom<CapabilityWire> for Capability {
    type Error = WireError;

    fn try_from(value: CapabilityWire) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            model_version: value.model_version,
            revision: value.revision,
            name: value.name,
            kind: CapabilityKind::from_str(&value.kind)
                .map_err(|error| WireError::message(error.to_string()))?,
            agent_profile: value.agent_profile,
            enabled: value.enabled,
            approval_duration_seconds: value.approval_duration_seconds,
            approved_at: parse_timestamp(&value.approved_at)?,
            expires_at: parse_timestamp(&value.expires_at)?,
            scope_digest: decode_digest(&value.scope_digest)?,
            limits: value.limits.into(),
            audit: value.audit.into(),
            http: value.http.map(Into::into),
            git: value.git.map(Into::into),
            ssh: value.ssh.map(Into::into),
            created_at: parse_timestamp(&value.created_at)?,
            updated_at: parse_timestamp(&value.updated_at)?,
        })
    }
}

impl TryFrom<&CapabilityAudit> for CapabilityAuditWire {
    type Error = WireError;

    fn try_from(value: &CapabilityAudit) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            capability_id: value.capability_id,
            agent_profile: value.agent_profile.clone(),
            kind: value.kind.as_str().to_owned(),
            operation: value.operation.clone(),
            target: value.target.clone(),
            success: value.success,
            error_class: value.error_class.clone(),
            duration_millis: value.duration_millis,
            request_bytes: value.request_bytes,
            response_bytes: value.response_bytes,
            redirects: value.redirects,
            created_at: format_timestamp(value.created_at)?,
        })
    }
}

impl TryFrom<CapabilityAuditWire> for CapabilityAudit {
    type Error = WireError;

    fn try_from(value: CapabilityAuditWire) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            capability_id: value.capability_id,
            agent_profile: value.agent_profile,
            kind: CapabilityKind::from_str(&value.kind)
                .map_err(|error| WireError::message(error.to_string()))?,
            operation: value.operation,
            target: value.target,
            success: value.success,
            error_class: value.error_class,
            duration_millis: value.duration_millis,
            request_bytes: value.request_bytes,
            response_bytes: value.response_bytes,
            redirects: value.redirects,
            created_at: parse_timestamp(&value.created_at)?,
        })
    }
}

impl From<&CapabilityLimits> for CapabilityLimitsWire {
    fn from(value: &CapabilityLimits) -> Self {
        Self {
            timeout_seconds: value.timeout_seconds,
            max_request_bytes: value.max_request_bytes,
            max_response_bytes: value.max_response_bytes,
            max_output_bytes: value.max_output_bytes,
            max_redirects: value.max_redirects,
            max_concurrent: value.max_concurrent,
        }
    }
}

impl From<CapabilityLimitsWire> for CapabilityLimits {
    fn from(value: CapabilityLimitsWire) -> Self {
        Self {
            timeout_seconds: value.timeout_seconds,
            max_request_bytes: value.max_request_bytes,
            max_response_bytes: value.max_response_bytes,
            max_output_bytes: value.max_output_bytes,
            max_redirects: value.max_redirects,
            max_concurrent: value.max_concurrent,
        }
    }
}

impl From<&CapabilityAuditPolicy> for CapabilityAuditPolicyWire {
    fn from(value: &CapabilityAuditPolicy) -> Self {
        Self {
            enabled: value.enabled,
            retain_last: value.retain_last,
        }
    }
}

impl From<CapabilityAuditPolicyWire> for CapabilityAuditPolicy {
    fn from(value: CapabilityAuditPolicyWire) -> Self {
        Self {
            enabled: value.enabled,
            retain_last: value.retain_last,
        }
    }
}

macro_rules! scope_conversions {
    ($wire:ty, $native:ty, { $($field:ident),+ $(,)? }) => {
        impl From<&$native> for $wire {
            fn from(value: &$native) -> Self {
                Self { $($field: value.$field.clone()),+ }
            }
        }
        impl From<$wire> for $native {
            fn from(value: $wire) -> Self {
                Self { $($field: value.$field),+ }
            }
        }
    };
}

scope_conversions!(HttpScopeWire, HttpScope, { base_url, methods, path_prefixes });
scope_conversions!(GitScopeWire, GitScope, {
    remote_name,
    remote_url,
    operations,
    branches,
    refspecs,
    allow_tags,
    allow_force_push,
    allow_delete_refs,
});
scope_conversions!(SshScopeWire, SshScope, {
    alias,
    host,
    port,
    user,
    host_key,
    allow_git,
    remote_commands,
    allow_upload,
    allow_download,
    upload_roots,
    download_roots,
    upload_remote_roots,
    download_remote_roots,
    allow_interactive_shell,
    local_forward_targets,
    remote_forward_targets,
});

pub(crate) fn encode_digest(value: Digest32) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if value.is_empty() {
        return String::new();
    }
    let mut result = String::with_capacity(64);
    for byte in value.0 {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    result
}

pub(crate) fn decode_digest(value: &str) -> Result<Digest32, WireError> {
    if value.is_empty() {
        return Ok(Digest32::EMPTY);
    }
    if value.len() != 64 {
        return Err(WireError::message(
            "scope digest must contain 64 lowercase hex characters",
        ));
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        digest[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(Digest32(digest))
}

fn hex_nibble(value: u8) -> Result<u8, WireError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(WireError::message(
            "scope digest must contain 64 lowercase hex characters",
        )),
    }
}

fn format_timestamp(value: Timestamp) -> Result<String, WireError> {
    let Timestamp::Fixed {
        seconds,
        nanoseconds,
        offset_seconds,
    } = value
    else {
        return Ok("0001-01-01T00:00:00Z".to_owned());
    };
    let nanos = i128::from(seconds) * 1_000_000_000 + i128::from(nanoseconds);
    let instant = OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .map_err(|_| WireError::message("timestamp is outside RFC3339 range"))?;
    let offset = UtcOffset::from_whole_seconds(offset_seconds)
        .map_err(|_| WireError::message("timestamp offset is invalid"))?;
    instant
        .to_offset(offset)
        .format(&Rfc3339)
        .map_err(|_| WireError::message("timestamp cannot be formatted as RFC3339Nano"))
}

fn parse_timestamp(value: &str) -> Result<Timestamp, WireError> {
    if value == "0001-01-01T00:00:00Z" {
        return Ok(Timestamp::Zero);
    }
    let instant = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| WireError::message("timestamp must use RFC3339Nano"))?;
    Ok(Timestamp::Fixed {
        seconds: instant.unix_timestamp(),
        nanoseconds: instant.nanosecond(),
        offset_seconds: instant.offset().whole_seconds(),
    })
}
