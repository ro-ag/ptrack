#![forbid(unsafe_code)]

mod notification_runtime;
#[cfg(test)]
mod notification_runtime_test;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ptrack_app::window_state::{
    DisplayV1, MAIN_WINDOW_LABEL, RectV1, WindowStateV1, captured, logical_rect, physical_rect,
    save_window_state, saved_placement,
};
use ptrack_app::{
    AppError, DesktopCommandRequest, DesktopEvent, DesktopEventSink, DesktopRuntime,
    RoutedApplication, ShutdownOutcome, production_desktop_runtime_for_startup,
    resolve_global_home, resolved_startup_project, scope_request_to_window,
};
use ptrack_desktop::{
    DesktopPlatform, MenuDispatch, MenuEntrySpec, MenuRole, menu_dispatch, menu_spec, window_spec,
};
use tauri::menu::{Menu, MenuBuilder, MenuItem, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Runtime, WindowEvent};
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_opener::OpenerExt as _;

use notification_runtime::{NativeNotificationController, disabled_notification_patch};

struct TauriEventSink {
    app: AppHandle,
    notifications: Arc<NativeNotificationController>,
}

impl DesktopEventSink for TauriEventSink {
    fn emit(&self, event: DesktopEvent) {
        let refresh_notifications = matches!(
            event,
            DesktopEvent::WorkspaceRuntimeChanged(_) | DesktopEvent::WorkspaceDataChanged(_)
        );
        let result = match event {
            DesktopEvent::WorkspaceRuntimeChanged(generation) => {
                self.app.emit("workspace:runtime-changed", generation)
            }
            DesktopEvent::WorkspaceDataChanged(generation) => {
                self.app.emit("workspace:data-changed", generation)
            }
            DesktopEvent::UpdateStateChanged(state) => self.app.emit("update:state-changed", state),
            DesktopEvent::TerminalStatus(status) => self.app.emit("terminal:status", status),
            DesktopEvent::TerminalExit(exit) => self.app.emit("terminal:exit", exit),
            // Broadcast: the dock and every terminal window show the same
            // project scratchpad and each re-reads it on its own.
            DesktopEvent::ScratchpadChanged(change) => self.app.emit("scratchpad:changed", change),
        };
        let _ = result;
        if refresh_notifications {
            self.notifications.refresh(&self.app);
        }
    }
}

/// Converts an unstructured error message to the frontend's expected shape.
fn bridge_message(message: &str) -> serde_json::Value {
    serde_json::Value::String(message.to_owned())
}

#[tauri::command]
async fn gui_invoke(
    runtime: tauri::State<'_, Arc<DesktopRuntime>>,
    app: AppHandle,
    window: tauri::WebviewWindow,
    request: DesktopCommandRequest,
) -> Result<serde_json::Value, serde_json::Value> {
    // Scoped by the window that sent it, never by anything in the payload: a
    // terminal window reaches only its own commands and its own assignment.
    let request =
        scope_request_to_window(window.label(), request).map_err(serde_json::Value::from)?;
    let runtime = Arc::clone(runtime.inner());
    let notifications = Arc::clone(app.state::<Arc<NativeNotificationController>>().inner());
    let shell_command = request.method == "InstallShellCommand";
    // Preference reads and writes resync native chrome after a reset.
    let appearance = matches!(
        request.method.as_str(),
        "GetPreferences" | "SetPreferences" | "ResetPreferences"
    );
    let terminal_window = request.method == "OpenTerminalWindow";
    tauri::async_runtime::spawn_blocking(move || {
        let _dialog_lease = if shell_command {
            Some(
                runtime
                    .begin_native_action()
                    .map_err(serde_json::Value::from)?,
            )
        } else {
            None
        };
        let request_permission = request.method == "SetPreferences";
        let result = if appearance {
            notifications.with_configuration(|| {
                let mut result = runtime.invoke(request).map_err(serde_json::Value::from)?;
                apply_theme(&app, &result);
                if notifications.configure(&app, &result, request_permission) {
                    result = runtime
                        .invoke(DesktopCommandRequest {
                            method: "SetPreferences".to_owned(),
                            arguments: vec![disabled_notification_patch()],
                        })
                        .map_err(serde_json::Value::from)?;
                    let _ = notifications.configure(&app, &result, false);
                    apply_theme(&app, &result);
                }
                notifications.refresh(&app);
                Ok::<_, serde_json::Value>(result)
            })?
        } else {
            runtime.invoke(request).map_err(serde_json::Value::from)?
        };
        // Switching projects invalidates their terminal-window assignments.
        close_windows(&app, runtime.expire_terminal_windows());
        if terminal_window {
            let label = result["label"].as_str().unwrap_or_default().to_owned();
            if let Err(error) = build_terminal_window(&app, &label) {
                // A failed pop-out must never leave a session with no owner:
                // the assignment is released so the main window keeps it.
                runtime.close_terminal_window(&label);
                return Err(bridge_message(&error));
            }
        }
        if shell_command {
            let message = result.as_str().ok_or_else(|| {
                bridge_message("shell command installation returned an invalid result")
            })?;
            app.dialog()
                .message(message)
                .title("Shell Command")
                .blocking_show();
            Ok(serde_json::Value::Null)
        } else {
            Ok(result)
        }
    })
    .await
    .map_err(|error| bridge_message(&error.to_string()))?
}

