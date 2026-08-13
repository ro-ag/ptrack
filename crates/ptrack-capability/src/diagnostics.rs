use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use ptrack_capability_policy::{approve, normalize};
use ptrack_core::{Capability, CapabilityKind};
use ptrack_store::{Clock, SystemClock};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::git::{ProcessRunner, system_runner};
use crate::process::ProcessSpec;
use crate::ssh::{PinnedKnownHosts, ssh_base_args};
use crate::{GitExecutor, GitRequest, HttpExecutor, HttpRequest, classify_ssh_exit};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VpnState {
    Active,
    Inactive,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConnectionDiagnostic {
    pub kind: String,
    pub success: bool,
    pub stage: String,
    pub class: String,
    pub message: String,
    pub vpn: VpnState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proxy: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ca_store: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub status_code: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VpnUnavailableError;

impl fmt::Display for VpnUnavailableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("required VPN route is unavailable")
    }
}

impl std::error::Error for VpnUnavailableError {}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConnectionTester;

impl ConnectionTester {
    /// Runs a read-only HTTP base-origin probe against an in-memory approval.
    pub async fn test_http(
        &self,
        cancellation: &CancellationToken,
        draft: &Capability,
    ) -> ConnectionDiagnostic {
        let vpn = detect_vpn_state();
        let Ok(preview) = normalize(draft) else {
            return diagnostic(CapabilityKind::Http, "denied", 0, vpn);
        };
        if preview.capability.kind != CapabilityKind::Http {
            return diagnostic(CapabilityKind::Http, "denied", 0, vpn);
        }
        let Ok(approved) = approve(
            &preview.capability,
            preview.scope_digest,
            SystemClock.now_utc(),
        ) else {
            return diagnostic(CapabilityKind::Http, "denied", 0, vpn);
        };
        let Some(scope) = approved.http.as_ref() else {
            return diagnostic(CapabilityKind::Http, "denied", 0, vpn);
        };
        let Some(method) = ["HEAD", "GET", "OPTIONS"]
            .into_iter()
            .find(|method| scope.methods.iter().any(|approved| approved == method))
        else {
            return diagnostic(CapabilityKind::Http, "denied", 0, vpn);
        };
        let result = HttpExecutor::new(None)
            .execute(
                cancellation,
                &approved,
                &approved.agent_profile,
                &HttpRequest {
                    method: method.to_owned(),
                    url: scope.base_url.clone(),
                    headers: BTreeMap::default(),
                    body: Vec::new(),
                },
            )
            .await;
        match result {
            Ok(response) => {
                let class = if response.status_code == 407 {
                    "proxy"
                } else if response.status_code >= 400 {
                    "remote-policy"
                } else {
                    "none"
                };
                let mut result = diagnostic(CapabilityKind::Http, class, response.status_code, vpn);
                result.proxy = response.diagnostics.proxy;
                result.ca_store = response.diagnostics.ca_store;
                result
            }
            Err(error) => {
                let mut result = diagnostic(
                    CapabilityKind::Http,
                    error.class().as_str(),
                    error.status_code(),
                    vpn,
                );
                result.proxy.clone_from(&error.diagnostics().proxy);
                result.ca_store.clone_from(&error.diagnostics().ca_store);
                result
            }
        }
    }

    /// Runs only `ls-remote` through the fixed Git executor.
    pub async fn test_git(
        &self,
        cancellation: &CancellationToken,
        draft: &Capability,
        ssh_draft: Option<&Capability>,
        project_root: &Path,
    ) -> ConnectionDiagnostic {
        self.test_git_with_runner(
            cancellation,
            draft,
            ssh_draft,
            project_root,
            system_runner(),
        )
        .await
    }

