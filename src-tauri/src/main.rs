#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ptrack_app::window_state::{
    DisplayV1, MAIN_WINDOW_LABEL, RectV1, WindowStateV1, captured, logical_rect, physical_rect,
    save_window_state, saved_placement,
};
use ptrack_app::{
    AppError, DesktopCommandRequest, DesktopEvent, DesktopEventSink, DesktopRuntime,
    RoutedApplication, StartupProjectV1, production_desktop_runtime, resolve_global_home,
    resolved_startup_project,
};
use ptrack_desktop::{
    DesktopPlatform, MenuDispatch, MenuEntrySpec, MenuRole, menu_dispatch, menu_spec, window_spec,
};
use tauri::menu::{
    AboutMetadataBuilder, Menu, MenuBuilder, MenuItem, MenuItemBuilder, SubmenuBuilder,
};
use tauri::{AppHandle, Emitter, Manager, Runtime, WindowEvent};
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_opener::OpenerExt as _;

struct TauriEventSink {
    app: AppHandle,
}

impl DesktopEventSink for TauriEventSink {
    fn emit(&self, event: DesktopEvent) {
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
        };
        let _ = result;
    }
}

#[tauri::command]
async fn gui_invoke(
    runtime: tauri::State<'_, Arc<DesktopRuntime>>,
    app: AppHandle,
    request: DesktopCommandRequest,
) -> Result<serde_json::Value, String> {
    let runtime = Arc::clone(runtime.inner());
    let shell_command = request.method == "InstallShellCommand";
    // Every command that answers with the whole normalized preference document
    // resyncs the native chrome. The read is in the set because
    // `ResetApplicationState` clears the record behind its own result shape and
    // the client reloads afterwards, so one resync point covers every writer.
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
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let result = runtime.invoke(request).map_err(|error| error.to_string())?;
        if appearance {
            apply_theme(&app, &result);
        }
        // Switching or closing a project takes its terminal windows with it:
        // the sessions they showed died with the workspace. The sweep is a
        // lock and a comparison while nothing changed.
        close_windows(&app, runtime.expire_terminal_windows());
        if terminal_window {
            let label = result["label"].as_str().unwrap_or_default().to_owned();
            if let Err(error) = build_terminal_window(&app, &label) {
                // A failed pop-out must never leave a session with no owner:
                // the assignment is released so the main window keeps it.
                runtime.close_terminal_window(&label);
                return Err(error);
            }
        }
        if shell_command {
            let message = result.as_str().ok_or_else(|| {
                "shell command installation returned an invalid result".to_owned()
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
    .map_err(|error| error.to_string())?
}

/// Maps the stored appearance preference onto the native window theme. `None`
/// is "follow the OS": an unset preferred appearance is how every platform
/// spells that, and it is what keeps `"system"` tracking later OS flips without
/// the shell hearing about them.
fn preferred_theme(preferences: &serde_json::Value) -> Option<tauri::Theme> {
    match preferences["appearance"]["theme"].as_str() {
        Some("dark") => Some(tauri::Theme::Dark),
        Some("light") => Some(tauri::Theme::Light),
        _ => None,
    }
}

/// Repaints the native chrome to match the app theme. macOS paints its own
/// titlebar out of `NSApp.effectiveAppearance`, which follows the *system*
/// Dark/Light setting and not the app's theme, so a dark app on a light system
/// gets a light titlebar around a dark window. `set_theme` routes to
/// `NSApplication setAppearance:`, which covers the titlebar, the menu bar, and
/// the native dialogs at once; on Windows it repaints the frame and on Linux it
/// sets the GTK preference. Best effort: chrome that stays a shade off is never
/// worth failing a preference write over.
///
/// Every window is repainted, not the hard-coded `main`: a popped-out terminal
/// window left on the old theme is the same defect on a second frame.
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

/// Builds one terminal window and waits for the result.
///
/// `WebviewWindowBuilder::build` deadlocks when it is called synchronously on
/// Windows and `gui_invoke` runs on `spawn_blocking`, so the build is
/// dispatched with `run_on_main_thread`.
fn build_terminal_window(app: &AppHandle, label: &str) -> Result<(), String> {
    let handle = app.clone();
    let label = label.to_owned();
    let (sender, receiver) = channel();
    app.run_on_main_thread(move || {
        let _ = sender.send(terminal_window(&handle, &label));
    })
    .map_err(|error| error.to_string())?;
    receiver
        .recv_timeout(TERMINAL_WINDOW_BUILD_TIMEOUT)
        .map_err(|error| error.to_string())?
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
    .min_inner_size(f64::from(spec.min_width), f64::from(spec.min_height))
    .inner_size(f64::from(spec.min_width), f64::from(spec.min_height));
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
    let Some(session_id) = runtime.close_terminal_window(label) else {
        return;
    };
    let _ = app.emit_to(
        MAIN_WINDOW_LABEL,
        "terminal:window-closed",
        serde_json::json!({ "label": label, "sessionId": session_id }),
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
            cancellation: ptrack_app::CapabilityCancellation::new(),
        },
    );
    match outcome {
        Ok(ptrack_cli::RunOutcome::ExitSuccess) => {}
        Ok(ptrack_cli::RunOutcome::LaunchGui { path, plan_id }) => {
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

/// Per-window capture bookkeeping. Both flags are label-scoped: with a second
/// window open, sealing every window on the first terminal flush would throw
/// away the rect the other one really closed at.
#[derive(Default)]
struct WindowCaptureState {
    sealed: bool,
    trailing: bool,
}

struct WindowStateCapture {
    version: String,
    /// `sealed` is set by a window's terminal flush. A trailing capture that
    /// wakes up after it must not put its stale rect back, so the flag and the
    /// write share one lock and a late capture is dropped instead of ordered
    /// behind it.
    windows: Mutex<BTreeMap<String, WindowCaptureState>>,
}

impl WindowStateCapture {
    fn new() -> Self {
        Self {
            version: ptrack_cli::version().to_owned(),
            windows: Mutex::new(BTreeMap::new()),
        }
    }

    /// Coalesces one window event into a trailing write on its own thread: the
    /// global store retries a busy lock for up to a second per open, and the
    /// event loop this drag runs on cannot afford to wait for it. One trailing
    /// write is pending per window at a time, and it is redundant whenever a
    /// later event or the exit flush beats it.
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

    /// Writes the window's current geometry now. `seal` marks that window's
    /// last write of the process: the exit flushes stay synchronous because an
    /// async write dies with the process, and sealing keeps a late trailing
    /// capture from landing after them.
    fn flush<R: Runtime>(&self, window: &tauri::Window<R>, seal: bool) {
        // The geometry is read before the lock is taken. The window getters hop
        // to the main thread, so a background capture holding the lock while it
        // waits for a main thread blocked on that same lock would deadlock.
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

fn run_desktop(initial_path: Option<PathBuf>, initial_plan: u64) {
    let capture = Arc::new(WindowStateCapture::new());
    let capture_events = Arc::clone(&capture);
    let capture_exit = Arc::clone(&capture);
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            // The window is configured hidden so the restored rect is the first
            // one painted instead of a visible jump from the default geometry.
            // Restore and show run before every fallible step below: no `?` and
            // no early return can leave the window invisible, and a restore that
            // decides to leave the configured geometry alone still shows it.
            if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                restore_window_state(&window, &capture.version);
                let _ = window.show();
            }
            let sink: Arc<dyn DesktopEventSink> = Arc::new(TauriEventSink {
                app: app.handle().clone(),
            });
            let global_home = resolve_global_home().map_err(std::io::Error::other)?;
            // An explicit context wins: a named path, then a working directory
            // that is itself a project. The opt-in only decides the Finder and
            // Dock launch, where the working directory is no project.
            let current_dir = std::env::current_dir().map_err(std::io::Error::other)?;
            let current = match resolved_startup_project(
                &global_home,
                ptrack_cli::version(),
                initial_path.clone(),
                &current_dir,
            ) {
                StartupProjectV1::Open(path) => path,
                StartupProjectV1::Welcome(_) => current_dir,
            };
            let runtime = production_desktop_runtime(
                global_home,
                ptrack_cli::version(),
                &current,
                Some(Arc::clone(&sink)),
                initial_plan,
            )
            .map_err(std::io::Error::other)?;
            // The stored preference is only reachable once the runtime is bound,
            // and the window contract fixes that after the show. The webview has
            // not painted yet either way, so the first frame the user reads
            // already carries the right chrome.
            apply_theme(
                app.handle(),
                &runtime
                    .invoke(DesktopCommandRequest {
                        method: "GetPreferences".to_owned(),
                        arguments: Vec::new(),
                    })
                    .unwrap_or_default(),
            );
            app.manage(runtime);
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
            WindowEvent::CloseRequested { api, .. } => {
                // Not sealed: a prevented close leaves the window alive, and
                // the exit flush below is the one that ends the session.
                capture_events.flush(window, false);
                // A terminal window must never begin shutdown: that would kill
                // the whole app runtime and leave the main window a shell whose
                // every command fails. Its session pops back in on `Destroyed`.
                if window.label() != MAIN_WINDOW_LABEL {
                    // `Builder::menu` shares one native menu across every
                    // window, and on Windows `DestroyWindow` destroys the menu
                    // still attached to the dying window. Left attached, the
                    // first pop-in kills the shared handle and every window
                    // built afterwards paints without a menu bar. Detached
                    // here, the menu survives the close. Windows only: macOS
                    // has one application-global menu no window can take down,
                    // and a GTK window owns its own menubar widget.
                    #[cfg(windows)]
                    let _ = window.remove_menu();
                    return;
                }
                let runtime = window.state::<Arc<DesktopRuntime>>();
                if runtime.begin_shutdown().is_err() {
                    api.prevent_close();
                    return;
                }
                // The app exits with its main window, so the terminal windows
                // go with it rather than outliving the runtime that serves them.
                close_windows(window.app_handle(), runtime.drain_terminal_windows());
            }
            // The pop-in waits for the webview to be gone. Its stream socket
            // drops with it, and only then does the session release its output
            // lease: claiming on the close request instead makes the main
            // window's re-attach race that release and lose it about half the
            // time, which the reclaim loop then papers over with a visible
            // "Reconnecting…". The assignment is still the token — a window
            // destroyed by a project switch or by app quit had its assignment
            // cleared first, so this finds nothing and reports nothing.
            WindowEvent::Destroyed if window.label() != MAIN_WINDOW_LABEL => {
                pop_in_terminal_window(window.app_handle(), window.label());
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
        // macOS quits through the Quit menu role and its Cmd-Q accelerator by
        // terminating the application, which tao reports as
        // `applicationWillTerminate` and Tauri as `RunEvent::Exit`: no window
        // ever sees `CloseRequested`. Both exit events flush, so the most
        // common quit gesture cannot leave a stale rect behind.
        //
        // Whichever window is destroyed last varies once a terminal window
        // exists, so the flush enumerates whatever is still registered instead
        // of assuming `main` is.
        if matches!(
            event,
            tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
        ) {
            for webview in app.webview_windows().values() {
                capture_exit.flush(&AsRef::<tauri::Webview>::as_ref(webview).window(), true);
            }
        }
    });
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
                    MenuRole::About => submenu.about(Some(
                        AboutMetadataBuilder::new()
                            .name(Some("p-track"))
                            .version(Some(ptrack_cli::version()))
                            .build(),
                    )),
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
            // `Builder::menu` applies one menu to every window and `emit`
            // broadcasts to every webview, so a broadcast fires each command
            // once per window — "Open Project…" would open two dialogs. Every
            // command in the allowlist acts on the project workspace, and a
            // terminal window has no board, no palette and no dialogs, so the
            // main window answers wherever the command was invoked from.
            // Targeting the focused window instead made every accelerator a
            // silent no-op while a terminal window was in front: that window
            // listens for `terminal:exit` and nothing else.
            //
            // Raised first, because an answer painted behind the window the
            // user is looking at is the same dead accelerator with extra steps.
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
