use std::{fs, path::Path};

use serde_json::Value;

fn read_json(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", path.display());
    });
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!("failed to parse {}: {error}", path.display());
    })
}

#[test]
fn shell_has_no_application_commands_or_plugins() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(manifest_dir.join("src/main.rs"))
        .expect("desktop shell source should be readable");
    let manifest = fs::read_to_string(manifest_dir.join("Cargo.toml"))
        .expect("desktop shell manifest should be readable");

    assert!(!source.contains("#[tauri::command]"));
    assert!(!source.contains("invoke_handler"));
    assert!(!manifest.contains("tauri-plugin-"));
}

#[test]
fn main_window_is_the_only_capability_target() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let capability = read_json(&manifest_dir.join("capabilities/main-window.json"));

    assert_eq!(
        capability["windows"],
        Value::Array(vec![Value::String("main".into())])
    );
    assert_eq!(
        capability["permissions"],
        Value::Array(vec![Value::String("core:default".into())])
    );
}

#[test]
fn tauri_uses_the_existing_frontend_build() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = read_json(&manifest_dir.join("tauri.conf.json"));

    assert_eq!(config["build"]["beforeDevCommand"], "npm run dev");
    assert_eq!(config["build"]["beforeBuildCommand"], "npm run build");
    assert_eq!(config["build"]["devUrl"], "http://localhost:5173");
    assert_eq!(config["build"]["frontendDist"], "../frontend/dist");
    assert_eq!(
        config["app"]["security"]["capabilities"],
        Value::Array(vec![Value::String("main-window".into())])
    );
}
