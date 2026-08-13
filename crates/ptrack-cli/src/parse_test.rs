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
