use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_core::{Capability, CapabilityKind, Digest32, GitScope, HttpScope, SshScope, Timestamp};

use super::normalize_test::draft;
use super::*;

static NEXT_PATH_TEST: AtomicU64 = AtomicU64::new(1);

fn now() -> Timestamp {
    Timestamp::Fixed {
        seconds: 1_786_276_800,
        nanoseconds: 0,
        offset_seconds: 0,
    }
}

fn approved_http() -> Capability {
    let mut value = draft(CapabilityKind::Http);
    value.http = Some(HttpScope {
        base_url: "https://example.com/api".to_owned(),
        methods: vec!["GET".to_owned()],
        path_prefixes: vec!["/api/v1".to_owned()],
    });
    let preview = normalize(&value).unwrap();
    approve(&preview.capability, preview.scope_digest, now()).unwrap()
}

fn approved_git() -> Capability {
    let mut value = draft(CapabilityKind::Git);
    value.git = Some(GitScope {
        remote_name: "origin".to_owned(),
        remote_url: "https://example.com/repo.git".to_owned(),
        operations: vec!["fetch".to_owned(), "push".to_owned()],
        branches: vec!["main".to_owned()],
        refspecs: vec!["main:main".to_owned()],
        allow_tags: false,
        allow_force_push: false,
        allow_delete_refs: false,
    });
    let preview = normalize(&value).unwrap();
    approve(&preview.capability, preview.scope_digest, now()).unwrap()
}

fn approved_ssh() -> Capability {
    let mut value = draft(CapabilityKind::Ssh);
    value.ssh = Some(SshScope {
        alias: "prod".to_owned(),
        host: "example.com".to_owned(),
        port: 22,
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
        local_forward_targets: vec!["db.internal:5432".to_owned()],
        remote_forward_targets: Vec::new(),
    });
    let preview = normalize(&value).unwrap();
    approve(&preview.capability, preview.scope_digest, now()).unwrap()
}

#[test]
fn approve_and_authorize_fail_closed_on_digest_profile_disable_and_expiry() {
    let approved = approved_http();
    assert!(authorize(&approved, "agent-codex", now()).is_ok());

    let mut candidate = approved.clone();
    candidate.scope_digest = Digest32::EMPTY;
    assert_eq!(
        authorize(&candidate, "agent-codex", now())
            .unwrap_err()
            .reason(),
        "approval scope is stale"
    );
    assert_eq!(
        authorize(&approved, "other", now()).unwrap_err().reason(),
        "agent profile does not match"
    );
    assert_eq!(
        authorize(&disable(&approved), "agent-codex", now())
            .unwrap_err()
            .reason(),
        "capability is disabled"
    );
    candidate = approved;
    candidate.expires_at = now();
    assert_eq!(
        authorize(&candidate, "agent-codex", now())
            .unwrap_err()
            .reason(),
        "capability approval has expired"
    );
}

#[test]
fn http_authorization_requires_exact_origin_method_path_and_size() {
    let capability = approved_http();
    assert!(
        authorize_http(
            &capability,
            "agent-codex",
            now(),
            "GET",
            "https://example.com/api/v1/items?transient=1",
            0,
        )
        .is_ok()
    );
    for (method, url) in [
        ("POST", "https://example.com/api/v1/items"),
        ("GET", "https://example.com/api/v10/items"),
        ("GET", "https://evil.example/api/v1/items"),
    ] {
        assert!(authorize_http(&capability, "agent-codex", now(), method, url, 0).is_err());
    }
}

#[test]
fn http_request_byte_bounds_are_exact_and_fail_closed() {
    assert_http_request_byte_bounds();
}

pub(super) fn assert_http_request_byte_bounds() {
    let capability = approved_http();
    let limit = capability.limits.max_request_bytes;
    for allowed in [0, limit] {
        assert!(
            authorize_http(
                &capability,
                "agent-codex",
                now(),
                "GET",
                "https://example.com/api/v1/items",
                allowed,
            )
            .is_ok()
        );
    }
    for denied in [-1, limit + 1] {
        assert_eq!(
            authorize_http(
                &capability,
                "agent-codex",
                now(),
                "GET",
                "https://example.com/api/v1/items",
                denied,
            )
            .unwrap_err()
            .reason(),
            "HTTP request exceeds its byte limit"
        );
    }
}

