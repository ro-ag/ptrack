use super::{
    DesktopPlatform, MenuDispatch, MenuEntrySpec, MenuRole, menu_dispatch, menu_spec, window_spec,
};

#[test]
fn platform_menu_specs_pin_order_roles_and_macos_only_accelerators() {
    let macos = menu_spec(DesktopPlatform::MacOs);
    assert_eq!(
        macos.iter().map(|menu| menu.label).collect::<Vec<_>>(),
        [
            "p-track", "File", "Project", "Edit", "View", "Window", "Help"
        ]
    );
    assert_eq!(
        macos[0].entries,
        [
            MenuEntrySpec::Role(MenuRole::About),
            MenuEntrySpec::Separator,
            MenuEntrySpec::Role(MenuRole::Services),
            MenuEntrySpec::Separator,
            MenuEntrySpec::Role(MenuRole::Hide),
            MenuEntrySpec::Role(MenuRole::HideOthers),
            MenuEntrySpec::Role(MenuRole::ShowAll),
            MenuEntrySpec::Separator,
            MenuEntrySpec::Role(MenuRole::Quit),
        ]
    );
    assert!(matches!(
        &macos[4].entries[0],
        MenuEntrySpec::Command {
            label: "Overview",
            ..
        }
    ));
    let other = menu_spec(DesktopPlatform::Other);
    assert_eq!(
        other.iter().map(|menu| menu.label).collect::<Vec<_>>(),
        ["File", "Project", "View", "Help"]
    );
    assert!(matches!(
        &other[2].entries[0],
        MenuEntrySpec::Command {
            label: "Overview",
            ..
        }
    ));
    assert!(other.iter().flat_map(|menu| &menu.entries).all(|entry| {
        matches!(
            entry,
            MenuEntrySpec::Command {
                macos_accelerator: None,
                ..
            }
        ) || matches!(entry, MenuEntrySpec::Separator)
    }));
    assert!(!other[1].entries.iter().any(|entry| matches!(
        entry,
        MenuEntrySpec::Command {
            id: "workspace:install-shell-command-requested",
            ..
        }
    )));
}

#[test]
fn dispatch_and_window_contracts_are_exact() {
    let events = [
        "update:open-requested",
        "workspace:board-requested",
        "workspace:close-requested",
        "workspace:command-palette-requested",
        "workspace:install-shell-command-requested",
        "workspace:intelligence-requested",
        "workspace:issues-requested",
        "workspace:open-requested",
        "workspace:settings-requested",
        "workspace:switch-requested",
        "workspace:terminal-panel-toggle-requested",
    ];
    for event in events {
        assert_eq!(menu_dispatch(event), MenuDispatch::Event(event));
    }
    assert_eq!(
        menu_dispatch("help:help-center"),
        MenuDispatch::Help("https://ro-ag.github.io/ptrack/help/")
    );
    assert_eq!(menu_dispatch("help:unknown"), MenuDispatch::Ignore);
    let window = window_spec();
    assert_eq!(
        (
            window.title,
            window.background,
            window.width,
            window.height,
            window.min_width,
            window.min_height,
            window.visible
        ),
        (
            "p-track Project Workspace",
            "#080d12",
            1_440,
            900,
            880,
            560,
            false
        )
    );
}
