use ptrack_core::{CapabilityKind, HttpScope};

use super::normalize_test::draft;
use super::*;

#[test]
fn audit_sanitizer_removes_secrets_and_bounds_every_counter() {
    let mut capability = draft(CapabilityKind::Http);
    capability.id = 7;
    capability.audit.enabled = true;
    capability.audit.retain_last = 20;
    capability.http = Some(HttpScope {
        base_url: "https://example.com".to_owned(),
        methods: vec!["GET".to_owned()],
        path_prefixes: Vec::new(),
    });
    let audit = sanitize_audit(
        &capability,
        &AuditEvent {
            operation: "GET".to_owned(),
            target: "https://example.com/private?token=secret".to_owned(),
            success: false,
            error_class: "raw secret stderr".to_owned(),
            duration_millis: i64::MAX,
            request_bytes: -1,
            response_bytes: i64::MAX,
            redirects: 99,
        },
    )
    .unwrap();
    let (record, retain) = audit.into_store_parts(ptrack_core::Timestamp::Zero);
    assert_eq!(retain, 20);
    assert_eq!(record.target, "https://example.com");
    assert_eq!(record.error_class, "internal");
    assert_eq!(record.duration_millis, 86_400_000);
    assert_eq!(record.request_bytes, 0);
    assert_eq!(record.response_bytes, 1 << 40);
    assert_eq!(record.redirects, 10);
}
