use std::{env, fs, path::Path};

use serde_json::Value;

use ptrack_desktop::{DesktopPlatform, MenuDispatch, menu_dispatch, menu_spec, window_spec};

/// Reads a text file with its line endings normalized. Windows checks the tree
/// out with CRLF, where a pattern spanning a newline matches nothing at all —
/// and a scan that matches nothing yields an empty slice that every claim made
/// against it passes. Every source scan below goes through here.
fn read_text(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

fn shell_source() -> String {
    read_text(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
}

/// The body of the window builder, as source text. A miss is fatal rather than
/// empty: an empty body silently satisfies every claim made about it.
fn terminal_window_builder(source: &str) -> &str {
    source
        .split_once("fn terminal_window(")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map(|(body, _)| body)
        .expect("the terminal window builder must be findable")
}

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
    let source = shell_source();
    let manifest = read_text(&manifest_dir.join("Cargo.toml"));

    assert_eq!(source.matches("#[tauri::command]").count(), 4);
    assert!(source.contains("gui_invoke"));
    assert!(source.contains("pick_project_directory"));
    assert!(source.contains("open_external_url"));
    // The frameless Windows and Linux title bar's controls; it grants the webview no
    // capability permission, and other platforms refuse it.
    assert!(source.contains("fn window_chrome("));
    assert!(source.contains("Err(\"custom window chrome requires Windows or Linux\".to_owned())"));
    assert!(source.contains("tauri::generate_handler!["));
    assert!(source.contains("production_desktop_runtime_for_startup("));
    assert!(source.contains("app.manage(runtime)"));
    assert!(!source.contains("ProductionDesktopAuthority::load("));
    assert!(!source.contains("DesktopRuntimeConfig::unavailable("));
    assert!(!source.contains("UpdateRuntime::for_bindings("));
    assert!(!source.contains("ActiveRuntime::load("));
    assert!(!source.contains("UnavailableApplication"));
    assert!(!source.contains("config.update_service = UnavailableUpdateService"));
    assert!(source.contains("request.method == \"InstallShellCommand\""));
    assert!(source.contains(".title(\"Shell Command\")"));
    let shell_dialog_lease = source
        .find("let _dialog_lease = if shell_command")
        .expect("shell command native dialog must acquire a shutdown fence");
    // Every dispatch carries the calling window's label into the runtime, so
    // terminal commands stay scoped to the window's own sessions there too.
    let shell_invoke = source
        .find(".invoke_from_window(&window_label, request)")
        .expect("shell command must run through the desktop bridge");
    let shell_dialog = source
        .find(".blocking_show()")
        .expect("shell command result must use the native dialog");
    assert!(shell_dialog_lease < shell_invoke && shell_invoke < shell_dialog);
    assert_eq!(
        source
            .matches(".invoke_from_window(&window_label, request)")
            .count(),
        2,
        "both bridge dispatches must carry the calling window"
    );
    assert!(source.contains("scope_request_to_window(window.label(), request)"));
    // The internal notification patch is host code, never window traffic: it
    // keeps the trusted unrestricted entry.
    assert!(source.contains("arguments: vec![disabled_notification_patch()]"));
    assert!(source.contains("ptrack_cli::version()"));
    // Every bridge request is scoped by the window that sent it before it
    // reaches the runtime, so a terminal window cannot use the main window's
    // commands or address another window's assignment.
    let scoped = source
        .find("scope_request_to_window(window.label(), request)")
        .expect("gui_invoke must scope each request to its calling window");
    assert!(scoped < shell_invoke);
    assert!(!source.contains("windows_subsystem"));
    assert!(manifest.contains("name = \"ptrack\"\npath = \"src/main.rs\""));
    assert!(manifest.contains("tauri-plugin-dialog = \"=2.7.2\""));
    assert!(manifest.contains("tauri-plugin-notification = \"=2.3.3\""));
    assert!(manifest.contains("tauri-plugin-opener = \"=2.5.4\""));
    assert!(!manifest.contains("tauri-plugin-shell"));
    assert!(!manifest.contains("tauri-plugin-http"));
    assert!(!manifest.contains("tauri-plugin-fs ="));
}

#[test]
fn main_window_has_only_one_way_event_subscription_authority() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let capability = read_json(&manifest_dir.join("capabilities/main-window.json"));

    // Runtime-created terminal windows need their own labels. The window-drag
    // permissions are the exception to event-only authority: the Overlay
    // title bar style leaves nothing
    // native to grab, so the frontend's data-tauri-drag-region strips need
    // start-dragging (and internal toggle-maximize for double-click zoom).
    // Everything else stays forbidden.
    assert_eq!(
        capability["windows"],
        Value::Array(vec![
            Value::String("main".into()),
            Value::String("terminal-*".into()),
        ])
    );
    assert_eq!(
        capability["permissions"],
        Value::Array(vec![
            Value::String("core:event:allow-listen".into()),
            Value::String("core:event:allow-unlisten".into()),
            Value::String("core:window:allow-start-dragging".into()),
            Value::String("core:window:allow-internal-toggle-maximize".into()),
        ])
    );
    let encoded = capability["permissions"].to_string();
    for forbidden in [
        "core:default",
        "allow-emit",
        "allow-emit-to",
        "image",
        "menu",
        "notification",
        "tray",
        "core:window:default",
        "webview",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "ambient permission {forbidden}"
        );
    }
}

