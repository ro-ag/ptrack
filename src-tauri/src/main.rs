#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ptrack_app::window_state::{
    DisplayV1, RectV1, WindowStateV1, captured, logical_rect, physical_rect, save_window_state,
    saved_placement,
};
use ptrack_app::{
    AppError, DesktopCommandRequest, DesktopEvent, DesktopEventSink, DesktopRuntime,
    RoutedApplication, StartupProjectV1, production_desktop_runtime, resolve_global_home,
    resolved_startup_project,
};
use ptrack_desktop::{
    DesktopPlatform, MenuDispatch, MenuEntrySpec, MenuRole, menu_dispatch, menu_spec,
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

struct WindowStateCapture {
    version: String,
    /// Set by the terminal flush. A trailing capture that wakes up after it
    /// must not put its stale rect back, so the flag and the write share one
    /// lock and a late capture is dropped instead of ordered behind it.
    sealed: Mutex<bool>,
    trailing: AtomicBool,
}

impl WindowStateCapture {
    fn new() -> Self {
        Self {
            version: ptrack_cli::version().to_owned(),
            sealed: Mutex::new(false),
            trailing: AtomicBool::new(false),
        }
    }

    /// Coalesces one window event into a trailing write on its own thread: the
    /// global store retries a busy lock for up to a second per open, and the
    /// event loop this drag runs on cannot afford to wait for it. One trailing
    /// write is pending at a time, and it is redundant whenever a later event
    /// or the exit flush beats it.
    fn schedule_trailing<R: Runtime>(self: &Arc<Self>, window: &tauri::Window<R>) {
        if self.trailing.swap(true, Ordering::SeqCst) {
            return;
        }
        let capture = Arc::clone(self);
        let window = window.clone();
        let spawned = std::thread::Builder::new()
            .name("ptrack-window-state".to_owned())
            .spawn(move || {
                std::thread::sleep(WINDOW_CAPTURE_INTERVAL);
                capture.trailing.store(false, Ordering::SeqCst);
                capture.flush(&window, false);
            });
        if spawned.is_err() {
            self.trailing.store(false, Ordering::SeqCst);
        }
    }

    /// Writes the current geometry now. `seal` marks the last write of the
    /// process: the exit flushes stay synchronous because an async write dies
    /// with the process, and sealing keeps a late trailing capture from
    /// landing after them.
    fn flush<R: Runtime>(&self, window: &tauri::Window<R>, seal: bool) {
        // The geometry is read before the lock is taken. The window getters hop
        // to the main thread, so a background capture holding the lock while it
        // waits for a main thread blocked on that same lock would deadlock.
        let state = window_geometry(window);
        self.guarded(seal, || {
            if let Some(state) = state {
                save_window_state(&self.version, &state);
            }
        });
    }

    /// Runs one write unless the terminal flush already ran. Returns whether
    /// it ran.
    fn guarded(&self, seal: bool, write: impl FnOnce()) -> bool {
        let Ok(mut sealed) = self.sealed.lock() else {
            return false;
        };
        if *sealed {
            return false;
        }
        *sealed = seal;
        write();
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

/// Replays the stored geometry onto the main window. Every decision is made by
/// `ptrack_app::window_state`; this only converts Tauri types and applies the
/// result.
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
    let Some(placement) = saved_placement(version, &monitors, primary) else {
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
            if let Some(window) = app.get_webview_window("main") {
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
                let runtime = window.state::<Arc<DesktopRuntime>>();
                if runtime.begin_shutdown().is_err() {
                    api.prevent_close();
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
        // macOS quits through the Quit menu role and its Cmd-Q accelerator by
        // terminating the application, which tao reports as
        // `applicationWillTerminate` and Tauri as `RunEvent::Exit`: no window
        // ever sees `CloseRequested`. Both exit events flush, so the most
        // common quit gesture cannot leave a stale rect behind.
        if matches!(
            event,
            tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
        ) && let Some(webview) = app.get_webview_window("main")
        {
            capture_exit.flush(&AsRef::<tauri::Webview>::as_ref(&webview).window(), true);
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
            let _ = app.emit(event, ());
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
