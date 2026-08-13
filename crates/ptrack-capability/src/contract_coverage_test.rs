#[test]
fn cap_055_through_076_execute_in_contract_order() {
    let checks: [fn(); 3] = [cap_055_062, cap_063_068, cap_069_076];
    for check in checks {
        check();
    }
}

fn cap_055_062() {
    super::audit_test::assert_cap_055_through_062_audit_contract();
}

fn cap_063_068() {
    super::http_test::assert_cap_063_through_068_http_contract();
}

fn cap_069_076() {
    super::git_test::assert_cap_069_through_076_git_contract();
}
