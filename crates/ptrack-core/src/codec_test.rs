use crate::{
    Capability, CapabilityAuditPolicy, CapabilityKind, CapabilityLimits, CodecError, Digest32,
    GitScope, HttpScope, IssueStatus, MAX_LIST_ITEMS, MAX_PAYLOAD_BYTES, MIN_NATIVE_PAYLOAD_SCHEMA,
    MemoryKind, Meta, MilestoneStatus, NATIVE_PAYLOAD_SCHEMA, NativeRecord, Note, NoteTarget, Plan,
    PlanStatus, RecordKind, Severity, SshScope, Task, TaskStatus, Timestamp, decode_record,
    decode_record_at_schema, encode_record,
};

fn fixed_time() -> Timestamp {
    Timestamp::Fixed {
        seconds: 1_700_000_000,
        nanoseconds: 123_456_789,
        offset_seconds: -25_200,
    }
}

pub(crate) fn valid_capability(kind: CapabilityKind) -> Capability {
    Capability {
        id: 7,
        model_version: 1,
        revision: 2,
        name: "scope".to_owned(),
        kind,
        agent_profile: "agent-codex".to_owned(),
        enabled: false,
        approval_duration_seconds: 3_600,
        approved_at: Timestamp::Zero,
        expires_at: Timestamp::Zero,
        scope_digest: Digest32([0x5a; 32]),
        limits: CapabilityLimits {
            timeout_seconds: 30,
            max_request_bytes: 1_024,
            max_response_bytes: 2_048,
            max_output_bytes: 4_096,
            max_redirects: 0,
            max_concurrent: 1,
        },
        audit: CapabilityAuditPolicy {
            enabled: true,
            retain_last: 100,
        },
        http: (kind == CapabilityKind::Http).then(|| HttpScope {
            base_url: "https://example.test/api".to_owned(),
            methods: vec!["GET".to_owned(), "POST".to_owned()],
            path_prefixes: vec!["/api".to_owned()],
        }),
        git: (kind == CapabilityKind::Git).then(|| GitScope {
            remote_name: "origin".to_owned(),
            remote_url: "ssh://git@example.test/repo".to_owned(),
            operations: vec!["fetch".to_owned()],
            branches: vec!["main".to_owned()],
            refspecs: vec![],
            allow_tags: true,
            allow_force_push: false,
            allow_delete_refs: false,
        }),
        ssh: (kind == CapabilityKind::Ssh).then(|| SshScope {
            alias: "prod".to_owned(),
            host: "example.test".to_owned(),
            port: 22,
            user: "deploy".to_owned(),
            host_key: "ssh-ed25519 AAAA".to_owned(),
            allow_git: true,
            remote_commands: vec!["uptime".to_owned()],
            allow_upload: true,
            allow_download: false,
            upload_roots: vec!["out".to_owned()],
            download_roots: vec![],
            upload_remote_roots: vec!["/srv/in".to_owned()],
            download_remote_roots: vec![],
            allow_interactive_shell: false,
            local_forward_targets: vec![],
            remote_forward_targets: vec![],
        }),
        created_at: fixed_time(),
        updated_at: fixed_time(),
    }
}

fn assert_round_trip(record: &NativeRecord) {
    let encoded = encode_record(record).expect("encode valid record");
    let decoded = decode_record(record.kind(), &encoded).expect("decode valid record");
    assert_eq!(&decoded, record);
    assert_eq!(encode_record(&decoded).expect("re-encode"), encoded);
}

#[test]
fn golden_meta_bytes_cover_zero_and_fixed_offset_times() {
    let record = NativeRecord::Meta(Meta {
        goal: "g".to_owned(),
        summary: String::new(),
        active_plan: 9,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Fixed {
            seconds: 1,
            nanoseconds: 2,
            offset_seconds: -3,
        },
        format_version: 5,
        last_write_version: "v1".to_owned(),
    });
    let expected = [
        0, 0, 0, 1, b'g', // goal
        0, 0, 0, 0, // summary
        0, 0, 0, 0, 0, 0, 0, 9, // active plan
        0, // zero time
        1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 0xff, 0xff, 0xff, 0xfd, // fixed time
        0, 0, 0, 0, 0, 0, 0, 5, // format version
        0, 0, 0, 2, b'v', b'1',
    ];
    assert_eq!(encode_record(&record).expect("encode"), expected);
    assert_eq!(
        decode_record(RecordKind::Meta, &expected).expect("decode"),
        record
    );
}

