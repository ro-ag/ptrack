use crate::{
    CapabilityAudit, CapabilityKind, Digest32, MAX_HOLD_REASON_BYTES, MemoryKind, Meta,
    NativeRecord, Note, NoteTarget, PlanStatus, TaskStatus, Timestamp, Validate, check_hold_reason,
};

use super::codec_test::valid_capability;

#[test]
fn note_summary_kind_is_never_persistable() {
    let note = NativeRecord::Note(Note {
        id: 1,
        target: NoteTarget::Project,
        target_id: 0,
        kind: MemoryKind::Summary,
        body: "rolling".to_owned(),
        created_at: Timestamp::Zero,
    });
    assert_eq!(
        note.validate().expect_err("summary note must fail").field(),
        "note.kind"
    );
}

#[test]
fn capability_requires_exact_scope_and_nonempty_digest() {
    let mut capability = valid_capability(CapabilityKind::Http);
    capability.git = valid_capability(CapabilityKind::Git).git;
    assert_eq!(
        capability
            .validate()
            .expect_err("mixed scopes must fail")
            .field(),
        "capability.scope"
    );

    capability.git = None;
    capability.scope_digest = Digest32::EMPTY;
    assert_eq!(
        capability
            .validate()
            .expect_err("empty digest must fail")
            .field(),
        "capability.scope_digest"
    );
}

#[test]
fn capability_approval_state_is_coherent() {
    let mut capability = valid_capability(CapabilityKind::Http);
    capability.enabled = true;
    assert_eq!(
        capability
            .validate()
            .expect_err("enabled requires approval")
            .field(),
        "capability.approved_at"
    );

    capability.approved_at = Timestamp::Fixed {
        seconds: 100,
        nanoseconds: 0,
        offset_seconds: 0,
    };
    capability.expires_at = Timestamp::Fixed {
        seconds: 3_701,
        nanoseconds: 0,
        offset_seconds: 0,
    };
    assert_eq!(
        capability
            .validate()
            .expect_err("expiry exceeds duration")
            .field(),
        "capability.expires_at"
    );

    capability.expires_at = Timestamp::Fixed {
        seconds: 3_700,
        nanoseconds: 0,
        offset_seconds: 0,
    };
    capability.validate().expect("coherent approval");

    capability.enabled = false;
    assert_eq!(
        capability
            .validate()
            .expect_err("disabled approval must be cleared")
            .field(),
        "capability.approval"
    );
}

#[test]
fn timestamp_rejects_noncanonical_components() {
    let timestamp = Timestamp::Fixed {
        seconds: 0,
        nanoseconds: 1_000_000_000,
        offset_seconds: 0,
    };
    assert_eq!(
        timestamp
            .validate()
            .expect_err("nanoseconds must be bounded")
            .field(),
        "timestamp.nanoseconds"
    );
}

#[test]
fn legacy_zero_format_meta_is_preserved_but_newer_formats_fail() {
    let mut meta = Meta {
        goal: String::new(),
        summary: String::new(),
        active_plan: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        format_version: 0,
        last_write_version: String::new(),
    };
    meta.validate().expect("legacy v0 is preserved");
    meta.format_version = 6;
    assert_eq!(
        meta.validate()
            .expect_err("future Go format must fail")
            .field(),
        "meta.format_version"
    );
}

#[test]
fn successful_audit_uses_go_none_error_class() {
    valid_audit()
        .validate()
        .expect("Go success class is canonical");
}

fn valid_audit() -> CapabilityAudit {
    CapabilityAudit {
        id: 1,
        capability_id: 2,
        agent_profile: "agent".to_owned(),
        kind: CapabilityKind::Http,
        operation: "GET".to_owned(),
        target: "https://example.test".to_owned(),
        success: true,
        error_class: "none".to_owned(),
        duration_millis: 0,
        request_bytes: 0,
        response_bytes: 0,
        redirects: 0,
        created_at: Timestamp::Zero,
    }
}

