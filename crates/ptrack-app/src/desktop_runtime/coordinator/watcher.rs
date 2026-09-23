//! The per-workspace watchers: a debounced poll of the project database for
//! data changes, and a drain of runtime invalidations from the agent runtime.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use super::super::ports::DesktopWorkspace;
use super::super::support::lock;
use super::super::wire::DesktopEvent;
use super::super::{WORKSPACE_WATCH_DEBOUNCE, WORKSPACE_WATCH_INTERVAL};
use super::DesktopRuntime;

impl DesktopRuntime {
    pub(super) fn start_workspace_watcher(
        &self,
        generation: u64,
        database: PathBuf,
        workspace: Arc<dyn DesktopWorkspace>,
    ) {
        self.stop_workspace_watcher();
        let Some(sink) = self.event_sink.clone() else {
            return;
        };
        let file_sink = sink.clone();
        let (file_cancel, file_cancellation) = channel();
        let file_handle = thread::Builder::new()
            .name("ptrack-workspace-watch".to_owned())
            .spawn(move || {
                watch_workspace_data(
                    &file_cancellation,
                    &database,
                    WORKSPACE_WATCH_INTERVAL,
                    WORKSPACE_WATCH_DEBOUNCE,
                    || file_sink.emit(DesktopEvent::WorkspaceDataChanged(generation)),
                );
            });
        let (runtime_cancel, runtime_cancellation) = channel();
        let runtime_handle = thread::Builder::new()
            .name("ptrack-runtime-watch".to_owned())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) =
                    runtime_cancellation.recv_timeout(Duration::from_millis(100))
                {
                    if workspace.drain_runtime_invalidations().unwrap_or(false) {
                        sink.emit(DesktopEvent::WorkspaceRuntimeChanged(generation));
                    }
                }
            });
        if let (Ok(file_handle), Ok(runtime_handle)) = (file_handle, runtime_handle) {
            *lock(&self.watcher) = Some(WorkspaceWatcher {
                cancellations: vec![file_cancel, runtime_cancel],
                handles: vec![file_handle, runtime_handle],
            });
        }
    }

    pub(super) fn stop_workspace_watcher(&self) {
        let watcher = lock(&self.watcher).take();
        if let Some(watcher) = watcher {
            watcher.stop();
        }
    }
}

pub(super) struct WorkspaceWatcher {
    cancellations: Vec<Sender<()>>,
    handles: Vec<JoinHandle<()>>,
}

impl WorkspaceWatcher {
    pub(super) fn stop(self) {
        for cancel in self.cancellations {
            let _ = cancel.send(());
        }
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct WorkspaceFileState {
    exists: bool,
    size: u64,
    modified: Option<SystemTime>,
}

fn workspace_file_state(path: &Path) -> WorkspaceFileState {
    fs::metadata(path).map_or_else(
        |_| WorkspaceFileState::default(),
        |metadata| WorkspaceFileState {
            exists: true,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        },
    )
}

pub(crate) fn watch_workspace_data(
    cancellation: &Receiver<()>,
    database: &Path,
    interval: Duration,
    debounce: Duration,
    mut emit: impl FnMut(),
) {
    let mut previous = workspace_file_state(database);
    let mut next_poll = Instant::now() + interval;
    let mut pending = None;
    loop {
        let deadline = pending.map_or(next_poll, |pending: Instant| pending.min(next_poll));
        let wait = deadline.saturating_duration_since(Instant::now());
        match cancellation.recv_timeout(wait) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if now >= next_poll {
            next_poll = now + interval;
            let current = workspace_file_state(database);
            if current != previous {
                previous = current;
                pending = Some(now + debounce);
            }
        }
        if pending.is_some_and(|deadline| now >= deadline) {
            pending = None;
            emit();
        }
    }
}
