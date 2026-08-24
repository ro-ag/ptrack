#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopPlatform {
    MacOs,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuRole {
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Minimize,
    Maximize,
    Fullscreen,
    CloseWindow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuEntrySpec {
    Command {
        id: &'static str,
        label: &'static str,
        macos_accelerator: Option<&'static str>,
    },
    Separator,
    Role(MenuRole),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuSpec {
    pub label: &'static str,
    pub entries: Vec<MenuEntrySpec>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowSpec {
    pub title: &'static str,
    pub background: &'static str,
    pub width: u16,
    pub height: u16,
    pub min_width: u16,
    pub min_height: u16,
    /// The window is configured hidden and shown by the shell once the stored
    /// geometry has been replayed, so the restored rect is the first paint.
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuDispatch {
    Event(&'static str),
    Help(&'static str),
    Ignore,
}

const MENU_EVENTS: [&str; 10] = [
    "update:open-requested",
    "workspace:board-requested",
    "workspace:close-requested",
    "workspace:command-palette-requested",
    "workspace:install-shell-command-requested",
    "workspace:intelligence-requested",
    "workspace:open-requested",
    "workspace:settings-requested",
    "workspace:switch-requested",
    "workspace:terminal-panel-toggle-requested",
];

const HELP_URLS: [(&str, &str); 4] = [
    ("help-center", "https://ro-ag.github.io/ptrack/help/"),
    (
        "keyboard-shortcuts",
        "https://ro-ag.github.io/ptrack/help/reference/shortcuts/",
    ),
    (
        "terminals",
        "https://ro-ag.github.io/ptrack/help/terminals/",
    ),
    ("report-issue", "https://github.com/ro-ag/ptrack/issues/new"),
];

const fn command(
    id: &'static str,
    label: &'static str,
    macos_accelerator: Option<&'static str>,
) -> MenuEntrySpec {
    MenuEntrySpec::Command {
        id,
        label,
        macos_accelerator,
    }
}

#[must_use]
#[allow(clippy::too_many_lines)] // Exact native menu order is a frozen desktop contract.
pub fn menu_spec(platform: DesktopPlatform) -> Vec<MenuSpec> {
    let file = MenuSpec {
        label: "File",
        entries: vec![
            command(
                "workspace:open-requested",
                "Open Project…",
                Some("CmdOrCtrl+O"),
            ),
            command("workspace:switch-requested", "Switch Project…", None),
            MenuEntrySpec::Separator,
            command("workspace:close-requested", "Close Project", None),
        ],
    };
    let mut project_entries = vec![command(
        "workspace:settings-requested",
        "Settings…",
        Some("CmdOrCtrl+,"),
    )];
    if platform == DesktopPlatform::MacOs {
        project_entries.extend([
            MenuEntrySpec::Separator,
            command(
                "workspace:install-shell-command-requested",
                "Install 'ptrack' Shell Command…",
                None,
            ),
        ]);
    }
    let project = MenuSpec {
        label: "Project",
        entries: project_entries,
    };
    let view = MenuSpec {
        label: "View",
        entries: vec![
            command("workspace:board-requested", "Board", Some("CmdOrCtrl+1")),
            command(
                "workspace:intelligence-requested",
                "Intelligence",
                Some("CmdOrCtrl+2"),
            ),
            MenuEntrySpec::Separator,
            command(
                "workspace:terminal-panel-toggle-requested",
                "Toggle Terminal Panel",
                None,
            ),
            command(
                "workspace:command-palette-requested",
                "Command Palette…",
                None,
            ),
        ],
    };
    let help = MenuSpec {
        label: "Help",
        entries: vec![
            command("help:help-center", "Help Center", None),
            command("help:keyboard-shortcuts", "Keyboard Shortcuts", None),
            MenuEntrySpec::Separator,
            command("update:open-requested", "Check for Updates…", None),
            command("help:report-issue", "Report Issue", None),
        ],
    };
    if platform == DesktopPlatform::Other {
        let mut menus = vec![file, project, view, help];
        for menu in &mut menus {
            for entry in &mut menu.entries {
                if let MenuEntrySpec::Command {
                    macos_accelerator, ..
                } = entry
                {
                    *macos_accelerator = None;
                }
            }
        }
        return menus;
    }
    vec![
        MenuSpec {
            label: "p-track",
            entries: vec![
                MenuEntrySpec::Role(MenuRole::About),
                MenuEntrySpec::Separator,
                MenuEntrySpec::Role(MenuRole::Services),
                MenuEntrySpec::Separator,
                MenuEntrySpec::Role(MenuRole::Hide),
                MenuEntrySpec::Role(MenuRole::HideOthers),
                MenuEntrySpec::Role(MenuRole::ShowAll),
                MenuEntrySpec::Separator,
                MenuEntrySpec::Role(MenuRole::Quit),
            ],
        },
        file,
        project,
        MenuSpec {
            label: "Edit",
            entries: vec![
                MenuEntrySpec::Role(MenuRole::Cut),
                MenuEntrySpec::Role(MenuRole::Copy),
                MenuEntrySpec::Role(MenuRole::Paste),
                MenuEntrySpec::Role(MenuRole::SelectAll),
            ],
        },
        view,
        MenuSpec {
            label: "Window",
            entries: vec![
                MenuEntrySpec::Role(MenuRole::Minimize),
                MenuEntrySpec::Role(MenuRole::Maximize),
                MenuEntrySpec::Role(MenuRole::Fullscreen),
                MenuEntrySpec::Role(MenuRole::CloseWindow),
            ],
        },
        help,
    ]
}

#[must_use]
pub const fn window_spec() -> WindowSpec {
    WindowSpec {
        title: "p-track Project Workspace",
        background: "#080d12",
        width: 1_440,
        height: 900,
        min_width: 880,
        min_height: 560,
        visible: false,
    }
}

#[must_use]
pub fn menu_dispatch(id: &str) -> MenuDispatch {
    if let Some(event) = MENU_EVENTS.iter().find(|event| **event == id) {
        return MenuDispatch::Event(event);
    }
    if let Some(destination) = id.strip_prefix("help:")
        && let Some((_, url)) = HELP_URLS.iter().find(|(name, _)| *name == destination)
    {
        return MenuDispatch::Help(url);
    }
    MenuDispatch::Ignore
}