    pub(crate) async fn test_git_with_runner(
        &self,
        cancellation: &CancellationToken,
        draft: &Capability,
        ssh_draft: Option<&Capability>,
        project_root: &Path,
        runner: &dyn ProcessRunner,
    ) -> ConnectionDiagnostic {
        let vpn = detect_vpn_state();
        let mut preview = match normalize(draft) {
            Ok(preview) if preview.capability.kind == CapabilityKind::Git => preview,
            _ => return diagnostic(CapabilityKind::Git, "denied", 0, vpn),
        };
        let Some(scope) = preview.capability.git.as_mut() else {
            return diagnostic(CapabilityKind::Git, "denied", 0, vpn);
        };
        if !scope
            .operations
            .iter()
            .any(|operation| operation == "ls-remote")
        {
            scope.operations.push("ls-remote".to_owned());
            preview = match normalize(&preview.capability) {
                Ok(preview) => preview,
                Err(_) => return diagnostic(CapabilityKind::Git, "denied", 0, vpn),
            };
        }
        let Ok(approved) = approve(
            &preview.capability,
            preview.scope_digest,
            SystemClock.now_utc(),
        ) else {
            return diagnostic(CapabilityKind::Git, "denied", 0, vpn);
        };
        let approved_ssh = ssh_draft.and_then(|draft| {
            let preview = normalize(draft).ok()?;
            approve(
                &preview.capability,
                preview.scope_digest,
                SystemClock.now_utc(),
            )
            .ok()
        });
        let result = GitExecutor::from_parts(crate::AuditRecorder::new(None), runner)
            .execute(
                cancellation,
                &approved,
                approved_ssh.as_ref(),
                &approved.agent_profile,
                project_root,
                &GitRequest {
                    operation: "ls-remote".to_owned(),
                    branch: String::new(),
                    refspec: String::new(),
                    force: false,
                },
            )
            .await;
        match result {
            Ok(_) => diagnostic(CapabilityKind::Git, "none", 0, vpn),
            Err(error) => diagnostic(CapabilityKind::Git, error.class(), 0, vpn),
        }
    }

    /// Authenticates with the pinned key and runs only the fixed command `true`.
    pub async fn test_ssh(
        &self,
        cancellation: &CancellationToken,
        draft: &Capability,
    ) -> ConnectionDiagnostic {
        self.test_ssh_with_runner(cancellation, draft, system_runner())
            .await
    }

    pub(crate) async fn test_ssh_with_runner(
        &self,
        cancellation: &CancellationToken,
        draft: &Capability,
        runner: &dyn ProcessRunner,
    ) -> ConnectionDiagnostic {
        let vpn = detect_vpn_state();
        let preview = match normalize(draft) {
            Ok(preview) if preview.capability.kind == CapabilityKind::Ssh => preview,
            _ => return diagnostic(CapabilityKind::Ssh, "denied", 0, vpn),
        };
        let Ok(approved) = approve(
            &preview.capability,
            preview.scope_digest,
            SystemClock.now_utc(),
        ) else {
            return diagnostic(CapabilityKind::Ssh, "denied", 0, vpn);
        };
        let Some(scope) = approved.ssh.as_ref() else {
            return diagnostic(CapabilityKind::Ssh, "denied", 0, vpn);
        };
        let Ok(pinned) = PinnedKnownHosts::new(scope) else {
            return diagnostic(CapabilityKind::Ssh, "internal", 0, vpn);
        };
        let mut args = ssh_base_args(scope, pinned.file(), false);
        args.extend([
            OsString::from("-T"),
            OsString::from(format!("{}@{}", scope.user, scope.host)),
            OsString::from("true"),
        ]);
        let result = runner
            .run(
                &ProcessSpec {
                    name: OsString::from("ssh"),
                    args,
                    env: vec![
                        (OsString::from("LC_ALL"), OsString::from("C")),
                        (OsString::from("LANG"), OsString::from("C")),
                    ],
                    max_output_bytes: u64::try_from(approved.limits.max_output_bytes)
                        .unwrap_or_default(),
                    timeout: Duration::from_secs(
                        u64::try_from(approved.limits.timeout_seconds).unwrap_or_default(),
                    ),
                },
                cancellation,
            )
            .await;
        match result {
            Ok(result) if result.truncated => {
                diagnostic(CapabilityKind::Ssh, "output-limit", 0, vpn)
            }
            Ok(result) if result.exit_code == 0 => diagnostic(CapabilityKind::Ssh, "none", 0, vpn),
            Ok(result) => diagnostic(
                CapabilityKind::Ssh,
                classify_ssh_exit(&String::from_utf8_lossy(&result.stderr)),
                0,
                vpn,
            ),
            Err(error) => diagnostic(
                CapabilityKind::Ssh,
                match error {
                    crate::process::ProcessError::Cancelled => "cancelled",
                    crate::process::ProcessError::Timeout => "timeout",
                    crate::process::ProcessError::Spawn | crate::process::ProcessError::Wait => {
                        "transport"
                    }
                },
                0,
                vpn,
            ),
        }
    }
}

