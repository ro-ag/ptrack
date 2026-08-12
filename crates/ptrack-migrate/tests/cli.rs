use std::process::Command;

#[test]
fn only_explicit_absolute_inspection_is_accepted() {
    for arguments in [
        Vec::<&str>::new(),
        vec!["inspect"],
        vec!["inspect", "--bundle", "relative.bundle"],
        vec!["import", "--bundle", "/tmp/anything"],
        vec![
            "import",
            "--bundle",
            "/tmp/anything",
            "--destination",
            "/tmp/anything.redb",
            "--accept-one-way",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ptrack-migrate"))
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).starts_with("ptrack-migrate:"));
        assert!(output.stdout.is_empty());
    }
}