/// Maps the stored appearance preference to a native theme; `None` follows the OS.
fn preferred_theme(preferences: &serde_json::Value) -> Option<tauri::Theme> {
    match preferences["appearance"]["theme"].as_str() {
        Some("dark") => Some(tauri::Theme::Dark),
        Some("light") => Some(tauri::Theme::Light),
        _ => None,
    }
}

/// Repaints every native window to match the app theme. On macOS, `set_theme`
/// updates the titlebar, menu bar, and dialogs; this is best effort.
fn apply_theme<R: Runtime>(app: &AppHandle<R>, preferences: &serde_json::Value) {
    let theme = preferred_theme(preferences);
    for window in app.webview_windows().values() {
        let _ = window.set_theme(theme);
    }
}

/// A wedged main thread must not hang the command that asked for the window.
const TERMINAL_WINDOW_BUILD_TIMEOUT: Duration = Duration::from_secs(10);
/// A popped-out terminal must not read as a second project workspace.
const TERMINAL_WINDOW_TITLE: &str = "p-track Terminal";

/// Builds a terminal window on the main thread and waits for the result.
///
/// Building synchronously deadlocks on Windows. Recheck the assignment after
/// the queued build because it can finish after its caller timed out and needs
/// cleanup if the assignment has expired.
fn build_terminal_window(app: &AppHandle, label: &str) -> Result<(), String> {
    let handle = app.clone();
    let owned = label.to_owned();
    let (sender, receiver) = channel();
    app.run_on_main_thread(move || {
        let built = terminal_window(&handle, &owned);
        if built.is_ok() && !terminal_window_assigned(&handle, &owned) {
            destroy_window(&handle, &owned);
        }
        let _ = sender.send(built);
    })
    .map_err(|error| error.to_string())?;
    match receiver.recv_timeout(TERMINAL_WINDOW_BUILD_TIMEOUT) {
        Ok(built) => built,
        Err(error) => {
            let handle = app.clone();
            let owned = label.to_owned();
            let _ = app.run_on_main_thread(move || destroy_window(&handle, &owned));
            Err(error.to_string())
        }
    }
}

fn terminal_window_assigned<R: Runtime>(app: &AppHandle<R>, label: &str) -> bool {
    app.try_state::<Arc<DesktopRuntime>>()
        .is_some_and(|runtime| runtime.terminal_window_tab(label).is_some())
}

fn destroy_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    close_windows(app, vec![label.to_owned()]);
}

