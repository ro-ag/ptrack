//! The terminal-window assignment map.
//!
//! One entry per popped-out terminal window: its label and the tab it is
//! showing — the tab's sessions in pane order and its serialized shape. The
//! map is in-memory, lives for exactly one run, and is **never persisted** — a
//! crashed or restarted app opens with no terminal windows, so nothing can
//! resurrect a window pointing at a dead session.
//!
//! Assignments are fenced by the generation of the open workspace. Switching or
//! closing a project changes the fence, which drops every assignment and hands
//! the shell the labels whose windows must close.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{AppError, AppResult};

/// Minted labels are `terminal-<n>`; `src-tauri/capabilities/main-window.json`
/// admits `terminal-*`.
pub const TERMINAL_WINDOW_PREFIX: &str = "terminal-";
/// Bounded so a runaway caller cannot open windows without end.
const TERMINAL_WINDOW_LIMIT: usize = 16;

/// The tab one terminal window shows: its sessions in pane order and the
/// serialized tab shape the frontend re-hydrates a split tree from. The shape
/// is opaque here — the map guards session ownership, not layout validity.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalWindowTab {
    pub sessions: Vec<String>,
    pub shape: Value,
}

/// One freshly minted window assignment and the labels of the windows whose
/// assignments the same call expired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedTerminalWindow {
    pub label: String,
    /// Windows of a superseded workspace. The caller closes them: nothing
    /// else will, because their assignments are already gone.
    pub expired: Vec<String>,
}

/// Window label to the tab it owns, fenced by one workspace generation.
#[derive(Debug, Default)]
pub struct TerminalWindows {
    minted: u64,
    fence: Option<u64>,
    assigned: BTreeMap<String, TerminalWindowTab>,
}

impl TerminalWindows {
    /// Mints the next label and records the assignment. `fence` is the open
    /// workspace's generation, and `None` while no project is open.
    ///
    /// Labels are monotonic for the whole run: a closed window's label is never
    /// reused, so a late message naming it can never reach a different window.
    ///
    /// The map must never carry an assignment from a superseded workspace
    /// into a new one, so an open under a changed fence expires the old
    /// assignments first and reports their labels. The shell's sweep after the
    /// fence-changing command usually got there first, but that command and
    /// this open can run concurrently, and a label dropped here would leave its
    /// window open with nothing behind it.
    ///
    /// # Errors
    /// Returns an error with no project open, without at least one session,
    /// when any session is already shown by a window, or at the window limit;
    /// the error carries no labels, so a refused open expires nothing.
    pub fn open(
        &mut self,
        fence: Option<u64>,
        tab: TerminalWindowTab,
    ) -> AppResult<OpenedTerminalWindow> {
        let fence =
            fence.ok_or_else(|| AppError::Message("no project workspace is open".into()))?;
        self.check_sessions_under(fence, &tab.sessions)?;
        let expired = self.expire(Some(fence));
        if self.assigned.len() >= TERMINAL_WINDOW_LIMIT {
            return Err(AppError::Message(
                "no more terminal windows can be opened".into(),
            ));
        }
        self.minted = self.minted.saturating_add(1);
        let label = format!("{TERMINAL_WINDOW_PREFIX}{}", self.minted);
        self.assigned.insert(label.clone(), tab);
        Ok(OpenedTerminalWindow { label, expired })
    }

    /// Session checks as they will stand once `fence` is current: under a
    /// changed fence every existing assignment is about to expire, so none of
    /// them can own the requested sessions.
    fn check_sessions_under(&self, fence: u64, sessions: &[String]) -> AppResult<()> {
        if self.fence == Some(fence) {
            self.check_sessions(None, sessions)
        } else {
            Self::check_session_shape(sessions)
        }
    }

    /// The tab a window owns. An unknown label reads as `None` rather than an
    /// error, so a stale window learns it owns nothing and closes cleanly.
    #[must_use]
    pub fn tab(&self, label: &str) -> Option<&TerminalWindowTab> {
        self.assigned.get(label)
    }

    /// Replaces one window's tab — a split created, closed, or resized inside
    /// it. Ownership checks skip the window's own current sessions: a tab may
    /// keep what it had, only another window's sessions are off limits.
    ///
    /// # Errors
    /// Returns an error for an unknown label, without at least one session, or
    /// when any session belongs to a different window. A failed replacement
    /// leaves the assignment untouched.
    pub fn set_tab(&mut self, label: &str, tab: TerminalWindowTab) -> AppResult<()> {
        if !self.assigned.contains_key(label) {
            return Err(AppError::Message(
                "no terminal window has that label".into(),
            ));
        }
        self.check_sessions(Some(label), &tab.sessions)?;
        self.assigned.insert(label.to_owned(), tab);
        Ok(())
    }

    /// Clears one assignment and reports the tab it freed, so the caller knows
    /// what to take back. An unknown label frees nothing.
    pub fn close(&mut self, label: &str) -> Option<TerminalWindowTab> {
        self.assigned.remove(label)
    }

    /// Drops every assignment made under a superseded fence and reports the
    /// labels whose windows must close. An unchanged fence closes nothing.
    pub fn expire(&mut self, fence: Option<u64>) -> Vec<String> {
        if self.fence == fence {
            return Vec::new();
        }
        self.fence = fence;
        self.drain()
    }

    /// Clears every assignment and reports the labels, for app shutdown.
    pub fn drain(&mut self) -> Vec<String> {
        std::mem::take(&mut self.assigned).into_keys().collect()
    }

    /// One non-empty session set, unique inside the tab and against every
    /// window except `skip` — a session rendered twice would be two writers on
    /// one lease.
    fn check_sessions(&self, skip: Option<&str>, sessions: &[String]) -> AppResult<()> {
        Self::check_session_shape(sessions)?;
        let owned_elsewhere = self
            .assigned
            .iter()
            .filter(|(label, _)| Some(label.as_str()) != skip)
            .any(|(_, owned)| owned.sessions.iter().any(|held| sessions.contains(held)));
        if owned_elsewhere {
            return Err(AppError::Message(
                "terminal is already in a terminal window".into(),
            ));
        }
        Ok(())
    }

    /// One non-empty session set with no session twice.
    fn check_session_shape(sessions: &[String]) -> AppResult<()> {
        if sessions.is_empty() || sessions.iter().any(String::is_empty) {
            return Err(AppError::Message("terminal session is required".into()));
        }
        let duplicate_inside = sessions
            .iter()
            .enumerate()
            .any(|(index, session)| sessions[..index].contains(session));
        if duplicate_inside {
            return Err(AppError::Message(
                "terminal is already in a terminal window".into(),
            ));
        }
        Ok(())
    }
}