/// A terminal window is a second window over the same runtime, so every
/// app-wide handler is a defect the moment one exists.
///
/// Only claims that are inherently source-level live here — how the window is
/// built, and which label the shell addresses. The behaviour behind them is
/// tested where it can actually run: the assignment lifecycle and its
/// report-exactly-once discipline in `ptrack-app`'s `desktop_runtime_test` and
/// `terminal_windows_test`. Which arm of Tauri's window-event match a call sits
/// in is not observable without a windowing harness, and asserting the source
/// text of that match only reads as coverage it does not have.
#[test]
fn terminal_windows_are_label_scoped_and_independent() {
    let source = shell_source();

    // Which window event a call is wired to is source-level by nature: no test
    // can deliver one without a windowing harness. These say where the calls
    // sit, and nothing about what they do — that is the runtime's job above.
    let handler = source
        .split_once(".on_window_event(")
        .and_then(|(_, rest)| rest.split_once(".invoke_handler("))
        .map(|(body, _)| body)
        .expect("the shell must register a window-event handler");
    let close_requested = handler
        .find("WindowEvent::CloseRequested")
        .expect("the shell must handle the close request");
    // Only the main window's close begins shutdown. A terminal window that
    // began it would kill the app runtime and leave the main window a dead
    // shell whose every command fails.
    let main_only = handler
        .find("if window.label() != MAIN_WINDOW_LABEL {")
        .expect("close must be label scoped");
    // The main window's close holds the close and tears the runtime down off
    // the event loop, bounded; the runtime tests cover what that teardown does.
    let shutdown = handler
        .find("close_main_window(window.app_handle(), &closing_events);")
        .expect("the main window's close must begin shutdown");
    let held = handler
        .find("api.prevent_close();")
        .expect("the main window's close must be held until teardown finishes");
    assert!(main_only < held && held < shutdown);
    assert!(source.contains("runtime.shutdown_within(EXIT_TEARDOWN_BOUND, false)"));
    // Quit takes the same bounded teardown, whether or not a window saw it.
    let run = source
        .split_once("application.run(move |app, event| {")
        .map(|(_, rest)| rest)
        .expect("the shell must run the application event loop");
    assert!(run.contains("tauri::RunEvent::ExitRequested { api, code, .. }"));
    assert!(run.contains("runtime.shutdown_within(EXIT_TEARDOWN_BOUND, true)"));
    // The pop-in runs on destruction, not on the close request: the webview's
    // stream socket drops with the webview, and only then does its session
    // release the output lease the main window is about to re-claim.
    let destroyed = handler
        .find("WindowEvent::Destroyed")
        .expect("a destroyed terminal window must pop its session back in");
    let pop_in = handler
        .find("pop_in_terminal_window(")
        .expect("the destroyed window's session must go back to the main window");
    assert!(close_requested < main_only && main_only < shutdown);
    assert!(shutdown < destroyed && destroyed < pop_in);
    assert_eq!(handler.matches("pop_in_terminal_window(").count(), 1);

    // A failed build releases the assignment, so a failed pop-out never leaves
    // a session with no owner.
    let build = source
        .find("if let Err(error) = build_terminal_window(&app, &label) {")
        .expect("the window build must be fallible");
    let release = source
        .find("runtime.close_terminal_window(&label);")
        .expect("a failed build must release the assignment");
    assert!(build < release);

    // The build is dispatched to the main thread: it deadlocks when called
    // synchronously on Windows and `gui_invoke` runs on `spawn_blocking`.
    assert!(source.contains("app.run_on_main_thread(move || {"));
    // The window is independent, never a platform child. Scoped to the builder
    // itself: `.parent(` anywhere else in the shell, in a path or a comment,
    // says nothing about this window.
    let builder = terminal_window_builder(&source);
    assert!(builder.contains("WebviewWindowBuilder::new("));
    assert!(!builder.contains(".parent("));
    // One frontend bundle, addressed by URL fragment.
    assert!(builder.contains("index.html#terminal-window={label}"));
    // Menu commands reach the main window: every one of them acts on the
    // project workspace, a broadcast would fire each one once per window, and
    // targeting the focused window made them dead while a terminal window was
    // in front.
    assert!(source.contains("app.emit_to(MAIN_WINDOW_LABEL, event, ())"));
    assert!(!source.contains("app.emit(event, ())"));
    assert!(!source.contains("focused_label"));
    // Theme and the exit flush cover every window, not the hard-coded `main`.
    assert!(!source.contains("app.get_webview_window(\"main\")"));
}