#[test]
fn authorization_denial_order_is_stable() {
    let approved = approved_http();
    let mut candidate = approved.clone();
    candidate.name.clear();
    candidate.scope_digest = Digest32::EMPTY;
    assert_eq!(
        authorize(&candidate, "wrong", now()).unwrap_err().reason(),
        "stored capability is invalid"
    );
    candidate = approved.clone();
    candidate.scope_digest = Digest32::EMPTY;
    candidate.enabled = false;
    assert_eq!(
        authorize(&candidate, "wrong", now()).unwrap_err().reason(),
        "approval scope is stale"
    );
    candidate = approved.clone();
    candidate.enabled = false;
    assert_eq!(
        authorize(&candidate, "wrong", now()).unwrap_err().reason(),
        "capability is disabled"
    );
    assert_eq!(
        authorize(&approved, "wrong", now()).unwrap_err().reason(),
        "agent profile does not match"
    );
    candidate = approved.clone();
    candidate.approved_at = Timestamp::Zero;
    assert_eq!(
        authorize(&candidate, "agent-codex", now())
            .unwrap_err()
            .reason(),
        "capability has not been approved"
    );
    candidate = approved.clone();
    candidate.expires_at = now();
    assert_eq!(
        authorize(&candidate, "agent-codex", now())
            .unwrap_err()
            .reason(),
        "capability approval has expired"
    );
    candidate = approved;
    candidate.expires_at = Timestamp::Fixed {
        seconds: 1_786_276_800 + 7_200,
        nanoseconds: 0,
        offset_seconds: 0,
    };
    assert_eq!(
        authorize(&candidate, "agent-codex", now())
            .unwrap_err()
            .reason(),
        "approval expiry exceeds its duration"
    );
}

#[test]
fn git_authorization_dimensions_are_independent() {
    let capability = approved_git();
    let allowed = GitAuthorization {
        operation: "fetch".to_owned(),
        remote_name: "origin".to_owned(),
        remote_url: "https://example.com/repo.git".to_owned(),
        branch: "main".to_owned(),
        refspec: String::new(),
        force: false,
    };
    assert!(authorize_git(&capability, "agent-codex", now(), &allowed).is_ok());
    let mutations: &[fn(&mut GitAuthorization)] = &[
        |request: &mut GitAuthorization| request.operation = "status".to_owned(),
        |request: &mut GitAuthorization| request.remote_name = "upstream".to_owned(),
        |request: &mut GitAuthorization| {
            request.remote_url = "https://evil.example/repo.git".to_owned();
        },
        |request: &mut GitAuthorization| request.branch = "release".to_owned(),
        |request: &mut GitAuthorization| {
            request.refspec = "refs/heads/release:refs/heads/release".to_owned();
        },
        |request: &mut GitAuthorization| request.force = true,
    ];
    for mutate in mutations {
        let mut request = allowed.clone();
        mutate(&mut request);
        assert!(authorize_git(&capability, "agent-codex", now(), &request).is_err());
    }
}

#[test]
fn git_delete_and_tag_gates_have_exact_deny_and_allow_outcomes() {
    assert_git_delete_and_tag_gates();
}

pub(super) fn assert_git_delete_and_tag_gates() {
    let mut scope = GitScope {
        remote_name: "origin".to_owned(),
        remote_url: "https://example.com/repo.git".to_owned(),
        operations: vec!["push".to_owned()],
        branches: vec!["main".to_owned()],
        refspecs: vec![
            ":refs/heads/obsolete".to_owned(),
            "refs/tags/v1:refs/tags/v1".to_owned(),
        ],
        allow_tags: false,
        allow_force_push: false,
        allow_delete_refs: false,
    };
    let mut request = GitAuthorization {
        operation: "push".to_owned(),
        remote_name: "origin".to_owned(),
        remote_url: "https://example.com/repo.git".to_owned(),
        branch: "main".to_owned(),
        refspec: ":refs/heads/obsolete".to_owned(),
        force: false,
    };
    assert_eq!(
        super::policy::authorize_git_scope(&scope, &request)
            .unwrap_err()
            .reason(),
        "Git ref deletion is not approved"
    );
    scope.allow_delete_refs = true;
    assert!(super::policy::authorize_git_scope(&scope, &request).is_ok());

    request.refspec = "refs/tags/v1:refs/tags/v1".to_owned();
    assert_eq!(
        super::policy::authorize_git_scope(&scope, &request)
            .unwrap_err()
            .reason(),
        "Git tag writes are not approved"
    );
    scope.allow_tags = true;
    assert!(super::policy::authorize_git_scope(&scope, &request).is_ok());
}

#[test]
fn ssh_grants_are_independent_and_endpoints_are_exact() {
    let capability = approved_ssh();
    assert!(
        authorize_ssh(
            &capability,
            "agent-codex",
            now(),
            SshOperation::RemoteCommand,
            "uptime"
        )
        .is_ok()
    );
    assert!(
        authorize_ssh(
            &capability,
            "agent-codex",
            now(),
            SshOperation::LocalForward,
            "DB.INTERNAL:5432"
        )
        .is_ok()
    );
    for (operation, value) in [
        (SshOperation::Git, ""),
        (SshOperation::RemoteCommand, "uptime "),
        (SshOperation::Upload, ""),
        (SshOperation::Download, ""),
        (SshOperation::InteractiveShell, ""),
        (SshOperation::RemoteForward, "db.internal:5432"),
        (SshOperation::LocalForward, "db.internal:5433"),
    ] {
        assert!(authorize_ssh(&capability, "agent-codex", now(), operation, value).is_err());
    }
}

