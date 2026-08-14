#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use ptrack_app::{
    AppError, DesktopCommandRequest, DesktopEvent, DesktopEventSink, DesktopRuntime,
    RoutedApplication, production_desktop_runtime, resolve_global_home,
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

fn run_desktop(initial_path: Option<PathBuf>, initial_plan: u64) {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let sink: Arc<dyn DesktopEventSink> = Arc::new(TauriEventSink {
                app: app.handle().clone(),
            });
            let current = initial_path
                .clone()
                .map_or_else(std::env::current_dir, Ok)
                .map_err(std::io::Error::other)?;
            let runtime = production_desktop_runtime(
                resolve_global_home().map_err(std::io::Error::other)?,
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
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let runtime = window.state::<Arc<DesktopRuntime>>();
                if runtime.begin_shutdown().is_err() {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            gui_invoke,
            pick_project_directory,
            open_external_url
        ]);
    match builder.run(tauri::generate_context!()) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("failed to run p-track desktop: {error}");
            std::process::exit(1);
        }
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
