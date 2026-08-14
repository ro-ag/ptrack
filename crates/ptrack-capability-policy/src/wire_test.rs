use ptrack_core::{CapabilityKind, Digest32, HttpScope};

use super::normalize_test::draft;
use super::wire::{CapabilityDraftWire, CapabilityWire, decode_digest, encode_digest};

#[test]
fn empty_digest_is_go_empty_string_and_nonempty_is_strict_lower_hex() {
    assert_eq!(encode_digest(Digest32::EMPTY), "");
    assert_eq!(decode_digest("").unwrap(), Digest32::EMPTY);
    assert!(decode_digest(&"A".repeat(64)).is_err());
    assert!(decode_digest("00").is_err());
}

#[test]
fn draft_accepts_go_omissions_and_rejects_unknown_kind_and_fields() {
    let wire: CapabilityDraftWire = serde_json::from_str(
        r#"{"name":"api","kind":"http","agent_profile":"agent-codex","http":{"base_url":"https://example.com","methods":["GET"],"path_prefixes":[]}}"#,
    ).unwrap();
    let capability = ptrack_core::Capability::try_from(wire).unwrap();
    assert_eq!(capability.kind, CapabilityKind::Http);
    assert_eq!(capability.limits.timeout_seconds, 0);
    assert!(serde_json::from_str::<CapabilityDraftWire>(r#"{"name":"x","kind":"smtp"}"#).is_ok());
    assert!(
        ptrack_core::Capability::try_from(
            serde_json::from_str::<CapabilityDraftWire>(r#"{"name":"x","kind":"smtp"}"#).unwrap()
        )
        .unwrap_err()
        .to_string()
        .contains("unsupported capability kind")
    );
    assert!(
        serde_json::from_str::<CapabilityDraftWire>(r#"{"name":"x","kind":"http","extra":1}"#)
            .is_err()
    );
}

#[test]
fn full_wire_rejects_bad_digest_and_time_instead_of_defaulting() {
    let mut capability = draft(CapabilityKind::Http);
    capability.http = Some(HttpScope {
        base_url: "https://example.com".to_owned(),
        methods: vec!["GET".to_owned()],
        path_prefixes: Vec::new(),
    });
    let normalized = super::normalize(&capability).unwrap().capability;
    let mut value = serde_json::to_value(CapabilityWire::try_from(&normalized).unwrap()).unwrap();
    value["scope_digest"] = serde_json::Value::String("BAD".to_owned());
    let wire: CapabilityWire = serde_json::from_value(value).unwrap();
    assert!(ptrack_core::Capability::try_from(wire).is_err());
}