#[test]
fn plan_golden_bytes_pin_enum_and_signed_order() {
    let record = NativeRecord::Plan(Plan {
        id: 1,
        title: "x".to_owned(),
        status: PlanStatus::Archived,
        milestone_id: 2,
        order: 3,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
    });
    let expected = [
        0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, b'x', 3, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0,
        3, 0, 0, 0,
    ];
    assert_eq!(encode_record(&record).expect("encode"), expected);
    assert_round_trip(&record);
}

#[test]
fn capability_round_trips_every_scope_and_option_shape() {
    for kind in [
        CapabilityKind::Http,
        CapabilityKind::Git,
        CapabilityKind::Ssh,
    ] {
        assert_round_trip(&NativeRecord::Capability(valid_capability(kind)));
    }
}

#[test]
fn record_and_enum_tags_are_stable_and_global_tags_are_reserved() {
    assert_eq!(RecordKind::Meta.wire_tag(), 1);
    assert_eq!(RecordKind::ProjectRef.wire_tag(), 11);
    assert_eq!(RecordKind::GlobalConfig.wire_tag(), 12);
    assert_eq!(RecordKind::GlobalBackup.wire_tag(), 13);
    assert_eq!(MemoryKind::Legacy.wire_tag(), 0);
    assert_eq!(MemoryKind::Summary.wire_tag(), 4);
    assert_eq!(PlanStatus::Active.wire_tag(), 1);
    assert_eq!(PlanStatus::Done.wire_tag(), 2);
    assert_eq!(PlanStatus::Archived.wire_tag(), 3);
    assert_eq!(TaskStatus::Todo.wire_tag(), 1);
    assert_eq!(TaskStatus::Doing.wire_tag(), 2);
    assert_eq!(TaskStatus::Done.wire_tag(), 3);
    assert_eq!(TaskStatus::Blocked.wire_tag(), 4);
    assert_eq!(NoteTarget::Project.wire_tag(), 1);
    assert_eq!(NoteTarget::Plan.wire_tag(), 2);
    assert_eq!(NoteTarget::Task.wire_tag(), 3);
    assert_eq!(MemoryKind::Decision.wire_tag(), 1);
    assert_eq!(MemoryKind::Blocker.wire_tag(), 2);
    assert_eq!(MemoryKind::Handoff.wire_tag(), 3);
    assert_eq!(MilestoneStatus::Open.wire_tag(), 1);
    assert_eq!(MilestoneStatus::Done.wire_tag(), 2);
    assert_eq!(IssueStatus::Open.wire_tag(), 1);
    assert_eq!(IssueStatus::Closed.wire_tag(), 2);
    assert_eq!(Severity::Low.wire_tag(), 1);
    assert_eq!(Severity::Medium.wire_tag(), 2);
    assert_eq!(Severity::High.wire_tag(), 3);
    assert_eq!(Severity::Critical.wire_tag(), 4);
    assert_eq!(CapabilityKind::Http.wire_tag(), 1);
    assert_eq!(CapabilityKind::Git.wire_tag(), 2);
    assert_eq!(CapabilityKind::Ssh.wire_tag(), 3);
}

#[test]
fn malformed_and_trailing_payloads_are_rejected() {
    let plan = NativeRecord::Plan(Plan {
        id: 1,
        title: "x".to_owned(),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
    });
    let encoded = encode_record(&plan).expect("encode");
    assert!(matches!(
        decode_record(RecordKind::Plan, &encoded[..encoded.len() - 1]),
        Err(CodecError::Truncated { .. })
    ));
    let mut trailing = encoded;
    trailing.push(0);
    assert_eq!(
        decode_record(RecordKind::Plan, &trailing),
        Err(CodecError::TrailingBytes(1))
    );
}

#[test]
fn invalid_utf8_enum_and_time_tags_are_rejected() {
    let mut invalid_utf8 = vec![0, 0, 0, 1, 0xff];
    invalid_utf8.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(
        decode_record(RecordKind::Meta, &invalid_utf8),
        Err(CodecError::InvalidUtf8)
    );

    let mut plan = encode_record(&NativeRecord::Plan(Plan {
        id: 1,
        title: String::new(),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
    }))
    .expect("encode");
    plan[12] = 99;
    assert!(matches!(
        decode_record(RecordKind::Plan, &plan),
        Err(CodecError::InvalidEnum { .. })
    ));
    plan[12] = PlanStatus::Active.wire_tag();
    plan[29] = 8;
    assert_eq!(
        decode_record(RecordKind::Plan, &plan),
        Err(CodecError::InvalidTimestampTag(8))
    );
}

