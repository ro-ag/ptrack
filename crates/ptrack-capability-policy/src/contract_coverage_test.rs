use ptrack_core::{Capability, CapabilityKind, Digest32, GitScope, HttpScope, SshScope, Timestamp};

use super::normalize::{
    normalize_git_remote, normalize_http_url, normalize_project_path, normalize_remote_path,
};
use super::normalize_test::draft;
use super::wire::CapabilityDraftWire;
use super::*;

type ContractCheck = fn();

#[test]
fn cap_001_through_036_execute_in_contract_order() {
    let checks: [ContractCheck; 36] = [
        cap_001, cap_002, cap_003, cap_004, cap_005, cap_006, cap_007, cap_008, cap_009, cap_010,
        cap_011, cap_012, cap_013, cap_014, cap_015, cap_016, cap_017, cap_018, cap_019, cap_020,
        cap_021, cap_022, cap_023, cap_024, cap_025, cap_026, cap_027, cap_028, cap_029, cap_030,
        cap_031, cap_032, cap_033, cap_034, cap_035, cap_036,
    ];
    for check in checks {
        check();
    }
}

fn cap_001() {
    let wire: CapabilityDraftWire = serde_json::from_str(r#"{"kind":"smtp"}"#).unwrap();
    assert!(
        Capability::try_from(wire)
            .unwrap_err()
            .to_string()
            .contains("unsupported capability kind")
    );
}

fn cap_002() {
    let mut value = http();
    assert_eq!(normalize(&value).unwrap().capability.model_version, 1);
    value.model_version = 2;
    assert!(normalize(&value).is_err());
}

fn cap_003() {
    let mut value = http();
    value.git = Some(git_scope());
    assert_eq!(
        normalize(&value).unwrap_err().to_string(),
        "HTTP capability must contain only an HTTP scope"
    );
}

fn cap_004() {
    let mut value = http();
    value.name = " \t".to_owned();
    assert!(normalize(&value).is_err());
}

fn cap_005() {
    let mut value = http();
    value.agent_profile = "agent-١".to_owned();
    assert!(normalize(&value).is_ok());
    value.agent_profile = "-agent".to_owned();
    assert!(normalize(&value).is_err());
}

fn cap_006() {
    let mut value = http();
    let normalized = normalize(&value).unwrap().capability;
    assert_eq!(normalized.approval_duration_seconds, 3_600);
    assert_eq!(normalized.limits.timeout_seconds, 30);
    assert_eq!(normalized.limits.max_redirects, 0);
    value.limits.max_concurrent = 9;
    assert!(normalize(&value).is_err());
}

fn cap_007() {
    let mut value = http();
    value.http.as_mut().unwrap().methods =
        vec!["HEAD".to_owned(), "GET".to_owned(), "GET".to_owned()];
    assert_eq!(
        normalize(&value).unwrap().capability.http.unwrap().methods,
        ["GET", "HEAD"]
    );
}

fn cap_008() {
    assert!(normalize_http_url("https://example.com/api", false).is_ok());
    assert!(normalize_http_url("example.com/api", false).is_err());
    assert!(normalize_http_url("https://user@example.com/api", false).is_err());
}

fn cap_009() {
    assert!(normalize_http_url("https://example.com/%2e%2e/private", false).is_err());
    assert!(normalize_http_url("https://example.com/%00", false).is_err());
}

fn cap_010() {
    let mut value = http();
    value.http.as_mut().unwrap().methods = vec!["TRACE".to_owned()];
    assert!(normalize(&value).is_err());
}

fn cap_011() {
    let mut value = http();
    value.http.as_mut().unwrap().base_url = "https://example.com/api".to_owned();
    value.http.as_mut().unwrap().path_prefixes = vec!["/apix".to_owned()];
    assert!(normalize(&value).is_err());
    value.http.as_mut().unwrap().path_prefixes.clear();
    assert_eq!(
        normalize(&value)
            .unwrap()
            .capability
            .http
            .unwrap()
            .path_prefixes,
        ["/api"]
    );
}

fn cap_012() {
    assert!(
        normalize(&http())
            .unwrap()
            .effective_scope
            .contains("scope GET https://example.com/api paths=/api")
    );
}

fn cap_013() {
    assert_eq!(
        normalize_git_remote("HTTPS://Example.COM:443/repo.git").unwrap(),
        "https://example.com/repo.git"
    );
    assert!(normalize_git_remote("https://token@example.com/repo.git").is_err());
}

fn cap_014() {
    assert!(normalize_git_remote("https://example.com/a/%2e%2e/repo.git").is_err());
    assert!(normalize_git_remote("ssh://git@example.com/a%5Crepo.git").is_err());
}

fn cap_015() {
    let mut value = git();
    value.git.as_mut().unwrap().operations = vec!["clone".to_owned()];
    assert!(normalize(&value).is_err());
}

fn cap_016() {
    let mut value = git();
    value.git.as_mut().unwrap().branches = vec!["HEAD".to_owned()];
    assert!(normalize(&value).is_err());
}

fn cap_017() {
    let mut value = git();
    value.git.as_mut().unwrap().refspecs = vec!["+main:main".to_owned()];
    assert!(normalize(&value).is_err());
    value.git.as_mut().unwrap().refspecs = vec!["main:main".to_owned()];
    assert_eq!(
        normalize(&value).unwrap().capability.git.unwrap().refspecs,
        ["refs/heads/main:refs/heads/main"]
    );
}

fn cap_018() {
    let preview = normalize(&git()).unwrap();
    for field in [
        "remote origin=",
        "operations=[",
        "branches=[",
        "refspecs=[",
        "allow_tags=false",
        "allow_force_with_lease=false",
        "allow_delete_refs=false",
    ] {
        assert!(preview.effective_scope.contains(field));
    }
}

fn cap_019() {
    let mut value = ssh();
    value.ssh.as_mut().unwrap().allow_interactive_shell = true;
    assert!(
        normalize(&value)
            .unwrap_err()
            .to_string()
            .contains("interactive SSH shells")
    );
}

fn cap_020() {
    let normalized = normalize(&ssh()).unwrap().capability.ssh.unwrap();
    assert_eq!(normalized.host, "example.com");
    assert_eq!(normalized.port, 22);
    let mut value = ssh();
    value.ssh.as_mut().unwrap().alias = "prod/path".to_owned();
    assert!(normalize(&value).is_err());
}

fn cap_021() {
    let mut value = ssh();
    value.ssh.as_mut().unwrap().remote_commands = vec!["uptime\nwhoami".to_owned()];
    assert!(normalize(&value).is_err());
}

fn cap_022() {
    let mut value = ssh();
    let scope = value.ssh.as_mut().unwrap();
    scope.allow_upload = true;
    scope.upload_roots = vec!["dist".to_owned()];
    assert!(
        normalize(&value)
            .unwrap_err()
            .to_string()
            .contains("upload roots")
    );
}

fn cap_023() {
    assert_eq!(
        normalize_project_path("dist/../artifacts").unwrap(),
        "artifacts"
    );
    assert!(normalize_project_path("../secret").is_err());
    assert!(normalize_remote_path("/").is_err());
}

fn cap_024() {
    let mut value = ssh();
    value.ssh.as_mut().unwrap().local_forward_targets = vec!["DB.INTERNAL:5432".to_owned()];
    assert_eq!(
        normalize(&value)
            .unwrap()
            .capability
            .ssh
            .unwrap()
            .local_forward_targets,
        ["db.internal:5432"]
    );
}

fn cap_025() {
    let mut value = ssh();
    value.ssh.as_mut().unwrap().remote_commands.clear();
    assert!(normalize(&value).is_err());
    assert!(
        normalize(&ssh())
            .unwrap()
            .effective_scope
            .contains("grants=[\"commands\"]")
    );
}

fn cap_026() {
    assert_eq!(
        hex(normalize(&http()).unwrap().scope_digest),
        "cfc368fb59a5bc99e1ac01229b69fba9fc4286efa1cc2ef4c1a53924a0caf083"
    );
}

fn cap_027() {
    let preview = normalize(&http()).unwrap();
    for field in [
        "model_version=1",
        "kind=http",
        "profile=agent-codex",
        "limits timeout_seconds=30",
        "audit enabled=false",
        "scope GET",
    ] {
        assert!(preview.effective_scope.contains(field));
    }
}

fn cap_028() {
    let preview = normalize(&http()).unwrap();
    assert!(approve(&preview.capability, Digest32::EMPTY, time()).is_err());
    assert!(
        approve(&preview.capability, preview.scope_digest, time())
            .unwrap()
            .enabled
    );
}

fn cap_029() {
    let preview = normalize(&http()).unwrap();
    let approved = approve(&preview.capability, preview.scope_digest, time()).unwrap();
    let disabled = disable(&approved);
    assert!(!disabled.enabled && disabled.approved_at.is_zero() && disabled.expires_at.is_zero());
}

fn cap_030() {
    let original = normalize(&http()).unwrap();
    let mut name = original.capability.clone();
    name.name = "renamed".to_owned();
    assert_eq!(
        normalize(&name).unwrap().scope_digest,
        original.scope_digest
    );
    let mut scope = original.capability;
    scope.http.as_mut().unwrap().methods.push("POST".to_owned());
    assert_ne!(
        normalize(&scope).unwrap().scope_digest,
        original.scope_digest
    );
}

fn cap_031() {
    let denied = authorize(
        &normalize(&http()).unwrap().capability,
        "agent-codex",
        time(),
    )
    .unwrap_err();
    assert_eq!(
        denied.to_string(),
        "capability denied: capability is disabled"
    );
}

fn cap_032() {
    let preview = normalize(&http()).unwrap();
    let approved = approve(&preview.capability, preview.scope_digest, time()).unwrap();
    let mut stale = approved.clone();
    stale.scope_digest = Digest32::EMPTY;
    stale.enabled = false;
    assert_eq!(
        authorize(&stale, "wrong", time()).unwrap_err().reason(),
        "approval scope is stale"
    );
}

fn cap_033() {
    super::policy_test::assert_http_request_byte_bounds();
}

fn cap_034() {
    super::policy_test::assert_git_delete_and_tag_gates();
}

fn cap_035() {
    let capability = approved(&ssh());
    assert!(
        authorize_ssh(
            &capability,
            "agent-codex",
            time(),
            SshOperation::RemoteCommand,
            "uptime"
        )
        .is_ok()
    );
    assert!(authorize_ssh(&capability, "agent-codex", time(), SshOperation::Git, "").is_err());
}

#[cfg(unix)]
fn cap_036() {
    super::policy_test::assert_portable_project_path_containment();
    super::policy_test::assert_unix_symlink_path_containment();
}

#[cfg(windows)]
fn cap_036() {
    super::policy_test::assert_portable_project_path_containment();
    super::policy_test::assert_windows_reparse_path_containment();
}

#[cfg(not(any(unix, windows)))]
fn cap_036() {
    super::policy_test::assert_portable_project_path_containment();
}

fn http() -> Capability {
    let mut value = draft(CapabilityKind::Http);
    value.http = Some(HttpScope {
        base_url: "https://example.com/api".to_owned(),
        methods: vec!["GET".to_owned()],
        path_prefixes: vec!["/api".to_owned()],
    });
    value
}

fn git() -> Capability {
    let mut value = draft(CapabilityKind::Git);
    value.git = Some(git_scope());
    value
}

fn git_scope() -> GitScope {
    GitScope {
        remote_name: "origin".to_owned(),
        remote_url: "https://example.com/repo.git".to_owned(),
        operations: vec!["fetch".to_owned()],
        branches: vec!["main".to_owned()],
        refspecs: vec!["main:main".to_owned()],
        allow_tags: false,
        allow_force_push: false,
        allow_delete_refs: false,
    }
}

fn ssh() -> Capability {
    let mut value = draft(CapabilityKind::Ssh);
    value.ssh = Some(SshScope {
        alias: "prod".to_owned(),
        host: "Example.COM.".to_owned(),
        port: 0,
        user: "deploy".to_owned(),
        host_key: "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA==".to_owned(),
        allow_git: false,
        remote_commands: vec!["uptime".to_owned()],
        allow_upload: false,
        allow_download: false,
        upload_roots: Vec::new(),
        download_roots: Vec::new(),
        upload_remote_roots: Vec::new(),
        download_remote_roots: Vec::new(),
        allow_interactive_shell: false,
        local_forward_targets: Vec::new(),
        remote_forward_targets: Vec::new(),
    });
    value
}

fn time() -> Timestamp {
    Timestamp::Fixed {
        seconds: 1_786_276_800,
        nanoseconds: 0,
        offset_seconds: 0,
    }
}

fn approved(value: &Capability) -> Capability {
    let preview = normalize(value).unwrap();
    approve(&preview.capability, preview.scope_digest, time()).unwrap()
}

fn hex(digest: Digest32) -> String {
    super::wire::encode_digest(digest)
}
