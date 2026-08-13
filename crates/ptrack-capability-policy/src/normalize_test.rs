use ptrack_core::{
    Capability, CapabilityAuditPolicy, CapabilityKind, CapabilityLimits, GitScope, HttpScope,
    SshScope, Timestamp,
};

use super::*;

type InvalidDraftCase = (fn(&mut Capability), &'static str);

pub(super) fn draft(kind: CapabilityKind) -> Capability {
    Capability {
        id: 0,
        model_version: 0,
        revision: 0,
        name: "fixture".to_owned(),
        kind,
        agent_profile: "agent-codex".to_owned(),
        enabled: false,
        approval_duration_seconds: 0,
        approved_at: Timestamp::Zero,
        expires_at: Timestamp::Zero,
        scope_digest: ptrack_core::Digest32::EMPTY,
        limits: CapabilityLimits {
            timeout_seconds: 0,
            max_request_bytes: 0,
            max_response_bytes: 0,
            max_output_bytes: 0,
            max_redirects: 0,
            max_concurrent: 0,
        },
        audit: CapabilityAuditPolicy {
            enabled: false,
            retain_last: 0,
        },
        http: None,
        git: None,
        ssh: None,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
    }
}

#[test]
fn http_normalizes_defaults_lists_and_ambiguous_paths() {
    let mut value = draft(CapabilityKind::Http);
    value.name = " API ".to_owned();
    value.audit.enabled = true;
    value.http = Some(HttpScope {
        base_url: "HTTPS://Example.COM:443/api/./v1/".to_owned(),
        methods: vec!["get".to_owned(), "HEAD".to_owned(), "get".to_owned()],
        path_prefixes: vec!["/api/v1/users".to_owned(), "/api/v1".to_owned()],
    });
    let preview = normalize(&value).unwrap();
    let scope = preview.capability.http.unwrap();
    assert_eq!(scope.base_url, "https://example.com/api/v1");
    assert_eq!(scope.methods, ["GET", "HEAD"]);
    assert!(preview.effective_scope.contains("max_redirects=0"));

    for raw in [
        "https://token@example.com/api",
        "https://example.com/api/%2e%2e/admin",
        "https://example.com/api%2fadmin",
        "https://example.com/api/%00",
    ] {
        let mut candidate = draft(CapabilityKind::Http);
        candidate.http = Some(HttpScope {
            base_url: raw.to_owned(),
            methods: vec!["GET".to_owned()],
            path_prefixes: Vec::new(),
        });
        assert!(normalize(&candidate).is_err(), "accepted {raw}");
    }
}

#[test]
fn git_rewrites_bare_refspecs_and_denies_bypasses() {
    let mut value = draft(CapabilityKind::Git);
    value.git = Some(GitScope {
        remote_name: "origin".to_owned(),
        remote_url: "https://example.com:443/repo.git".to_owned(),
        operations: vec!["push".to_owned()],
        branches: vec!["main".to_owned()],
        refspecs: vec!["main:main".to_owned()],
        allow_tags: false,
        allow_force_push: true,
        allow_delete_refs: false,
    });
    let preview = normalize(&value).unwrap();
    let scope = preview.capability.git.unwrap();
    assert_eq!(scope.remote_url, "https://example.com/repo.git");
    assert_eq!(scope.refspecs, ["refs/heads/main:refs/heads/main"]);

    for branch in ["+main", "refs/tags/v1", "HEAD", "@", "FETCH_HEAD"] {
        let mut candidate = value.clone();
        candidate.git.as_mut().unwrap().branches = vec![branch.to_owned()];
        assert!(normalize(&candidate).is_err(), "accepted {branch}");
    }
}

#[test]
fn ssh_requires_independent_paired_grants_and_rejects_line_separators() {
    let mut value = draft(CapabilityKind::Ssh);
    value.ssh = Some(SshScope {
        alias: "prod".to_owned(),
        host: "EXAMPLE.com.".to_owned(),
        port: 0,
        user: "deploy".to_owned(),
        host_key: "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA==".to_owned(),
        allow_git: false,
        remote_commands: vec!["uptime".to_owned()],
        allow_upload: true,
        allow_download: false,
        upload_roots: vec!["dist".to_owned()],
        download_roots: Vec::new(),
        upload_remote_roots: vec!["/srv/app".to_owned()],
        download_remote_roots: Vec::new(),
        allow_interactive_shell: false,
        local_forward_targets: Vec::new(),
        remote_forward_targets: Vec::new(),
    });
    let preview = normalize(&value).unwrap();
    assert_eq!(preview.capability.ssh.unwrap().host, "example.com");

    value.ssh.as_mut().unwrap().remote_commands = vec!["printf  ".to_owned()];
    assert!(normalize(&value).is_err());
    value.ssh.as_mut().unwrap().remote_commands.clear();
    value.ssh.as_mut().unwrap().alias = "prod/escape".to_owned();
    assert!(normalize(&value).is_err());
}

#[test]
fn canonical_json_matches_go_html_and_line_separator_escaping() {
    let value = vec!["<>&   "];
    assert_eq!(
        super::normalize::go_json(&value).unwrap(),
        "[\"\\u003c\\u003e\\u0026 \\u2028\\u2029\"]"
    );
}

#[test]
fn draft_contract_errors_defaults_bounds_and_exclusivity() {
    let mut value = draft(CapabilityKind::Http);
    value.http = Some(HttpScope {
        base_url: "https://example.com".to_owned(),
        methods: vec!["GET".to_owned()],
        path_prefixes: Vec::new(),
    });
    let preview = normalize(&value).unwrap();
    assert_eq!(preview.capability.model_version, 1);
    assert_eq!(preview.capability.approval_duration_seconds, 3_600);
    assert_eq!(preview.capability.limits.timeout_seconds, 30);
    assert_eq!(preview.capability.limits.max_request_bytes, 1 << 20);
    assert_eq!(preview.capability.limits.max_response_bytes, 4 << 20);
    assert_eq!(preview.capability.limits.max_output_bytes, 1 << 20);
    assert_eq!(preview.capability.limits.max_redirects, 0);
    assert_eq!(preview.capability.limits.max_concurrent, 1);
    assert_eq!(preview.capability.audit.retain_last, 100);

    let cases: &[InvalidDraftCase] = &[
        (
            |candidate: &mut Capability| candidate.model_version = 2,
            "unsupported capability model version",
        ),
        (
            |candidate: &mut Capability| candidate.name.clear(),
            "capability name",
        ),
        (
            |candidate: &mut Capability| candidate.agent_profile = "-agent".to_owned(),
            "agent profile",
        ),
        (
            |candidate: &mut Capability| candidate.approval_duration_seconds = 59,
            "approval duration",
        ),
        (
            |candidate: &mut Capability| candidate.limits.timeout_seconds = 301,
            "timeout",
        ),
        (
            |candidate: &mut Capability| candidate.limits.max_request_bytes = (32 << 20) + 1,
            "maximum request",
        ),
        (
            |candidate: &mut Capability| candidate.limits.max_redirects = 11,
            "maximum redirects",
        ),
        (
            |candidate: &mut Capability| candidate.limits.max_concurrent = 9,
            "maximum concurrent",
        ),
        (
            |candidate: &mut Capability| candidate.audit.retain_last = 1_001,
            "audit retention",
        ),
    ];
    for (mutate, message) in cases {
        let mut candidate = value.clone();
        mutate(&mut candidate);
        assert!(
            normalize(&candidate)
                .unwrap_err()
                .to_string()
                .contains(message)
        );
    }
    let mut cross_kind = value.clone();
    cross_kind.git = Some(GitScope {
        remote_name: "origin".to_owned(),
        remote_url: "https://example.com/repo.git".to_owned(),
        operations: vec!["fetch".to_owned()],
        branches: Vec::new(),
        refspecs: Vec::new(),
        allow_tags: false,
        allow_force_push: false,
        allow_delete_refs: false,
    });
    assert_eq!(
        normalize(&cross_kind).unwrap_err().to_string(),
        "HTTP capability must contain only an HTTP scope"
    );
    let mut unicode_profile = value.clone();
    unicode_profile.agent_profile = "agent-١".to_owned();
    assert!(normalize(&unicode_profile).is_ok());
    unicode_profile.agent_profile = "agent-²".to_owned();
    assert!(normalize(&unicode_profile).is_err());
}

#[test]
fn strict_url_parser_rejects_whatwg_coercions_and_decodes_git_path_once() {
    for raw in [
        "example.com/api",
        "http:example.com/api",
        "https://@example.com/api",
        "https://127.1/api",
        "https://0x7f000001/api",
        "https://aé.example/api",
    ] {
        assert!(
            super::normalize::normalize_http_url(raw, false).is_err(),
            "accepted {raw}"
        );
    }
    for raw in [
        "https:example.com/repo.git",
        "https://example.com/a/%2e%2e/repo.git",
        "ssh://git@example.com/a%5Crepo.git",
        "ssh://git@example.com/a%2500repo.git",
    ] {
        assert!(
            super::normalize::normalize_git_remote(raw).is_err(),
            "accepted {raw}"
        );
    }
}
