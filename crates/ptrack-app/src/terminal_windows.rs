//! The terminal-window assignment map.
//!
//! One entry per popped-out terminal window: its label and the session it is
//! showing. The map is in-memory, lives for exactly one run, and is **never
//! persisted** — a crashed or restarted app opens with no terminal windows, so
//! nothing can resurrect a window pointing at a dead session.
//!
//! Assignments are fenced by the generation of the open workspace. Switching or
//! closing a project changes the fence, which drops every assignment and hands
//! the shell the labels whose windows must close.

use std::collections::BTreeMap;

use crate::{AppError, AppResult};

/// Minted labels are `terminal-<n>`; `src-tauri/capabilities/main-window.json`
/// admits `terminal-*`.
pub const TERMINAL_WINDOW_PREFIX: &str = "terminal-";
/// Bounded so a runaway caller cannot open windows without end.
const TERMINAL_WINDOW_LIMIT: usize = 16;

/// Window label to the session it owns, fenced by one workspace generation.
#[derive(Debug, Default)]
pub struct TerminalWindows {
    minted: u64,
    fence: Option<u64>,
    assigned: BTreeMap<String, String>,
}

impl TerminalWindows {
    /// Mints the next label and records the assignment. `fence` is the open
    /// workspace's generation, and `None` while no project is open.
    ///
    /// Labels are monotonic for the whole run: a closed window's label is never
    /// reused, so a late message naming it can never reach a different window.
    ///
    /// # Errors
    /// Returns an error with no project open, without a session, when the
    /// session is already shown by another window, or at the window limit.
    pub fn open(&mut self, fence: Option<u64>, session_id: &str) -> AppResult<String> {
        let fence =
            fence.ok_or_else(|| AppError::Message("no project workspace is open".into()))?;
        if session_id.is_empty() {
            return Err(AppError::Message("terminal session is required".into()));
        }
        // The map must never carry an assignment from a superseded workspace
        // into a new one. The labels are dropped rather than returned because
        // the shell sweeps `expire` after every command, and the command that
        // changed the fence is itself one — so by the time an open runs, this
        // has nothing left to find.
        drop(self.expire(Some(fence)));
        if self.assigned.values().any(|owned| owned == session_id) {
            return Err(AppError::Message(
                "terminal is already in a terminal window".into(),
            ));
        }
        if self.assigned.len() >= TERMINAL_WINDOW_LIMIT {
            return Err(AppError::Message(
                "no more terminal windows can be opened".into(),
            ));
        }
        self.minted = self.minted.saturating_add(1);
        let label = format!("{TERMINAL_WINDOW_PREFIX}{}", self.minted);
        self.assigned.insert(label.clone(), session_id.to_owned());
        Ok(label)
    }

    /// The session a window owns. An unknown label reads as `None` rather than
    /// an error, so a stale window learns it owns nothing and closes cleanly.
    #[must_use]
    pub fn session(&self, label: &str) -> Option<String> {
        self.assigned.get(label).cloned()
    }

    /// Clears one assignment and reports the session it freed, so the caller
    /// knows what to take back. An unknown label frees nothing.
    pub fn close(&mut self, label: &str) -> Option<String> {
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
}
