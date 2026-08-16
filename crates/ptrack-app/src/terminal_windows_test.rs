use crate::terminal_windows::TerminalWindows;

#[test]
fn labels_are_monotonic_and_an_assignment_reports_the_session_it_owns() {
    let mut windows = TerminalWindows::default();
    assert_eq!(windows.open(Some(1), "session-a").unwrap(), "terminal-1");
    assert_eq!(windows.open(Some(1), "session-b").unwrap(), "terminal-2");
    assert_eq!(windows.session("terminal-1").as_deref(), Some("session-a"));
    // An unknown label is never an error: a stale window closes cleanly.
    assert_eq!(windows.session("terminal-9"), None);
    assert_eq!(windows.session("main"), None);

    assert_eq!(windows.close("terminal-1").as_deref(), Some("session-a"));
    assert_eq!(windows.close("terminal-1"), None);
    assert_eq!(windows.session("terminal-1"), None);
    // A freed label is never reused, so a late message cannot reach a
    // different window.
    assert_eq!(windows.open(Some(1), "session-a").unwrap(), "terminal-3");
}

#[test]
fn opening_requires_an_open_workspace_a_session_and_room() {
    let mut windows = TerminalWindows::default();
    assert_eq!(
        windows.open(None, "session-a").unwrap_err().to_string(),
        "no project workspace is open"
    );
    assert_eq!(
        windows.open(Some(1), "").unwrap_err().to_string(),
        "terminal session is required"
    );
    windows.open(Some(1), "session-a").unwrap();
    assert_eq!(
        windows.open(Some(1), "session-a").unwrap_err().to_string(),
        "terminal is already in a terminal window"
    );
    for index in 1..16 {
        windows.open(Some(1), &format!("session-{index}")).unwrap();
    }
    assert_eq!(
        windows
            .open(Some(1), "session-last")
            .unwrap_err()
            .to_string(),
        "no more terminal windows can be opened"
    );
}

#[test]
fn a_superseded_fence_expires_every_assignment_exactly_once() {
    let mut windows = TerminalWindows::default();
    windows.open(Some(1), "session-a").unwrap();
    windows.open(Some(1), "session-b").unwrap();
    // The same generation closes nothing: this runs after every command.
    assert!(windows.expire(Some(1)).is_empty());

    // Closing the project leaves no fence at all.
    assert_eq!(windows.expire(None), ["terminal-1", "terminal-2"]);
    assert!(windows.expire(None).is_empty());
    assert_eq!(windows.session("terminal-1"), None);

    // A switched project takes its windows with it on the next open.
    windows.open(Some(2), "session-c").unwrap();
    assert_eq!(windows.expire(Some(3)), ["terminal-3"]);
    assert!(windows.drain().is_empty());
}

#[test]
fn a_drain_reports_every_label_and_leaves_the_fence_alone() {
    let mut windows = TerminalWindows::default();
    windows.open(Some(1), "session-a").unwrap();
    windows.open(Some(1), "session-b").unwrap();
    assert_eq!(windows.drain(), ["terminal-1", "terminal-2"]);
    // The fence survives, so a drain is not mistaken for a project switch.
    assert!(windows.expire(Some(1)).is_empty());
}
