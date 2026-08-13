use std::{fs, path::Path};

use serde_json::Value;

use ptrack_desktop::{DesktopPlatform, MenuDispatch, menu_dispatch, menu_spec, window_spec};

fn read_json(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", path.display());
    });
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!("failed to parse {}: {error}", path.display());
    })
}

#[test]
fn shell_has_only_the_bounded_adapter_commands() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(manifest_dir.join("src/main.rs"))
        .expect("desktop shell source should be readable");
    let manifest = fs::read_to_string(manifest_dir.join("Cargo.toml"))
        .expect("desktop shell manifest should be readable");

    assert_eq!(source.matches("#[tauri::command]").count(), 3);
    assert!(source.contains("gui_invoke"));
    assert!(source.contains("pick_project_directory"));
    assert!(source.contains("open_external_url"));
    assert!(source.contains("tauri::generate_handler!["));
    assert!(source.contains("UpdateRuntime::for_bindings("));
    assert!(source.contains("ActiveRuntime::load("));
    assert!(source.contains("ProductionDesktopWorkspaceFactory::new("));
    assert!(!source.contains("UnavailableApplication"));
    assert!(source.contains("DesktopUpdateEventSink::new("));
    assert!(!source.contains("config.update_service = UnavailableUpdateService"));
    assert!(source.contains("request.method == \"InstallShellCommand\""));
    assert!(source.contains(".title(\"Shell Command\")"));
    assert!(source.contains("ptrack_cli::version()"));
    assert!(!source.contains("windows_subsystem"));
    assert!(manifest.contains("name = \"ptrack\"\npath = \"src/main.rs\""));
    assert!(manifest.contains("tauri-plugin-dialog = \"=2.7.2\""));
    assert!(manifest.contains("tauri-plugin-opener = \"=2.5.4\""));
    assert!(!manifest.contains("tauri-plugin-shell"));
    assert!(!manifest.contains("tauri-plugin-http"));
    assert!(!manifest.contains("tauri-plugin-fs ="));
}

#[test]
fn main_window_has_only_one_way_event_subscription_authority() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let capability = read_json(&manifest_dir.join("capabilities/main-window.json"));

    assert_eq!(
        capability["windows"],
        Value::Array(vec![Value::String("main".into())])
    );
    assert_eq!(
        capability["permissions"],
        Value::Array(vec![
            Value::String("core:event:allow-listen".into()),
            Value::String("core:event:allow-unlisten".into()),
        ])
    );
    let encoded = capability["permissions"].to_string();
    for forbidden in [
        "core:default",
        "allow-emit",
        "allow-emit-to",
        "image",
        "menu",
        "tray",
        "window",
        "webview",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "ambient permission {forbidden}"
        );
    }
}

#[test]
fn tauri_uses_the_existing_frontend_and_exact_window_contract() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = read_json(&manifest_dir.join("tauri.conf.json"));

    assert_eq!(config["build"]["beforeDevCommand"], "npm run dev");
    assert_eq!(config["build"]["beforeBuildCommand"], "npm run build");
    assert_eq!(config["build"]["devUrl"], "http://localhost:5173");
    assert_eq!(config["build"]["frontendDist"], "../frontend/dist");
    assert_eq!(config["app"]["windows"][0]["width"], 1440);
    assert_eq!(config["app"]["windows"][0]["height"], 900);
    assert_eq!(config["app"]["windows"][0]["minWidth"], 880);
    assert_eq!(config["app"]["windows"][0]["minHeight"], 560);
    assert_eq!(
        config["app"]["windows"][0]["title"],
        "p-track Project Workspace"
    );
    assert_eq!(config["app"]["windows"][0]["backgroundColor"], "#080d12");
    assert_eq!(config["bundle"]["macOS"]["minimumSystemVersion"], "12.0");
    assert_eq!(
        config["bundle"]["macOS"]["entitlements"],
        "../build/darwin/entitlements.plist"
    );
    assert!(
        config["app"]["security"]["csp"]
            .as_str()
            .is_some_and(|csp| csp.contains("ws://127.0.0.1:*"))
    );
    assert_eq!(
        config["app"]["security"]["capabilities"],
        Value::Array(vec![Value::String("main-window".into())])
    );
}

#[test]
fn external_url_gate_rejects_non_web_and_credentialed_urls() {
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
        .expect("desktop shell source should be readable");
    assert!(source.contains("matches!(parsed.scheme(), \"http\" | \"https\")"));
    assert!(source.contains("parsed.username().is_empty()"));
    assert!(source.contains("parsed.password().is_some()"));
}

#[test]
fn menu_event_and_help_allowlists_are_exact() {
    for event in [
        "workspace:open-requested",
        "workspace:switch-requested",
        "workspace:close-requested",
        "workspace:settings-requested",
        "workspace:capabilities-requested",
        "workspace:board-requested",
        "workspace:intelligence-requested",
        "workspace:terminal-panel-toggle-requested",
        "workspace:command-palette-requested",
        "workspace:install-shell-command-requested",
        "update:open-requested",
    ] {
        assert_eq!(menu_dispatch(event), MenuDispatch::Event(event));
    }
    assert_eq!(
        menu_spec(DesktopPlatform::Other)
            .iter()
            .map(|menu| menu.label)
            .collect::<Vec<_>>(),
        ["File", "Project", "View", "Help"]
    );
    assert_eq!(
        menu_spec(DesktopPlatform::MacOs)
            .iter()
            .map(|menu| menu.label)
            .collect::<Vec<_>>(),
        [
            "p-track", "File", "Project", "Edit", "View", "Window", "Help"
        ]
    );
    let window = window_spec();
    assert_eq!(window.title, "p-track Project Workspace");
    assert_eq!(window.background, "#080d12");
    assert_eq!((window.width, window.height), (1_440, 900));
    assert_eq!((window.min_width, window.min_height), (880, 560));
}

#[test]
fn parity_matrix_counts_are_self_consistent() {
    let matrix = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/rust-parity-matrix.md"),
    )
    .expect("parity matrix should be readable");
    let ids = matrix
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|line| line.split_once('`').map(|(id, _)| id))
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 762);
    assert_eq!(ids.iter().filter(|id| id.starts_with("GUI-")).count(), 136);
    assert_eq!(ids.iter().filter(|id| id.starts_with("TERM-")).count(), 108);
}