#[test]
fn project_path_resolution_enforces_portable_actual_containment() {
    assert_portable_project_path_containment();
}

pub(super) fn assert_portable_project_path_containment() {
    let temp = path_test_root("portable");
    let _ = fs::remove_dir_all(&temp);
    let project = temp.join("project");
    fs::create_dir_all(project.join("dist/nested")).unwrap();
    fs::create_dir_all(project.join("private")).unwrap();
    fs::write(project.join("dist/app.js"), "ok").unwrap();
    fs::write(project.join("dist/not-a-directory"), "file").unwrap();
    let roots = vec!["dist".to_owned()];
    assert!(resolve_project_path(&project, "dist/app.js", &roots, true).is_ok());
    assert!(resolve_project_path(&project, "dist/nested/new.js", &roots, false).is_ok());
    assert_eq!(
        resolve_project_path(&project, "private/secret", &roots, false)
            .unwrap_err()
            .reason(),
        "path is outside approved roots"
    );
    assert_eq!(
        resolve_project_path(&project, "../escape", &roots, false)
            .unwrap_err()
            .reason(),
        "path is not project-relative"
    );
    assert_eq!(
        resolve_project_path(&project, "dist/missing", &roots, true)
            .unwrap_err()
            .reason(),
        "path escapes the project"
    );
    assert_eq!(
        resolve_project_path(&project, "dist/not-a-directory/child", &roots, false,)
            .unwrap_err()
            .reason(),
        "path escapes the project"
    );
    fs::remove_dir_all(&temp).unwrap();
}

#[cfg(unix)]
#[test]
fn project_path_resolution_denies_existing_and_missing_symlink_escapes() {
    assert_unix_symlink_path_containment();
}

#[cfg(unix)]
pub(super) fn assert_unix_symlink_path_containment() {
    let temp = path_test_root("unix-symlink");
    let _ = fs::remove_dir_all(&temp);
    let project = temp.join("project");
    let outside = temp.join("outside");
    fs::create_dir_all(project.join("dist")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret"), "secret").unwrap();
    std::os::unix::fs::symlink(&outside, project.join("dist/escape")).unwrap();
    std::os::unix::fs::symlink("loop", project.join("dist/loop")).unwrap();
    let roots = vec!["dist".to_owned()];
    for (requested, must_exist) in [("dist/escape/secret", true), ("dist/escape/new", false)] {
        assert_eq!(
            resolve_project_path(&project, requested, &roots, must_exist)
                .unwrap_err()
                .reason(),
            "path escapes the project"
        );
    }
    assert!(
        resolve_project_path(
            &project,
            "dist/escape/new",
            &["dist/escape".to_owned()],
            false
        )
        .is_err()
    );
    assert_eq!(
        resolve_project_path(&project, "dist/loop/new", &roots, false)
            .unwrap_err()
            .reason(),
        "path escapes the project"
    );
    fs::remove_dir_all(&temp).unwrap();
}

#[cfg(windows)]
#[test]
fn project_path_resolution_denies_windows_reparse_escapes() {
    assert_windows_reparse_path_containment();
}

#[cfg(windows)]
pub(super) fn assert_windows_reparse_path_containment() {
    let temp = path_test_root("windows-reparse");
    let _ = fs::remove_dir_all(&temp);
    let project = temp.join("project");
    let outside = temp.join("outside");
    fs::create_dir_all(project.join("dist")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    // Symlink creation on stock Windows requires Developer Mode or
    // SeCreateSymbolicLinkPrivilege (os error 1314); only that specific
    // failure skips, and the assertions run untouched when it succeeds.
    if let Err(error) = std::os::windows::fs::symlink_dir(&outside, project.join("dist/escape")) {
        if error.raw_os_error() == Some(1314) {
            eprintln!("SKIP: symlink privilege not held (enable Developer Mode to run this test)");
            return;
        }
        panic!("create windows directory symlink: {error}");
    }
    let roots = vec!["dist".to_owned()];
    assert_eq!(
        resolve_project_path(&project, "dist/escape/new", &roots, false)
            .unwrap_err()
            .reason(),
        "path escapes the project"
    );
    fs::remove_dir_all(&temp).unwrap();
}

fn path_test_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "ptrack-capability-{label}-{}-{}",
        std::process::id(),
        NEXT_PATH_TEST.fetch_add(1, Ordering::Relaxed)
    ))
}
