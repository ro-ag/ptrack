use super::process_alive;

#[test]
fn process_liveness_is_only_a_positive_pid_hint() {
    assert!(!process_alive(0));
    assert!(!process_alive(-1));
    assert!(process_alive(i32::try_from(std::process::id()).unwrap()));
}
