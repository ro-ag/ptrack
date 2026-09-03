use crate::parse::{Preflight, parse_u64, preflight};

#[test]
fn cobra_argument_errors_are_owned_and_stable() {
    let error =
        preflight(vec!["ptrack".into(), "context".into(), "extra".into()]).expect_err("extra arg");
    assert_eq!(
        error.to_string(),
        "unknown command \"extra\" for \"ptrack context\""
    );
    let error =
        preflight(vec!["ptrack".into(), "task".into(), "show".into()]).expect_err("missing arg");
    assert_eq!(error.to_string(), "accepts 1 arg(s), received 0");
    let error = parse_u64("18446744073709551616").expect_err("overflow");
    assert_eq!(
        error.to_string(),
        "invalid id \"18446744073709551616\": strconv.ParseUint: parsing \"18446744073709551616\": value out of range"
    );
}

#[test]
fn aliases_and_cobra_group_fallbacks_are_preserved() {
    let result = preflight(vec![
        "ptrack".into(),
        "ms".into(),
        "list".into(),
        "--json".into(),
    ])
    .expect("alias");
    assert!(matches!(result, Preflight::Run { path, .. } if path == ["milestone", "list"]));
    let result =
        preflight(vec!["ptrack".into(), "task".into(), "nope".into()]).expect("group renders help");
    assert_eq!(result, Preflight::Help(vec!["task".to_owned()]));
    let result = preflight(vec!["ptrack".into(), "goal".into(), "nope".into()])
        .expect("goal defaults to show");
    assert_eq!(result, Preflight::GroupDefault(vec!["goal".to_owned()]));
}

#[test]
fn plan_lifecycle_leaves_validate_flags_and_arg_counts() {
    let error =
        preflight(vec!["ptrack".into(), "plan".into(), "delete".into()]).expect_err("missing id");
    assert_eq!(error.to_string(), "accepts 1 arg(s), received 0");
    let result = preflight(vec![
        "ptrack".into(),
        "plan".into(),
        "delete".into(),
        "3".into(),
        "--force".into(),
    ])
    .expect("delete parses");
    assert!(matches!(result, Preflight::Run { path, .. } if path == ["plan", "delete"]));
    let result = preflight(vec![
        "ptrack".into(),
        "plan".into(),
        "move".into(),
        "3".into(),
        "--to".into(),
        "beta".into(),
    ])
    .expect("move parses");
    assert!(matches!(result, Preflight::Run { path, .. } if path == ["plan", "move"]));
    let error = preflight(vec![
        "ptrack".into(),
        "plan".into(),
        "move".into(),
        "3".into(),
        "--bogus".into(),
        "x".into(),
    ])
    .expect_err("unknown flag");
    assert_eq!(error.to_string(), "unknown flag: --bogus");
    let result = preflight(vec![
        "ptrack".into(),
        "plan".into(),
        "copy".into(),
        "3".into(),
        "--to".into(),
        "beta".into(),
        "--as".into(),
        "New".into(),
    ])
    .expect("copy parses");
    assert!(matches!(result, Preflight::Run { path, .. } if path == ["plan", "copy"]));
}

#[test]
fn agent_leaves_validate_json_and_run_id() {
    let result = preflight(vec![
        "ptrack".into(),
        "agent".into(),
        "list".into(),
        "--json".into(),
    ])
    .unwrap();
    assert!(matches!(result, Preflight::Run { path, .. } if path == ["agent", "list"]));
    let error = preflight(vec!["ptrack".into(), "agent".into(), "show".into()]).unwrap_err();
    assert_eq!(error.to_string(), "accepts 1 arg(s), received 0");
}
