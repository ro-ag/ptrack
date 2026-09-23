use serde_json::json;

use crate::terminal_windows::{TerminalWindowTab, TerminalWindows};

fn tab(sessions: &[&str]) -> TerminalWindowTab {
    TerminalWindowTab {
        sessions: sessions.iter().map(ToString::to_string).collect(),
        shape: json!({ "id": "tab-1" }),
    }
}

#[test]
fn labels_are_monotonic_and_an_assignment_reports_the_tab_it_owns() {
    let mut windows = TerminalWindows::default();
    assert_eq!(
        windows
            .open(Some(1), tab(&["session-a", "session-b"]))
            .unwrap()
            .label,
        "terminal-1"
    );
    assert_eq!(
        windows.open(Some(1), tab(&["session-c"])).unwrap().label,
        "terminal-2"
    );
    let owned = windows.tab("terminal-1").unwrap();
    assert_eq!(owned.sessions, ["session-a", "session-b"]);
    assert_eq!(owned.shape, json!({ "id": "tab-1" }));
    // An unknown label is never an error: a stale window closes cleanly.
    assert!(windows.tab("terminal-9").is_none());
    assert!(windows.tab("main").is_none());

    let closed = windows.close("terminal-1").unwrap();
    assert_eq!(closed.sessions, ["session-a", "session-b"]);
    assert!(windows.close("terminal-1").is_none());
    assert!(windows.tab("terminal-1").is_none());
    // A freed label is never reused, so a late message cannot reach a
    // different window.
    assert_eq!(
        windows.open(Some(1), tab(&["session-a"])).unwrap().label,
        "terminal-3"
    );
}

#[test]
fn opening_requires_an_open_workspace_sessions_and_room() {
    let mut windows = TerminalWindows::default();
    assert_eq!(
        windows
            .open(None, tab(&["session-a"]))
            .unwrap_err()
            .to_string(),
        "no project workspace is open"
    );
    assert_eq!(
        windows.open(Some(1), tab(&[])).unwrap_err().to_string(),
        "terminal session is required"
    );
    assert_eq!(
        windows
            .open(Some(1), tab(&["session-a", ""]))
            .unwrap_err()
            .to_string(),
        "terminal session is required"
    );
    // The same session twice inside one tab can never be rendered twice.
    assert_eq!(
        windows
            .open(Some(1), tab(&["session-a", "session-a"]))
            .unwrap_err()
            .to_string(),
        "terminal is already in a terminal window"
    );
    windows
        .open(Some(1), tab(&["session-a", "session-b"]))
        .unwrap();
    // A session one window owns cannot appear in another window's tab.
    assert_eq!(
        windows
            .open(Some(1), tab(&["session-c", "session-b"]))
            .unwrap_err()
            .to_string(),
        "terminal is already in a terminal window"
    );
    for index in 1..16 {
        windows
            .open(Some(1), tab(&[&format!("extra-{index}")]))
            .unwrap();
    }
    assert_eq!(
        windows
            .open(Some(1), tab(&["session-last"]))
            .unwrap_err()
            .to_string(),
        "no more terminal windows can be opened"
    );
}

#[test]
fn set_tab_replaces_the_shape_and_sessions_of_one_window_only() {
    let mut windows = TerminalWindows::default();
    windows.open(Some(1), tab(&["session-a"])).unwrap();
    windows.open(Some(1), tab(&["session-b"])).unwrap();

    // A split inside the window adds a session and a new shape.
    let replaced = TerminalWindowTab {
        sessions: vec!["session-a".into(), "session-c".into()],
        shape: json!({ "id": "tab-1", "split": true }),
    };
    windows.set_tab("terminal-1", replaced).unwrap();
    let owned = windows.tab("terminal-1").unwrap();
    assert_eq!(owned.sessions, ["session-a", "session-c"]);
    assert_eq!(owned.shape, json!({ "id": "tab-1", "split": true }));

    // The window's own previous sessions are not "another window's".
    assert_eq!(
        windows
            .set_tab("terminal-1", tab(&["session-b"]))
            .unwrap_err()
            .to_string(),
        "terminal is already in a terminal window"
    );
    assert_eq!(
        windows
            .set_tab("terminal-9", tab(&["session-z"]))
            .unwrap_err()
            .to_string(),
        "no terminal window has that label"
    );
    assert_eq!(
        windows
            .set_tab("terminal-1", tab(&[]))
            .unwrap_err()
            .to_string(),
        "terminal session is required"
    );
    // A failed replacement leaves the assignment untouched.
    assert_eq!(
        windows.tab("terminal-1").unwrap().sessions,
        ["session-a", "session-c"]
    );
}

#[test]
fn a_superseded_fence_expires_every_assignment_exactly_once() {
    let mut windows = TerminalWindows::default();
    windows.open(Some(1), tab(&["session-a"])).unwrap();
    windows
        .open(Some(1), tab(&["session-b", "session-c"]))
        .unwrap();
    // The same generation closes nothing: this runs after every command.
    assert!(windows.expire(Some(1)).is_empty());

    // Closing the project leaves no fence at all.
    assert_eq!(windows.expire(None), ["terminal-1", "terminal-2"]);
    assert!(windows.expire(None).is_empty());
    assert!(windows.tab("terminal-1").is_none());

    // A switched project takes its windows with it on the next open.
    assert!(
        windows
            .open(Some(2), tab(&["session-d"]))
            .unwrap()
            .expired
            .is_empty()
    );
    assert_eq!(windows.expire(Some(3)), ["terminal-3"]);
    assert!(windows.drain().is_empty());
}

#[test]
fn a_drain_reports_every_label_and_leaves_the_fence_alone() {
    let mut windows = TerminalWindows::default();
    windows.open(Some(1), tab(&["session-a"])).unwrap();
    windows.open(Some(1), tab(&["session-b"])).unwrap();
    assert_eq!(windows.drain(), ["terminal-1", "terminal-2"]);
    // The fence survives, so a drain is not mistaken for a project switch.
    assert!(windows.expire(Some(1)).is_empty());
}

/// An open that lands before the shell's sweep for a project switch must hand
/// back the windows it expired: the sweep will find nothing left, and a label
/// dropped here would leave a window standing with no assignment behind it.
#[test]
fn an_open_under_a_new_fence_reports_the_windows_it_expired() {
    let mut windows = TerminalWindows::default();
    windows.open(Some(1), tab(&["session-a"])).unwrap();
    windows.open(Some(1), tab(&["session-b"])).unwrap();

    // The new workspace may reuse nothing of the old one's ownership: the
    // expiring windows can never block its sessions.
    let opened = windows.open(Some(2), tab(&["session-a"])).unwrap();
    assert_eq!(opened.label, "terminal-3");
    assert_eq!(opened.expired, ["terminal-1", "terminal-2"]);
    assert!(windows.tab("terminal-1").is_none());
    assert!(windows.expire(Some(2)).is_empty());

    // A refused open expires nothing.
    assert!(windows.open(Some(3), tab(&[])).is_err());
    assert_eq!(windows.tab("terminal-3").unwrap().sessions, ["session-a"]);
}
