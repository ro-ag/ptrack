use crate::{
    CapabilityAudit, CapabilityKind, Digest32, LEGACY_ACTOR, MAX_HOLD_REASON_BYTES,
    MAX_IDENTITY_NAME_BYTES, MemoryKind, Meta, NativeRecord, Note, NoteTarget, PlanStatus,
    TaskStatus, Timestamp, Validate, check_hold_reason, check_identity_name, is_identity_id,
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
        actor: None,
        ulid: None,
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
        active_plans: Vec::new(),
        actors: Vec::new(),
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
        // The remaining invisibles: byte order mark, word joiner, Mongolian
        // vowel separator, and the tag block that mirrors ASCII invisibly.
        "a\u{feff}b",
        "a\u{2060}b",
        "a\u{180e}b",
        "a\u{e0000}b",
        "a\u{e0041}b",
        "a\u{e007f}b",
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
        "a\u{205f}b",
        "a\u{180f}b",
        "a\u{e0080}b",
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

#[test]
fn identity_ids_are_26_char_lowercase_crockford_base32() {
    assert!(is_identity_id("01hzvyekq3s7m8w9x0abcdefgh"));
    assert!(!is_identity_id(""));
    assert!(!is_identity_id("legacy"));
    assert!(!is_identity_id("01HZVYEKQ3S7M8W9X0ABCDEFGH")); // uppercase
    assert!(!is_identity_id("01hzvyekq3s7m8w9x0abcdefg")); // 25 chars
    assert!(!is_identity_id("01hzvyekq3s7m8w9x0abcdefghi")); // 27 chars
    assert!(!is_identity_id("01hzvyekq3s7m8w9x0abcdefgi")); // 'i' excluded
    assert!(!is_identity_id("01hzvyekq3s7m8w9x0abcdefgl")); // 'l' excluded
    assert!(!is_identity_id("01hzvyekq3s7m8w9x0abcdefgo")); // 'o' excluded
    assert!(!is_identity_id("01hzvyekq3s7m8w9x0abcdefgu")); // 'u' excluded
}

#[test]
fn identity_names_are_bounded_single_line_text() {
    assert_eq!(check_identity_name("Rodrigo"), Ok(()));
    assert_eq!(
        check_identity_name(&"x".repeat(MAX_IDENTITY_NAME_BYTES)),
        Ok(())
    );
    assert!(check_identity_name("").is_err());
    assert!(check_identity_name("   ").is_err());
    assert!(check_identity_name(&"x".repeat(MAX_IDENTITY_NAME_BYTES + 1)).is_err());
    assert!(check_identity_name("two\nlines").is_err());
    assert!(check_identity_name("bidi\u{202e}trick").is_err());
}

#[test]
fn stored_actor_fields_must_be_identity_ids() {
    const ACTOR: &str = "01hzvyekq3s7m8w9x0abcdefgh";
    let mut plan = super::test_support::plan(1, "p", PlanStatus::Active, 0, 0);
    plan.actor = Some(ACTOR.to_owned());
    plan.ulid = Some(ACTOR.to_owned());
    plan.claim_owner = Some(ACTOR.to_owned());
    plan.claim_epoch = 1;
    assert!(plan.validate().is_ok());

    // The `legacy` presentation sentinel is never storable, so an unattributed
    // record and an actor literally named "legacy" can never be confused.
    for rejected in [LEGACY_ACTOR, "", "01HZVYEKQ3S7M8W9X0ABCDEFGH"] {
        plan.actor = Some(rejected.to_owned());
        assert_eq!(
            plan.validate().expect_err("non-identity actor").field(),
            "plan.actor"
        );
    }
    plan.actor = Some(ACTOR.to_owned());
    plan.claim_owner = Some("nope".to_owned());
    assert_eq!(
        plan.validate()
            .expect_err("non-identity claim owner")
            .field(),
        "plan.claim_owner"
    );

    let mut task = super::test_support::task(1, 2, "t", TaskStatus::Todo, 0);
    task.ulid = Some("nope".to_owned());
    assert_eq!(
        task.validate().expect_err("non-identity ulid").field(),
        "task.ulid"
    );
}

#[test]
fn meta_actor_maps_must_be_sorted_identity_keyed_and_bounded() {
    const FIRST: &str = "01hzvyekq3s7m8w9x0abcdefgh";
    const SECOND: &str = "01hzvyekq3s7m8w9x0abcdefgj";
    let mut meta = super::test_support::meta(1);
    meta.active_plans = vec![(FIRST.to_owned(), 4), (SECOND.to_owned(), 0)];
    meta.actors = vec![(FIRST.to_owned(), "Rodrigo".to_owned())];
    assert!(meta.validate().is_ok());

    for unsorted in [
        vec![(SECOND.to_owned(), 4), (FIRST.to_owned(), 0)],
        vec![(FIRST.to_owned(), 4), (FIRST.to_owned(), 0)],
    ] {
        meta.active_plans = unsorted;
        assert_eq!(
            meta.validate().expect_err("unsorted keys").reason(),
            "must be sorted strictly ascending by identity id"
        );
    }
    meta.active_plans = vec![(LEGACY_ACTOR.to_owned(), 4)];
    assert_eq!(
        meta.validate().expect_err("non-identity key").reason(),
        "must key by identity ids"
    );

    meta.active_plans = Vec::new();
    meta.actors = vec![(FIRST.to_owned(), "two\nlines".to_owned())];
    assert_eq!(
        meta.validate().expect_err("unprintable name").reason(),
        "must hold bounded single-line names"
    );
    meta.actors = vec![(FIRST.to_owned(), "x".repeat(MAX_IDENTITY_NAME_BYTES + 1))];
    assert!(meta.validate().is_err());
}

#[test]
fn plan_claims_must_be_internally_consistent() {
    const ACTOR: &str = "01hzvyekq3s7m8w9x0abcdefgh";
    let mut plan = super::test_support::plan(1, "p", PlanStatus::Active, 0, 0);
    // Never claimed, and released-with-preserved-epoch, are both valid.
    assert!(plan.validate().is_ok());
    plan.claim_epoch = 4;
    assert!(plan.validate().is_ok());

    plan.claim_owner = Some(ACTOR.to_owned());
    assert!(plan.validate().is_ok());
    plan.claim_epoch = 0;
    assert_eq!(
        plan.validate().expect_err("owner without epoch").field(),
        "plan.claim_epoch"
    );

    // The conflict marker is accepted but never written; it only annotates a
    // live claim.
    plan.claim_epoch = 1;
    plan.claim_conflict = true;
    assert!(plan.validate().is_ok());
    plan.claim_owner = None;
    assert_eq!(
        plan.validate().expect_err("conflict without owner").field(),
        "plan.claim_conflict"
    );
}

#[test]
fn deps_must_be_nonzero_unique_and_never_self_referential() {
    let mut plan = super::test_support::plan(1, "p", PlanStatus::Active, 0, 0);
    plan.deps = vec![2, 3];
    assert!(plan.validate().is_ok());
    plan.deps = vec![0];
    assert_eq!(
        plan.validate().expect_err("zero dep").reason(),
        "must hold nonzero ids"
    );
    plan.deps = vec![1];
    assert_eq!(
        plan.validate().expect_err("self dep").reason(),
        "must not reference itself"
    );
    plan.deps = vec![2, 3, 2];
    assert_eq!(
        plan.validate().expect_err("duplicate dep").field(),
        "plan.deps"
    );

    let mut task = super::test_support::task(1, 2, "t", TaskStatus::Todo, 0);
    task.deps = vec![4];
    assert!(task.validate().is_ok());
    task.deps = vec![1];
    assert_eq!(task.validate().expect_err("self dep").field(), "task.deps");
    task.deps = vec![4, 4];
    assert_eq!(
        task.validate().expect_err("duplicate dep").reason(),
        "must not repeat an id"
    );
}