/// What a terminal window may call is the runtime's exact allowlist: the
/// terminal surface, its own assignment, the shared theme preference, and the
/// project scratchpad its side panel edits. Nothing that changes plans, tasks,
/// associations, or project memory is reachable from a popped-out window.
#[test]
fn terminal_windows_reach_only_the_terminal_and_scratchpad_commands() {
    assert_eq!(
        ptrack_app::allowed_terminal_window_commands(),
        [
            "ClaimTerminalStream",
            "CloseTerminalV2",
            "CreateTerminalV2",
            "GetPreferences",
            "GetScratchpadV1",
            "GetTerminalProfiles",
            "GetTerminalWindowTab",
            "GetWorkspaceState",
            "ResizeTerminalV2",
            "SetPreferences",
            "SetScratchpadV1",
            "SetTerminalWindowTab",
        ]
    );
    for refused in [
        "MutateTerminalAssociationV2",
        "WriteTerminalMemoryV2",
        "OpenTerminalWindow",
    ] {
        assert!(!ptrack_app::allowed_terminal_window_commands().contains(&refused));
    }
    // A scratchpad write is announced to every window, not only the main one:
    // the dock and each terminal window re-read the same project scratchpad.
    let source = shell_source();
    assert!(source.contains(
        "DesktopEvent::ScratchpadChanged(change) => self.app.emit(\"scratchpad:changed\", change)"
    ));
    assert!(!source.contains("emit_to(MAIN_WINDOW_LABEL, \"scratchpad:changed\""));
}

/// Windows checks the tree out with CRLF. A scan spanning a newline finds
/// nothing there, and the empty slice it falls back to satisfies every claim
/// made against it — which is exactly how the builder claims above passed on
/// Unix and failed on Windows. Reading the same source with CRLF endings must
/// produce the same findings.
#[test]
fn source_scans_survive_a_crlf_checkout() {
    let checkout = env::temp_dir().join(format!("ptrack-crlf-{}-main.rs", std::process::id()));
    fs::write(&checkout, shell_source().replace('\n', "\r\n"))
        .expect("a CRLF copy of the shell source should be writable");
    let source = read_text(&checkout);
    let found = terminal_window_builder(&source).contains("WebviewWindowBuilder::new(");
    fs::remove_file(&checkout).ok();
    assert!(found, "a CRLF checkout must scan the same as a LF one");
}