#[test]
fn invalid_bool_option_and_declared_lengths_are_rejected() {
    let mut capability = encode_record(&NativeRecord::Capability(valid_capability(
        CapabilityKind::Http,
    )))
    .expect("encode");
    // id + version + revision + name length + name + kind + profile length + profile
    let enabled_offset = 8 + 8 + 8 + 4 + 5 + 1 + 4 + 11;
    capability[enabled_offset] = 2;
    assert_eq!(
        decode_record(RecordKind::Capability, &capability),
        Err(CodecError::InvalidBool(2))
    );

    let mut note = encode_record(&NativeRecord::Note(Note {
        id: 1,
        target: NoteTarget::Project,
        target_id: 0,
        kind: MemoryKind::Decision,
        body: "x".to_owned(),
        created_at: Timestamp::Zero,
    }))
    .expect("encode");
    note[18..22].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(
        decode_record(RecordKind::Note, &note),
        Err(CodecError::StringTooLarge { .. } | CodecError::Truncated { .. })
    ));

    // Scope option tag follows the capability audit policy.
    let mut option = encode_record(&NativeRecord::Capability(valid_capability(
        CapabilityKind::Http,
    )))
    .expect("encode");
    let pattern = [1, 0, 0, 0, 0, 0, 0, 0, 100, 1];
    let offset = option
        .windows(pattern.len())
        .position(|window| window == pattern)
        .expect("audit followed by HTTP option")
        + pattern.len()
        - 1;
    option[offset] = 3;
    assert_eq!(
        decode_record(RecordKind::Capability, &option),
        Err(CodecError::InvalidOption(3))
    );
}

#[test]
fn list_count_is_bounded_before_allocation() {
    let mut capability = encode_record(&NativeRecord::Capability(valid_capability(
        CapabilityKind::Http,
    )))
    .expect("encode");
    let base_url = b"https://example.test/api";
    let methods = capability
        .windows(base_url.len())
        .position(|window| window == base_url)
        .expect("HTTP base URL")
        + base_url.len();
    let declared = MAX_LIST_ITEMS + 1;
    capability.truncate(methods);
    capability.extend_from_slice(&u32::try_from(declared).unwrap().to_be_bytes());
    capability.resize(methods + 4 + declared * 4, 0);
    assert!(capability.len() < MAX_PAYLOAD_BYTES);
    assert_eq!(MAX_LIST_ITEMS, 1_000_000);
    assert_eq!(
        decode_record(RecordKind::Capability, &capability),
        Err(CodecError::ListTooLarge {
            actual: 1_000_001,
            maximum: 1_000_000,
        })
    );
}

#[test]
fn list_count_is_bounded_before_writer_iteration() {
    let mut capability = valid_capability(CapabilityKind::Http);
    capability.http.as_mut().unwrap().methods = vec![String::new(); MAX_LIST_ITEMS + 1];
    assert_eq!(
        encode_record(&NativeRecord::Capability(capability)),
        Err(CodecError::ListTooLarge {
            actual: 1_000_001,
            maximum: 1_000_000,
        })
    );
}