/// The terminal window itself: the existing `index.html` with the window's
/// label in the URL fragment, so there is no second Vite entry point and the
/// fixed `app.js` / `style.css` output names are untouched.
///
/// `parent()` is deliberately not used. On macOS it forces the child above the
/// parent and hides it with the parent, and a popped-out terminal is meant to
/// sit on another display or Space and survive the main window being minimized.
fn terminal_window(app: &AppHandle, label: &str) -> Result<(), String> {
    let spec = window_spec();
    let monitors = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(display)
        .collect::<Vec<_>>();
    let primary = app.primary_monitor().ok().flatten().as_ref().map(display);
    let mut builder = tauri::WebviewWindowBuilder::new(
        app,
        label,
        tauri::WebviewUrl::App(format!("index.html#terminal-window={label}").into()),
    )
    .title(TERMINAL_WINDOW_TITLE)
    .background_color(tauri::window::Color(8, 13, 18, 255))
    .min_inner_size(480.0, 300.0)
    .inner_size(f64::from(spec.min_width), f64::from(spec.min_height));
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .traffic_light_position(tauri::LogicalPosition::new(16.0, 17.0));
    }
    if let Some(placement) = saved_placement(ptrack_cli::version(), label, &monitors, primary) {
        // The builder takes logical units, so the stored logical rect replays
        // without a scale conversion.
        builder = builder
            .inner_size(placement.logical.width, placement.logical.height)
            .position(placement.logical.x, placement.logical.y)
            .maximized(placement.maximized);
    }
    builder
        .build()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Destroys the named windows without running their close handler: their
/// assignments are already released, so there is nothing left to pop back in.
fn close_windows<R: Runtime>(app: &AppHandle<R>, labels: Vec<String>) {
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            // The shared app menu must not die with the window (see the
            // `CloseRequested` arm); this path destroys without a close
            // request, so it detaches on its own.
            #[cfg(windows)]
            let _ = window.remove_menu();
            let _ = window.destroy();
        }
    }
}

/// Returns a destroyed terminal window's session to the main window. The PTY
/// keeps running: only an explicit `CloseTerminal`, the shell exiting, a
/// project switch, or app quit terminates a session.
///
/// The assignment is the token — whoever clears it emits, and it can be cleared
/// only once. A window destroyed by a project switch or by app quit was cleared
/// by the drain that asked for the destruction, so this finds nothing and
/// cannot report the same session twice.
fn pop_in_terminal_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let runtime = app.state::<Arc<DesktopRuntime>>();
    let Some(tab) = runtime.close_terminal_window(label) else {
        return;
    };
    let _ = app.emit_to(
        MAIN_WINDOW_LABEL,
        "terminal:window-closed",
        serde_json::json!({ "label": label, "sessions": tab.sessions, "shape": tab.shape }),
    );
}