#[test]
fn tauri_uses_the_existing_frontend_and_exact_window_contract() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = read_json(&manifest_dir.join("tauri.conf.json"));
    let windows_icon = fs::read(manifest_dir.join("icons/icon.ico"))
        .expect("Windows builds require the Tauri application icon");

    assert_eq!(config["build"]["beforeDevCommand"], "npm run dev");
    assert_eq!(config["build"]["beforeBuildCommand"], "npm run build");
    assert_eq!(config["build"]["devUrl"], "http://localhost:5173");
    assert_eq!(config["build"]["frontendDist"], "../frontend/dist");
    assert_eq!(
        config["app"]["windows"]
            .as_array()
            .expect("desktop windows must be an array")
            .len(),
        1
    );
    assert_eq!(config["app"]["windows"][0]["label"], "main");
    assert_eq!(config["app"]["windows"][0]["width"], 1440);
    assert_eq!(config["app"]["windows"][0]["height"], 900);
    assert_eq!(config["app"]["windows"][0]["minWidth"], 880);
    assert_eq!(config["app"]["windows"][0]["minHeight"], 560);
    assert_eq!(
        config["app"]["windows"][0]["title"],
        "p-track Project Workspace"
    );
    assert_eq!(config["app"]["windows"][0]["backgroundColor"], "#080d12");
    // The web layout owns the top 44px drag strip. Keep the native macOS
    // controls inset into it instead of restoring a detached legacy titlebar.
    assert_eq!(config["app"]["windows"][0]["decorations"], true);
    assert_eq!(config["app"]["windows"][0]["titleBarStyle"], "Overlay");
    assert_eq!(config["app"]["windows"][0]["hiddenTitle"], true);
    assert_eq!(
        config["app"]["windows"][0]["trafficLightPosition"],
        serde_json::json!({ "x": 16, "y": 17 })
    );
    // Hidden at launch so the restored geometry is the first painted rect; the
    // shell shows the window in its setup once the replay has run.
    assert_eq!(config["app"]["windows"][0]["visible"], false);
    let source = shell_source();
    let restore = source
        .find("restore_window_state(&window, &capture.version);")
        .expect("setup must replay the stored window geometry");
    let show = source
        .find("let _ = window.show();")
        .expect("setup must show the hidden window");
    let runtime = source
        .find("production_desktop_runtime_for_startup(")
        .expect("setup must build the desktop runtime");
    // The show is unconditional: it precedes every fallible step of setup, so no
    // `?` can leave a permanently invisible window.
    assert!(restore < show && show < runtime);
    // A setup error must never unwind out of the platform's nounwind launch
    // callback — tauri turns a setup Err into exactly that, an abort() with no
    // message anywhere. Failures are recorded and said out loud instead.
    let failure = source
        .find("Err(error) => fail_startup(app.handle(), &error)")
        .expect("setup must route failures through fail_startup, never Err");
    assert!(runtime < failure);
    assert!(
        source.contains("record_startup_failure(&info.to_string())"),
        "a startup panic must leave evidence before the default hook"
    );
    assert_eq!(config["bundle"]["macOS"]["minimumSystemVersion"], "12.0");
    assert_eq!(
        config["bundle"]["macOS"]["entitlements"],
        "../build/darwin/entitlements.plist"
    );
    let csp = config["app"]["security"]["csp"]
        .as_str()
        .expect("the desktop CSP must be configured");
    // The terminal stream binds the IPv4 loopback address and names it in
    // every stream URL, so no other websocket origin is needed.
    assert!(csp.contains("ws://127.0.0.1:*"));
    assert!(!csp.contains("ws://localhost"));
    // Tauri hashes the one inline theme script in index.html into
    // `script-src` at build time, so inline script needs no blanket allowance.
    let script_src = csp
        .split(';')
        .map(str::trim)
        .find(|directive| directive.starts_with("script-src"))
        .expect("the desktop CSP must restrict scripts");
    assert_eq!(script_src, "script-src 'self'");
    assert!(
        !config["app"]["security"]
            .as_object()
            .is_some_and(|security| security.contains_key("dangerousDisableAssetCspModification")),
        "Tauri must keep injecting the inline script hash"
    );
    assert_eq!(
        config["app"]["security"]["capabilities"],
        Value::Array(vec![Value::String("main-window".into())])
    );
    assert_eq!(windows_icon.get(..4), Some([0, 0, 1, 0].as_slice()));
}

