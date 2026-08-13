use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_capability_policy::{approve, normalize};
use ptrack_core::{
    Capability, CapabilityAuditPolicy, CapabilityKind, CapabilityLimits, Digest32, GitScope,
    HttpScope, SshScope, Timestamp,
};
use ptrack_store::{ActiveBinding, ProjectStore, StoreKind};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

pub(super) struct TempDir(PathBuf);

impl TempDir {
    pub(super) fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-capability-{label}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn store(temp: &TempDir) -> ProjectStore {
    let path = temp.path().join("project.redb");
    ProjectStore::create_new(
        &path,
        ActiveBinding {
            generation: 1,
            database_id: "capability-test".to_owned(),
            kind: StoreKind::Project,
            canonical_path: temp.path().canonicalize().unwrap().join("project.redb"),
        },
        "test",
    )
    .unwrap()
}

pub(super) fn binding(temp: &TempDir) -> ActiveBinding {
    ActiveBinding {
        generation: 1,
        database_id: "capability-test".to_owned(),
        kind: StoreKind::Project,
        canonical_path: temp.path().canonicalize().unwrap().join("project.redb"),
    }
}

pub(super) fn store_at(temp: &TempDir) -> (ProjectStore, PathBuf, ActiveBinding) {
    let path = temp.path().join("project.redb");
    let binding = binding(temp);
    let store = ProjectStore::create_new(&path, binding.clone(), "test").unwrap();
    (store, path, binding)
}

pub(super) fn approved_http(base_url: &str) -> Capability {
    approved(&Capability {
        http: Some(HttpScope {
            base_url: base_url.to_owned(),
            methods: vec!["GET".to_owned(), "POST".to_owned()],
            path_prefixes: vec!["/api".to_owned()],
        }),
        ..draft(CapabilityKind::Http)
    })
}

pub(super) fn approved_git(remote_url: &str, operations: &[&str]) -> Capability {
    approved(&Capability {
        git: Some(GitScope {
            remote_name: "origin".to_owned(),
            remote_url: remote_url.to_owned(),
            operations: operations.iter().map(|value| (*value).to_owned()).collect(),
            branches: vec!["main".to_owned()],
            refspecs: vec!["refs/heads/main:refs/heads/main".to_owned()],
            allow_tags: false,
            allow_force_push: false,
            allow_delete_refs: false,
        }),
        ..draft(CapabilityKind::Git)
    })
}

pub(super) fn approved_ssh(host: &str, user: &str) -> Capability {
    approved(&Capability {
        ssh: Some(SshScope {
            alias: String::new(),
            host: host.to_owned(),
            port: 22,
            user: user.to_owned(),
            host_key: "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA==".to_owned(),
            allow_git: true,
            remote_commands: Vec::new(),
            allow_upload: false,
            allow_download: false,
            upload_roots: Vec::new(),
            download_roots: Vec::new(),
            upload_remote_roots: Vec::new(),
            download_remote_roots: Vec::new(),
            allow_interactive_shell: false,
            local_forward_targets: Vec::new(),
            remote_forward_targets: Vec::new(),
        }),
        ..draft(CapabilityKind::Ssh)
    })
}

pub(super) fn refresh_approval(mut capability: Capability) -> Capability {
    capability.enabled = false;
    capability.approved_at = Timestamp::Zero;
    capability.expires_at = Timestamp::Zero;
    capability.scope_digest = Digest32::EMPTY;
    approved(&capability)
}

fn approved(capability: &Capability) -> Capability {
    let preview = normalize(capability).unwrap();
    approve(&preview.capability, preview.scope_digest, now()).unwrap()
}

pub(super) fn draft(kind: CapabilityKind) -> Capability {
    Capability {
        id: 11,
        model_version: 1,
        revision: 1,
        name: "runtime".to_owned(),
        kind,
        agent_profile: "agent-codex".to_owned(),
        enabled: false,
        approval_duration_seconds: 3_600,
        approved_at: Timestamp::Zero,
        expires_at: Timestamp::Zero,
        scope_digest: Digest32::EMPTY,
        limits: CapabilityLimits {
            timeout_seconds: 2,
            max_request_bytes: 1_024,
            max_response_bytes: 1_024,
            max_output_bytes: 1_024,
            max_redirects: 2,
            max_concurrent: 1,
        },
        audit: CapabilityAuditPolicy {
            enabled: true,
            retain_last: 100,
        },
        http: None,
        git: None,
        ssh: None,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
    }
}

pub(super) fn now() -> Timestamp {
    Timestamp::Fixed {
        seconds: 1_800_000_000,
        nanoseconds: 0,
        offset_seconds: 0,
    }
}