#[tauri::command]
async fn pick_project_directory(
    runtime: tauri::State<'_, Arc<DesktopRuntime>>,
    app: AppHandle,
    purpose: String,
) -> Result<String, String> {
    let purpose = ProjectPickerPurpose::parse(&purpose)?;
    let lease = runtime
        .inner()
        .begin_native_action()
        .map_err(|error| error.to_string())?;
    let default_directory = runtime
        .workspace_state()
        .project
        .map_or_else(std::env::current_dir, |project| {
            Ok(PathBuf::from(project.root))
        })
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let selected = app
            .dialog()
            .file()
            .set_title(purpose.title())
            .set_directory(default_directory)
            .blocking_pick_folder();
        project_picker_result(selected)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn project_picker_result(
    selected: Option<tauri_plugin_dialog::FilePath>,
) -> Result<String, String> {
    selected.map_or_else(
        || Ok(String::new()),
        |path| {
            path.into_path()
                .map_err(|error| error.to_string())
                .and_then(|path| {
                    path.into_os_string()
                        .into_string()
                        .map_err(|_| "selected project path is not valid UTF-8".to_owned())
                })
        },
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectPickerPurpose {
    Initialize,
    LocateRecentProject,
    Open,
}

impl ProjectPickerPurpose {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "initialize" => Ok(Self::Initialize),
            "locate-recent-project" => Ok(Self::LocateRecentProject),
            "open" => Ok(Self::Open),
            _ => Err("project picker purpose is invalid".to_owned()),
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Initialize => "Initialize p-track Project",
            Self::LocateRecentProject => "Locate p-track Project",
            Self::Open => "Open p-track Project",
        }
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri injects an owned AppHandle command argument.
fn open_external_url(
    runtime: tauri::State<'_, Arc<DesktopRuntime>>,
    app: AppHandle,
    url: String,
) -> Result<(), String> {
    validate_external_url(&url)?;
    let _lease = runtime
        .inner()
        .begin_native_action()
        .map_err(|error| error.to_string())?;
    app.opener()
        .open_url(url, None::<String>)
        .map_err(|error| error.to_string())
}

fn main() {
    let global_home = match resolve_global_home() {
        Ok(home) => home,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let current_dir = match std::env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let mut application = RoutedApplication::new(global_home, current_dir, ptrack_cli::version());
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let outcome = ptrack_cli::run(
        std::env::args_os(),
        &mut application,
        ptrack_cli::Io {
            stdin: Box::new(std::io::stdin()),
            stdout: &mut stdout,
            stderr: &mut stderr,
            cancellation: ptrack_app::McpCancellation::new(),
        },
    );
    match outcome {
        Ok(ptrack_cli::RunOutcome::ExitSuccess) => {}
        Ok(ptrack_cli::RunOutcome::LaunchGui { path, plan_id }) => {
            if let Err(error) = application.require_global_mode() {
                eprintln!("{error}");
                std::process::exit(1);
            }
            run_desktop(
                if path.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(path))
                },
                plan_id,
            );
        }
        Ok(ptrack_cli::RunOutcome::LaunchTui) => {
            let bindings = match application.bindings() {
                Ok(bindings) => bindings,
                Err(AppError::NoProject) => {
                    print!("{}", ptrack_cli::no_project_hint());
                    return;
                }
                Err(error) if error.to_string().contains("runtime is not initialized") => {
                    print!("{}", ptrack_cli::no_project_hint());
                    return;
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            };
            let Some(project) = bindings.project else {
                print!("{}", ptrack_cli::no_project_hint());
                return;
            };
            if let Err(error) = ptrack_tui::run(
                &mut application,
                ptrack_tui::RuntimeContext {
                    project_root: project.root,
                    database: project.database,
                    global_home: bindings.global_home,
                },
            ) {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

/// `Resized` and `Moved` fire continuously during a drag, so captures are
/// coalesced into one trailing write per second.
const WINDOW_CAPTURE_INTERVAL: Duration = Duration::from_secs(1);

/// Per-window capture bookkeeping; flags must remain label-scoped.
#[derive(Default)]
struct WindowCaptureState {
    sealed: bool,
    trailing: bool,
}

struct WindowStateCapture {
    version: String,
    /// The seal check and write share this lock so a trailing capture cannot
    /// overwrite a terminal flush.
    windows: Mutex<BTreeMap<String, WindowCaptureState>>,
}

impl WindowStateCapture {
    fn new() -> Self {
        Self {
            version: ptrack_cli::version().to_owned(),
            windows: Mutex::new(BTreeMap::new()),
        }
    }

    /// Coalesces drag events into one background write per window.
    /// Store lock retries must not block the event loop.
    fn schedule_trailing<R: Runtime>(self: &Arc<Self>, window: &tauri::Window<R>) {
        let label = window.label().to_owned();
        {
            let Ok(mut windows) = self.windows.lock() else {
                return;
            };
            let state = windows.entry(label.clone()).or_default();
            if state.trailing {
                return;
            }
            state.trailing = true;
        }
        let capture = Arc::clone(self);
        let window = window.clone();
        let spawned = std::thread::Builder::new()
            .name("ptrack-window-state".to_owned())
            .spawn(move || {
                std::thread::sleep(WINDOW_CAPTURE_INTERVAL);
                capture.clear_trailing(window.label());
                capture.flush(&window, false);
            });
        if spawned.is_err() {
            self.clear_trailing(&label);
        }
    }

    fn clear_trailing(&self, label: &str) {
        if let Ok(mut windows) = self.windows.lock()
            && let Some(state) = windows.get_mut(label)
        {
            state.trailing = false;
        }
    }

    /// Writes current geometry; `seal` makes this the final write for a window.
    fn flush<R: Runtime>(&self, window: &tauri::Window<R>, seal: bool) {
        // Read before locking: getters hop to the main thread, which could be
        // waiting for this lock and deadlock with a background capture.
        let state = window_geometry(window);
        self.guarded(window.label(), seal, |label| {
            if let Some(state) = state {
                save_window_state(&self.version, label, &state);
            }
        });
    }

    /// Runs one window's write unless its terminal flush already ran. Returns
    /// whether it ran.
    fn guarded(&self, label: &str, seal: bool, write: impl FnOnce(&str)) -> bool {
        let Ok(mut windows) = self.windows.lock() else {
            return false;
        };
        let state = windows.entry(label.to_owned()).or_default();
        if state.sealed {
            return false;
        }
        state.sealed = seal;
        write(label);
        true
    }
}

/// Adapts the window's physical geometry to the stored logical record.
fn window_geometry<R: Runtime>(window: &tauri::Window<R>) -> Option<WindowStateV1> {
    let scale_factor = window.scale_factor().ok()?;
    let position = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    let physical = RectV1 {
        x: f64::from(position.x),
        y: f64::from(position.y),
        width: f64::from(size.width),
        height: f64::from(size.height),
    };
    Some(captured(
        logical_rect(physical, scale_factor),
        scale_factor,
        window.is_maximized().unwrap_or(false),
        window.is_fullscreen().unwrap_or(false),
        display(&window.current_monitor().ok()??),
    ))
}

/// Fingerprints one display by its logical work area and scale factor.
fn display(monitor: &tauri::Monitor) -> DisplayV1 {
    let scale_factor = monitor.scale_factor();
    let work_area = monitor.work_area();
    DisplayV1 {
        work_area: logical_rect(
            RectV1 {
                x: f64::from(work_area.position.x),
                y: f64::from(work_area.position.y),
                width: f64::from(work_area.size.width),
                height: f64::from(work_area.size.height),
            },
            scale_factor,
        ),
        scale_factor,
    }
}

/// Replays the stored geometry onto one window, keyed by its label. Every
/// decision is made by `ptrack_app::window_state`; this only converts Tauri
/// types and applies the result.
fn restore_window_state<R: Runtime>(window: &tauri::WebviewWindow<R>, version: &str) {
    let monitors = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(display)
        .collect::<Vec<_>>();
    let primary = window
        .primary_monitor()
        .ok()
        .flatten()
        .as_ref()
        .map(display);
    let Some(placement) = saved_placement(version, window.label(), &monitors, primary) else {
        return;
    };
    let physical = physical_rect(placement.logical, placement.scale_factor);
    let _ = window.set_size(tauri::PhysicalSize::new(physical.width, physical.height));
    let _ = window.set_position(tauri::PhysicalPosition::new(physical.x, physical.y));
    if placement.maximized {
        let _ = window.maximize();
    }
}

/// Appends one startup failure to the evidence log in `directory` and returns
/// the log's path. The log appends, never truncates: a failure on launch two
/// must not erase what launch one recorded.
fn write_startup_failure(directory: &Path, error: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let path = directory.join("ptrack-startup-failure.log");
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(
        file,
        "[{seconds}] ptrack {}: {error}",
        ptrack_cli::version()
    )?;
    Ok(path)
}

/// The evidence log lives in the user's home directory: it must be writable
/// even when resolving the p-track global home is itself the failure.
fn record_startup_failure(error: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    write_startup_failure(Path::new(&home), error).ok()
}

/// Records and reports setup failure instead of unwinding through launch.
fn fail_startup(app: &tauri::AppHandle, error: &str) -> ! {
    let recorded = record_startup_failure(error);
    let detail = recorded
        .map(|path| format!("\n\nRecorded at {}", path.display()))
        .unwrap_or_default();
    app.dialog()
        .message(format!("p-track could not start.\n\n{error}{detail}"))
        .title("p-track")
        .blocking_show();
    std::process::exit(1);
}

#[allow(clippy::too_many_lines)] // One linear launch sequence; splitting it hides the order.
fn run_desktop(initial_path: Option<PathBuf>, initial_plan: u64) {
    // Any panic during startup or run leaves the same evidence trail before
    // the default hook prints to a stderr nobody can see under launchd.
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = record_startup_failure(&info.to_string());
        default_panic(info);
    }));
    let capture = Arc::new(WindowStateCapture::new());
    let capture_events = Arc::clone(&capture);
    let capture_exit = Arc::clone(&capture);
    let notifications = Arc::new(NativeNotificationController::default());
    let notifications_setup = Arc::clone(&notifications);
    let notifications_events = Arc::clone(&notifications);
    let notifications_windows = Arc::clone(&notifications);
    let closing_events = Arc::new(AtomicBool::new(false));
    let exit_gate = Arc::new(ExitGate::default());
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            app.manage(Arc::clone(&notifications_setup));
            // Restore and show before any fallible step, so an early return
            // cannot leave the initially hidden window invisible.
            if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                restore_window_state(&window, &capture.version);
                let _ = window.show();
            }
            let sink: Arc<dyn DesktopEventSink> = Arc::new(TauriEventSink {
                app: app.handle().clone(),
                notifications: Arc::clone(&notifications_events),
            });
            // A Tauri setup `Err` panics in its nounwind launch callback and
            // aborts without diagnostics; report errors through `fail_startup`.
            let runtime = (|| -> Result<_, String> {
                let global_home = resolve_global_home().map_err(|error| error.to_string())?;
                // Preserve Welcome as a startup decision; an inherited working
                // directory must not override the user's startup preference.
                let current_dir = std::env::current_dir().map_err(|error| error.to_string())?;
                let startup = resolved_startup_project(
                    &global_home,
                    ptrack_cli::version(),
                    initial_path.clone(),
                    &current_dir,
                );
                production_desktop_runtime_for_startup(
                    global_home,
                    ptrack_cli::version(),
                    &startup,
                    Some(Arc::clone(&sink)),
                    initial_plan,
                )
                .map_err(|error| error.to_string())
            })();
            let runtime = match runtime {
                Ok(runtime) => runtime,
                Err(error) => fail_startup(app.handle(), &error),
            };
            // The webview has not painted yet, so its first frame gets this theme.
            let mut preferences = runtime
                .invoke(DesktopCommandRequest {
                    method: "GetPreferences".to_owned(),
                    arguments: Vec::new(),
                })
                .unwrap_or_default();
            if notifications_setup.configure(app.handle(), &preferences, false) {
                preferences = runtime
                    .invoke(DesktopCommandRequest {
                        method: "SetPreferences".to_owned(),
                        arguments: vec![disabled_notification_patch()],
                    })
                    .unwrap_or_default();
                let _ = notifications_setup.configure(app.handle(), &preferences, false);
            }
            apply_theme(app.handle(), &preferences);
            app.manage(runtime);
            notifications_setup.initialize_focus(app.handle());
            notifications_setup.refresh(app.handle());
            Ok(())
        })
        .menu(build_menu)
        .on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()))
        .on_window_event(move |window, event| match event {
            // Every non-terminal capture is coalesced off the event loop: a
            // store write here blocks the drag it is recording.
            WindowEvent::Resized(_)
            | WindowEvent::Moved(_)
            | WindowEvent::ScaleFactorChanged { .. } => {
                capture_events.schedule_trailing(window);
            }
            WindowEvent::Focused(focused) => {
                notifications_windows.set_window_focus(window.label(), *focused);
            }
            WindowEvent::CloseRequested { api, .. } => {
                // Not sealed: a prevented close leaves the window alive, and
                // the exit flush below is the one that ends the session.
                capture_events.flush(window, false);
                // Terminal windows reattach their session on `Destroyed`.
                if window.label() != MAIN_WINDOW_LABEL {
                    // On Windows, `DestroyWindow` destroys the attached shared
                    // menu, so detach it before closing a terminal window.
                    #[cfg(windows)]
                    let _ = window.remove_menu();
                    return;
                }
                // Teardown runs off the event loop.
                api.prevent_close();
                close_main_window(window.app_handle(), &closing_events);
            }
            // Wait for the destroyed webview to release its output lease.
            WindowEvent::Destroyed => {
                notifications_windows.remove_window(window.label());
                if window.label() != MAIN_WINDOW_LABEL {
                    pop_in_terminal_window(window.app_handle(), window.label());
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            gui_invoke,
            pick_project_directory,
            open_external_url
        ]);
    let application = match builder.build(tauri::generate_context!()) {
        Ok(application) => application,
        Err(error) => {
            eprintln!("failed to run p-track desktop: {error}");
            std::process::exit(1);
        }
    };
    application.run(move |app, event| {
        // Cmd-Q reaches `RunEvent::Exit` without `CloseRequested`; flush every
        // registered window because terminal destruction order varies.
        // Exit requests run bounded teardown off the main thread when possible.
        match event {
            tauri::RunEvent::ExitRequested { api, code, .. } => match exit_gate.request() {
                ExitStep::Proceed => flush_all(app, &capture_exit),
                ExitStep::Hold => api.prevent_exit(),
                ExitStep::TearDown => {
                    api.prevent_exit();
                    exit_after_teardown(app, &exit_gate, code.unwrap_or(0));
                }
            },
            tauri::RunEvent::Exit => {
                if exit_gate.finish()
                    && let Some(runtime) = app.try_state::<Arc<DesktopRuntime>>()
                {
                    let _ = runtime.shutdown_within(EXIT_TEARDOWN_BOUND, true);
                }
                flush_all(app, &capture_exit);
            }
            _ => {}
        }
    });
}

/// No close or quit may wait longer than this for the runtime teardown.
const EXIT_TEARDOWN_BOUND: Duration = Duration::from_secs(3);

fn flush_all(app: &AppHandle, capture: &WindowStateCapture) {
    for webview in app.webview_windows().values() {
        capture.flush(&AsRef::<tauri::Webview>::as_ref(webview).window(), true);
    }
}

/// Where the application is on its way out. One teardown runs per process:
/// the first exit request starts it, requests arriving while it runs are held,
/// and the exit it issues itself — or the final `Exit` — goes through.
#[derive(Default)]
struct ExitGate(Mutex<ExitPhase>);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ExitPhase {
    #[default]
    Running,
    TearingDown,
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitStep {
    /// Start the teardown and exit once it is done.
    TearDown,
    /// A teardown is already running; keep the app alive until it exits.
    Hold,
    /// The teardown is done; let the exit through.
    Proceed,
}

impl ExitGate {
    fn phase(&self) -> std::sync::MutexGuard<'_, ExitPhase> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn request(&self) -> ExitStep {
        let mut phase = self.phase();
        match *phase {
            ExitPhase::Running => {
                *phase = ExitPhase::TearingDown;
                ExitStep::TearDown
            }
            ExitPhase::TearingDown => ExitStep::Hold,
            ExitPhase::Finished => ExitStep::Proceed,
        }
    }

    /// Marks the teardown done and reports whether the caller still has to
    /// run it: true unless a teardown already finished.
    fn finish(&self) -> bool {
        let mut phase = self.phase();
        let pending = *phase != ExitPhase::Finished;
        *phase = ExitPhase::Finished;
        pending
    }
}

/// Tears the runtime down on its own thread, then exits for real. A process
/// that cannot even spawn the thread exits at once rather than never.
fn exit_after_teardown<R: Runtime>(app: &AppHandle<R>, gate: &Arc<ExitGate>, code: i32) {
    let handle = app.clone();
    let gate_for_thread = Arc::clone(gate);
    let spawned = std::thread::Builder::new()
        .name("ptrack-exit".to_owned())
        .spawn(move || {
            if let Some(runtime) = handle.try_state::<Arc<DesktopRuntime>>() {
                let _ = runtime.shutdown_within(EXIT_TEARDOWN_BOUND, true);
            }
            gate_for_thread.finish();
            handle.exit(code);
        });
    if spawned.is_err() {
        gate.finish();
        app.exit(code);
    }
}

/// Closes the main window once the runtime is torn down, off the event loop.
///
/// A refusal — a call that did not drain in time — keeps the window and
/// every service open and tells the window why, through
/// `app:close-refused`. A teardown still running at the bound closes the
/// window anyway: the process is leaving, and the exit path gives the
/// teardown the rest of its time.
fn close_main_window<R: Runtime>(app: &AppHandle<R>, closing: &Arc<AtomicBool>) {
    if closing.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(runtime) = app
        .try_state::<Arc<DesktopRuntime>>()
        .map(|runtime| Arc::clone(runtime.inner()))
    else {
        closing.store(false, Ordering::SeqCst);
        destroy_window(app, MAIN_WINDOW_LABEL);
        return;
    };
    let handle = app.clone();
    let closing_for_thread = Arc::clone(closing);
    let spawned = std::thread::Builder::new()
        .name("ptrack-close".to_owned())
        .spawn(move || {
            match runtime.shutdown_within(EXIT_TEARDOWN_BOUND, false) {
                ShutdownOutcome::Completed(windows) => {
                    // The app exits with its main window, so the terminal
                    // windows go with it rather than outliving the runtime
                    // that serves them.
                    close_windows(&handle, windows);
                    destroy_window(&handle, MAIN_WINDOW_LABEL);
                }
                ShutdownOutcome::TimedOut => {
                    close_windows(&handle, runtime.drain_terminal_windows());
                    destroy_window(&handle, MAIN_WINDOW_LABEL);
                }
                ShutdownOutcome::Refused(message) => {
                    closing_for_thread.store(false, Ordering::SeqCst);
                    let _ = handle.emit_to(MAIN_WINDOW_LABEL, "app:close-refused", message);
                }
            }
        });
    if spawned.is_err() {
        closing.store(false, Ordering::SeqCst);
    }
}

#[allow(clippy::too_many_lines)] // Native menu order is an explicit frozen contract.
fn build_menu<R: Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<Menu<R>> {
    #[cfg(target_os = "macos")]
    let platform = DesktopPlatform::MacOs;
    #[cfg(not(target_os = "macos"))]
    let platform = DesktopPlatform::Other;
    let mut menu = MenuBuilder::new(app);
    for submenu_spec in menu_spec(platform) {
        let mut submenu = SubmenuBuilder::new(app, submenu_spec.label);
        for entry in submenu_spec.entries {
            submenu = match entry {
                MenuEntrySpec::Command {
                    id,
                    label,
                    macos_accelerator,
                } => submenu.item(&item(app, id, label, macos_accelerator)?),
                MenuEntrySpec::Separator => submenu.separator(),
                MenuEntrySpec::Role(role) => match role {
                    MenuRole::Services => submenu.services(),
                    MenuRole::Hide => submenu.hide(),
                    MenuRole::HideOthers => submenu.hide_others(),
                    MenuRole::ShowAll => submenu.show_all(),
                    MenuRole::Quit => submenu.quit(),
                    MenuRole::Cut => submenu.cut(),
                    MenuRole::Copy => submenu.copy(),
                    MenuRole::Paste => submenu.paste(),
                    MenuRole::SelectAll => submenu.select_all(),
                    MenuRole::Minimize => submenu.minimize(),
                    MenuRole::Maximize => submenu.maximize(),
                    MenuRole::Fullscreen => submenu.fullscreen(),
                    MenuRole::CloseWindow => submenu.close_window(),
                },
            };
        }
        let submenu = submenu.build()?;
        menu = menu.item(&submenu);
    }
    menu.build()
}

fn item<R: Runtime>(
    app: &tauri::AppHandle<R>,
    id: &str,
    label: &str,
    accelerator: Option<&str>,
) -> tauri::Result<MenuItem<R>> {
    let builder = MenuItemBuilder::with_id(id, label);
    let builder = if let Some(value) = accelerator {
        builder.accelerator(value)
    } else {
        builder
    };
    builder.build(app)
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let runtime = app.state::<Arc<DesktopRuntime>>();
    let Ok(_lease) = runtime.inner().begin_native_action() else {
        return;
    };
    match menu_dispatch(id) {
        MenuDispatch::Event(event) => {
            // The shared menu must target one capable webview, never broadcast.
            if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                let _ = main.set_focus();
            }
            let _ = app.emit_to(MAIN_WINDOW_LABEL, event, ());
        }
        MenuDispatch::Help(url) => {
            let _ = app.opener().open_url(url, None::<String>);
        }
        MenuDispatch::Ignore => {}
    }
}

fn validate_external_url(url: &str) -> Result<(), String> {
    if url.len() > 2_048 {
        return Err("external URL exceeds its byte limit".to_owned());
    }
    let parsed = tauri::Url::parse(url).map_err(|_| "external URL is invalid".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("external URL scheme is not allowed".to_owned());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("external URL credentials are not allowed".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod main_test;