#[must_use]
pub fn detect_vpn_state() -> VpnState {
    detect_vpn_state_platform()
}

#[cfg(target_os = "linux")]
fn detect_vpn_state_platform() -> VpnState {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return VpnState::Unknown;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if is_tunnel_name(&name)
            && std::fs::read_to_string(entry.path().join("operstate"))
                .is_ok_and(|value| value.trim() == "up")
        {
            return VpnState::Active;
        }
    }
    VpnState::Inactive
}

#[cfg(all(unix, not(target_os = "linux")))]
fn detect_vpn_state_platform() -> VpnState {
    let output = std::process::Command::new("/sbin/ifconfig")
        .arg("-l")
        .output();
    let Ok(output) = output else {
        return VpnState::Unknown;
    };
    if !output.status.success() {
        return VpnState::Unknown;
    }
    let names = String::from_utf8_lossy(&output.stdout);
    for name in names.split_whitespace().filter(|name| is_tunnel_name(name)) {
        let active = std::process::Command::new("/sbin/ifconfig")
            .arg(name)
            .output()
            .is_ok_and(|output| {
                output.status.success() && String::from_utf8_lossy(&output.stdout).contains("<UP,")
            });
        if active {
            return VpnState::Active;
        }
    }
    VpnState::Inactive
}

#[cfg(windows)]
fn detect_vpn_state_platform() -> VpnState {
    match crate::private_windows::active_interface_names() {
        Ok(names) if names.iter().any(|name| is_tunnel_name(name)) => VpnState::Active,
        Ok(_) => VpnState::Inactive,
        Err(()) => VpnState::Unknown,
    }
}

#[cfg(not(any(unix, windows)))]
fn detect_vpn_state_platform() -> VpnState {
    VpnState::Unknown
}

pub(crate) fn is_tunnel_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["utun", "tun", "tap", "wg", "ppp"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

pub(crate) fn diagnostic(
    kind: CapabilityKind,
    class: &str,
    status_code: u16,
    vpn: VpnState,
) -> ConnectionDiagnostic {
    let class = if known_class(class) {
        class
    } else {
        "transport"
    };
    ConnectionDiagnostic {
        kind: kind.as_str().to_owned(),
        success: class == "none",
        stage: stage(class).to_owned(),
        class: class.to_owned(),
        message: message(class).to_owned(),
        vpn,
        proxy: String::new(),
        ca_store: String::new(),
        status_code,
    }
}

fn known_class(class: &str) -> bool {
    matches!(
        class,
        "none"
            | "denied"
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
            | "request-limit"
            | "response-limit"
            | "output-limit"
            | "cancelled"
            | "transport"
            | "internal"
    )
}

fn stage(class: &str) -> &'static str {
    match class {
        "none" => "complete",
        "denied" => "policy",
        "dns" => "dns",
        "routing" => "routing",
        "vpn" => "vpn",
        "proxy" => "proxy",
        "tls" => "tls",
        "host-key" => "host-key",
        "authentication" => "authentication",
        "sandbox" => "sandbox",
        "remote-policy" => "remote-policy",
        "timeout" => "connect",
        "request-limit" => "request",
        "response-limit" | "output-limit" => "response",
        "cancelled" => "cancelled",
        "internal" => "internal",
        _ => "transport",
    }
}

fn message(class: &str) -> &'static str {
    match class {
        "none" => "Connection test succeeded.",
        "denied" => "The capability policy rejected the test.",
        "dns" => "The host name could not be resolved.",
        "routing" => "No usable route to the host was available.",
        "vpn" => "A required VPN route or policy was unavailable.",
        "proxy" => "The current proxy rejected or could not authenticate the request.",
        "tls" => "TLS certificate or handshake validation failed with the system CA store.",
        "host-key" => "The SSH host key did not match the pinned key.",
        "authentication" => {
            "Host authentication failed using current credential helpers or ssh-agent."
        }
        "sandbox" => "The host sandbox or local permissions blocked the operation.",
        "remote-policy" => "The remote service was reached but rejected the operation.",
        "timeout" => "The connection test timed out.",
        "output-limit" => "The connection test exceeded its output limit.",
        "cancelled" => "The connection test was cancelled.",
        "internal" => "The connection test failed internally.",
        _ => "The connection failed for an unclassified transport reason.",
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(value: &u16) -> bool {
    *value == 0
}
