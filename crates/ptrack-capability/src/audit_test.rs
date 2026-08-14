use ptrack_capability_policy::AuditEvent;

use super::audit::AuditRecorder;
use super::test_support::{TempDir, approved_http, store};

#[test]
fn recorder_skips_absent_store_and_disabled_policy() {
    let capability = approved_http("https://example.com/api");
    AuditRecorder::new(None)
        .record(&capability, &event(true))
        .unwrap();

    let temp = TempDir::new("audit-disabled");
    let store = store(&temp);
    let mut disabled = capability;
    disabled.audit.enabled = false;
    AuditRecorder::new(Some(&store))
        .record(&disabled, &event(true))
        .unwrap();
    assert!(store.capability_audits(0, 0).unwrap().is_empty());
}

#[test]
fn recorder_persists_only_sanitized_bounded_shape() {
    let temp = TempDir::new("audit-sanitized");
    let store = store(&temp);
    let capability = approved_http("https://example.com/api");
    AuditRecorder::new(Some(&store))
        .record(&capability, &event(false))
        .unwrap();
    let audits = store.capability_audits(capability.id, 1).unwrap();
    let audit = &audits[0];
    assert_eq!(audit.capability_id, capability.id);
    assert_eq!(audit.agent_profile, "agent-codex");
    assert_eq!(audit.operation, "unknown");
    assert_eq!(audit.target, "https://example.com");
    assert_eq!(audit.error_class, "internal");
    assert_eq!(audit.duration_millis, 86_400_000);
    assert_eq!(audit.request_bytes, 0);
    assert_eq!(audit.response_bytes, 1_i64 << 40);
    assert_eq!(audit.redirects, 10);
}

#[test]
fn recorder_reports_sanitized_storage_failure() {
    let temp = TempDir::new("audit-error");
    let store = store(&temp);
    let mut capability = approved_http("https://example.com/api");
    capability.audit.retain_last = 1_001;
    let error = AuditRecorder::new(Some(&store))
        .record(&capability, &event(true))
        .unwrap_err();
    assert_eq!(error.to_string(), "record capability audit: internal");
}

pub(super) fn assert_cap_055_through_062_audit_contract() {
    recorder_skips_absent_store_and_disabled_policy();
    recorder_persists_only_sanitized_bounded_shape();
    recorder_reports_sanitized_storage_failure();
}

fn event(success: bool) -> AuditEvent {
    AuditEvent {
        operation: "SECRET method".to_owned(),
        target: "https://example.com/private?token=SECRET".to_owned(),
        success,
        error_class: "SECRET class".to_owned(),
        duration_millis: i64::MAX,
        request_bytes: -1,
        response_bytes: i64::MAX,
        redirects: i64::MAX,
    }
}