#[test]
fn go_encoder_golden_payloads_decode_and_reencode_exactly() {
    let fixtures = [
        (
            RecordKind::Meta,
            "0000000167000000017300000000000000010100000000000000010000000200000e100000000000000000050000000176",
        ),
        (
            RecordKind::Plan,
            "0000000000000002000000017001000000000000000300000000000000010100000000000000010000000200000e100100000000000000010000000200000e10",
        ),
        (
            RecordKind::Task,
            "0000000000000003000000000000000200000001740400000000000000040100000000000000010000000200000e100100000000000000010000000200000e10",
        ),
        (
            RecordKind::Note,
            "000000000000000403000000000000000301000000016e0100000000000000010000000200000e10",
        ),
        (
            RecordKind::Milestone,
            "0000000000000005000000016d020000000000000000060100000000000000010000000200000e100100000000000000010000000200000e10",
        ),
        (
            RecordKind::Issue,
            "000000000000000600000001690000000162020400000000000000030100000000000000010000000200000e100100000000000000010000000200000e10",
        ),
        (
            RecordKind::Commit,
            "000000000000000700000001610000000163000000000000000200000000000000030100000000000000010000000200000e10",
        ),
        (
            RecordKind::Capability,
            "000000000000000800000000000000010000000000000002000000016301000000017000000000000000003c0000abababababababababababababababababababababababababababababababab000000000000000100000000000000020000000000000003000000000000000400000000000000000000000000000001000000000000000001010000000968747470733a2f2f78000000010000000347455400000001000000012f0000000100000000000000010000000200000e10",
        ),
        (
            RecordKind::CapabilityAudit,
            "00000000000000090000000000000008000000017001000000036765740000000968747470733a2f2f7801000000046e6f6e6500000000000000010000000000000002000000000000000300000000000000040100000000000000010000000200000e10",
        ),
        (
            RecordKind::MemoryWriteback,
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f0000000000000001040000000000000000",
        ),
        (
            RecordKind::ProjectRef,
            "000000016e000000022f700100000000000000010000000200000e10",
        ),
    ];

    for (kind, golden) in fixtures {
        let payload = decode_hex(golden);
        // The Go encoder wrote payload schema 1. Decoding at that schema also
        // proves the schema-1 canonical round trip, because the decoder
        // re-encodes at the schema it was given.
        let record = decode_record_at_schema(kind, MIN_NATIVE_PAYLOAD_SCHEMA, &payload)
            .expect("decode Go golden payload");
        match &record {
            // Plan and Task gained a trailing hold reason at schema 2, so their
            // schema-1 payloads round-trip byte for byte only at schema 1.
            NativeRecord::Plan(plan) => {
                assert_eq!(plan.hold_reason, None);
                let mut upgraded = payload.clone();
                upgraded.push(0);
                assert_eq!(
                    encode_record(&record).expect("re-encode at schema 2"),
                    upgraded
                );
            }
            NativeRecord::Task(task) => {
                assert_eq!(task.hold_reason, None);
                let mut upgraded = payload.clone();
                upgraded.push(0);
                assert_eq!(
                    encode_record(&record).expect("re-encode at schema 2"),
                    upgraded
                );
            }
            _ => assert_eq!(
                encode_record(&record).expect("re-encode Go golden"),
                payload
            ),
        }
    }
}

#[test]
fn hold_reason_round_trips_and_pins_its_schema_2_bytes() {
    let record = NativeRecord::Plan(Plan {
        id: 1,
        title: "x".to_owned(),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: Some("waiting on review".to_owned()),
    });
    let mut expected = vec![
        0, 0, 0, 0, 0, 0, 0, 1, // id
        0, 0, 0, 1, b'x', // title
        1,    // active
        0, 0, 0, 0, 0, 0, 0, 0, // milestone
        0, 0, 0, 0, 0, 0, 0, 0, // order
        0, 0, // zero times
        1, 0, 0, 0, 17, // hold reason present, 17 bytes
    ];
    expected.extend_from_slice(b"waiting on review");
    assert_eq!(encode_record(&record).expect("encode"), expected);
    assert_round_trip(&record);

    let task = NativeRecord::Task(Task {
        id: 1,
        plan_id: 2,
        title: "t".to_owned(),
        status: TaskStatus::Blocked,
        order: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: Some("blocked upstream".to_owned()),
    });
    assert_round_trip(&task);
}

#[test]
fn unknown_payload_schemas_fail_closed_before_any_layout_is_assumed() {
    let record = NativeRecord::Plan(Plan {
        id: 1,
        title: "x".to_owned(),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
    });
    let payload = encode_record(&record).expect("encode");
    for schema in [0, NATIVE_PAYLOAD_SCHEMA + 1, u32::MAX] {
        assert_eq!(
            decode_record_at_schema(RecordKind::Plan, schema, &payload),
            Err(CodecError::UnsupportedPayloadSchema(schema))
        );
    }
}

#[test]
fn a_set_hold_reason_has_no_canonical_schema_1_form() {
    let mut payload = encode_record(&NativeRecord::Plan(Plan {
        id: 1,
        title: "x".to_owned(),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
    }))
    .expect("encode");
    // Strip the schema-2 `None` option tag to obtain the schema-1 payload.
    assert_eq!(payload.pop(), Some(0));
    let record = decode_record_at_schema(RecordKind::Plan, MIN_NATIVE_PAYLOAD_SCHEMA, &payload)
        .expect("schema 1 decode");
    let NativeRecord::Plan(plan) = record else {
        panic!("expected a plan");
    };
    assert_eq!(plan.hold_reason, None);
    assert!(matches!(
        decode_record(RecordKind::Plan, &payload),
        Err(CodecError::Truncated { .. })
    ));
}

fn decode_hex(input: &str) -> Vec<u8> {
    assert_eq!(input.len() % 2, 0);
    input
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0]);
            let low = hex_nibble(pair[1]);
            high << 4 | low
        })
        .collect()
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => panic!("invalid fixture hex"),
    }
}