#[test]
fn external_url_gate_rejects_non_web_credentialed_and_unlisted_hosts() {
    let source = shell_source();
    assert!(source.contains("matches!(parsed.scheme(), \"http\" | \"https\")"));
    assert!(source.contains("parsed.username().is_empty()"));
    assert!(source.contains("parsed.password().is_some()"));
    // The browser opens only the hosts the help system maps its destinations
    // to — the docs site and the issue tracker — and refuses everything else.
    assert!(
        source.contains(
            "const EXTERNAL_URL_HOSTS: [&str; 2] = [\"github.com\", \"ro-ag.github.io\"]"
        )
    );
    assert!(source.contains("EXTERNAL_URL_HOSTS.contains(&host)"));
}

/// The directory picker and the external browser are main-window actions: a
/// popped-out terminal window reaches neither from its script.
#[test]
fn native_window_actions_are_main_window_only() {
    let source = shell_source();
    assert_eq!(
        source
            .matches("require_main_window_label(window.label())?")
            .count(),
        3,
        "every native window action must be main-window scoped"
    );
    // The Windows title bar's app menu is the third: it opens only over the
    // main window, before the menu is even looked up.
    let menu_arm = source
        .split_once("\"menu\" => {")
        .map(|(_, rest)| rest.trim_start())
        .expect("the window chrome menu action must be findable");
    assert!(menu_arm.starts_with("require_main_window_label(window.label())?;"));
    // The guard runs first in each command body, before anything else can act.
    for signature in ["async fn pick_project_directory(", "fn open_external_url("] {
        let body = source
            .split_once(signature)
            .and_then(|(_, rest)| rest.split_once("{\n"))
            .map_or_else(|| panic!("{signature} must be findable"), |(_, body)| body);
        assert!(
            body.starts_with("    require_main_window_label(window.label())?;"),
            "{signature} must gate on the calling window first"
        );
    }
}

/// Pins [`scope_request_to_window`]'s appearance-only rule for terminal-window
/// `SetPreferences` patches.
#[test]
fn terminal_window_preference_patches_touch_appearance_only() {
    let scoped = |patch: Value| {
        ptrack_app::scope_request_to_window(
            "terminal-1",
            ptrack_app::DesktopCommandRequest {
                method: "SetPreferences".to_owned(),
                arguments: vec![patch],
            },
        )
    };
    assert!(scoped(serde_json::json!({ "appearance": { "theme": "dark" } })).is_ok());
    assert!(scoped(serde_json::json!({ "appearance": { "density": "compact" } })).is_ok());
    for refused in [
        serde_json::json!({ "startup": { "restoreLastProject": true } }),
        serde_json::json!({ "startup": { "lastProjectRoot": "/tmp" } }),
        serde_json::json!({ "notifications": { "handoffArrival": true } }),
        serde_json::json!({ "appearance": { "theme": "dark" }, "startup": { "lastProjectRoot": "/tmp" } }),
        serde_json::json!({ "appearance": { "fontSize": 20 } }),
        serde_json::json!({ "appearance": "dark" }),
        serde_json::json!("dark"),
    ] {
        assert!(scoped(refused.clone()).is_err(), "{refused}");
    }
    // The main window keeps the whole record.
    for patch in [
        serde_json::json!({ "startup": { "restoreLastProject": true } }),
        serde_json::json!({ "notifications": { "runCompletion": true } }),
        serde_json::json!({ "appearance": { "theme": "dark" } }),
    ] {
        assert!(
            ptrack_app::scope_request_to_window(
                "main",
                ptrack_app::DesktopCommandRequest {
                    method: "SetPreferences".to_owned(),
                    arguments: vec![patch],
                },
            )
            .is_ok()
        );
    }
}

#[test]
fn menu_event_and_help_allowlists_are_exact() {
    for event in [
        "workspace:open-requested",
        "workspace:switch-requested",
        "workspace:close-requested",
        "workspace:settings-requested",
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
    assert!(!window.visible);
}