#[test]
fn failed_audit_rejects_non_allowlisted_error_text() {
    let mut audit = valid_audit();
    audit.success = false;
    audit.error_class = "secret diagnostic text".to_owned();
    assert!(audit.validate().is_err());
    audit.error_class = "timeout".to_owned();
    assert!(audit.validate().is_ok());
}

#[test]
fn hold_reason_is_bounded_single_line_text_when_set() {
    let mut plan = super::test_support::plan(1, "p", PlanStatus::Active, 0, 0);
    assert!(plan.validate().is_ok());

    for blank in ["", "   ", "\t "] {
        plan.hold_reason = Some(blank.to_owned());
        assert_eq!(
            plan.validate().expect_err("blank hold reason").reason(),
            "must be nonblank when set"
        );
    }

    plan.hold_reason = Some("x".repeat(MAX_HOLD_REASON_BYTES));
    assert!(plan.validate().is_ok());
    plan.hold_reason = Some("x".repeat(MAX_HOLD_REASON_BYTES + 1));
    assert_eq!(
        plan.validate().expect_err("oversized hold reason").reason(),
        "exceeds the hold reason bound"
    );

    for control in [
        "a\nb",
        "a\rb",
        "a\u{0}b",
        // Not `char::is_control`, but still line breaks and bidirectional
        // overrides that can hide what a reason really says.
        "a\u{2028}b",
        "a\u{2029}b",
        "a\u{202a}b",
        "a\u{202b}b",
        "a\u{202c}b",
        "a\u{202d}b",
        "a\u{202e}b",
        "a\u{2066}b",
        "a\u{2067}b",
        "a\u{2068}b",
        "a\u{2069}b",
        // Directional marks and zero-width characters: invisible, but they
        // can still reorder or hide what a reason really says.
        "a\u{061c}b",
        "a\u{200b}b",
        "a\u{200c}b",
        "a\u{200d}b",
        "a\u{200e}b",
        "a\u{200f}b",
    ] {
        plan.hold_reason = Some(control.to_owned());
        let error = plan.validate().expect_err("control character");
        assert_eq!(error.field(), "plan.hold_reason");
        assert_eq!(
            error.reason(),
            "must be single-line text without control characters"
        );
        assert!(check_hold_reason(control).is_err(), "{control:?}");
    }

    // The neighbours of every rejected range stay usable.
    for allowed in [
        "a\u{2027}b",
        "a\u{202f}b",
        "a\u{2065}b",
        "a\u{206a}b",
        "a\u{061b}b",
        "a\u{061d}b",
        "a\u{200a}b",
        "a\u{2010}b",
    ] {
        plan.hold_reason = Some(allowed.to_owned());
        assert!(plan.validate().is_ok(), "{allowed:?}");
    }

    let mut task = super::test_support::task(1, 2, "t", TaskStatus::Todo, 0);
    task.hold_reason = Some(" ".to_owned());
    assert_eq!(
        task.validate().expect_err("blank hold reason").field(),
        "task.hold_reason"
    );
}

#[test]
fn the_input_boundary_check_agrees_with_the_record_validator() {
    assert_eq!(check_hold_reason("waiting on review"), Ok(()));
    assert_eq!(
        check_hold_reason("   "),
        Err("the hold reason cannot be blank".to_owned())
    );
    assert_eq!(
        check_hold_reason("a\nb"),
        Err("the hold reason must be one line without control characters".to_owned())
    );
    assert_eq!(
        check_hold_reason(&"x".repeat(MAX_HOLD_REASON_BYTES + 1)),
        Err(format!(
            "the hold reason is {} bytes; the limit is {MAX_HOLD_REASON_BYTES}",
            MAX_HOLD_REASON_BYTES + 1
        ))
    );

    // Anything the boundary accepts must also survive the record validator.
    let mut plan = super::test_support::plan(1, "p", PlanStatus::Active, 0, 0);
    for reason in ["ok", &"x".repeat(MAX_HOLD_REASON_BYTES), "still ok"] {
        assert_eq!(check_hold_reason(reason), Ok(()));
        plan.hold_reason = Some(reason.to_owned());
        assert!(plan.validate().is_ok());
    }
}
