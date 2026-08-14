#[test]
fn cap_055_through_076_execute_in_contract_order() {
    let checks: [fn(); 3] = [cap_055_062, cap_063_068, cap_069_076];
    for check in checks {
        check();
    }
}

#[test]
fn capability_static_contract_helpers_execute_in_contract_order() {
    let checks: [fn(); 5] = [
        super::broker_test::assert_cap_037_through_044_broker_contract,
        super::server_test::assert_cap_045_through_054_server_contract,
        super::ssh_test::assert_cap_077_through_084_ssh_contract,
        super::mcp_test::assert_cap_085_through_087_mcp_contract,
        super::diagnostics_test::assert_cap_089_090_and_092_diagnostic_contract,
    ];
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
