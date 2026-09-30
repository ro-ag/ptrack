# Linux title bar verification

## Linux package acceptance (2026-09-29)

Built unsigned x86-64 AppImage, Debian and RPM candidates for 0.41.4 in the
Ubuntu 22.04 container. Binary checks confirmed the expected version and CPU,
no Nix-store linkage, and no glibc symbol requirement above 2.35.

Actual package installation, CLI/PTY invocation, X11 window creation and
Wayland-session startup passed on Ubuntu 22.04, 24.04 and 26.04, Debian 13,
and Fedora 44. Native packages use GTK Wayland; AppImages use XWayland.
Container checks use software rendering and reject fatal WebKit process errors.
The AppImage repack step removes bundled Wayland libraries after reproducing
and resolving a Mesa/WebKit EGL abort on Fedora 44.

The NixOS 25.11 wrapper used the same AppImage without compiling p-track.
Its real GUI rendered under Xvfb/Openbox, started a shell, and executed
`echo packaged-terminal-ok` successfully. Evidence:
`/tmp/ptrack-packaged-terminal-verified.png`.

Validation: 911 frontend tests, 416 native app/updater/desktop tests, and 11
release-contract tests passed. Typechecking, strict Clippy, actionlint and
ShellCheck passed. Native tests used an isolated `/var/tmp` location because
preexisting `/tmp/.ptrack` state affects project-ancestor checks.

ARM64 build/install testing is configured on native CI runners but was not
executed locally. Physical Intel/AMD/NVIDIA GPU coverage remains manual.
No release tag, signature or publication was created.

## Title bar scope

Scope: the application title bar requested from the supplied Codex screenshot.
The existing p-track workspace and menu actions remain the product reference.

final result: passed

## Visual comparison

- Reference: `/tmp/codex-clipboard-0756826c-fb20-4829-bfc4-7c9b2c4c0725.png`
  (3834 × 2542 pixels; normalized at 50% for inspection).
- Native implementation: `/tmp/ptrack-titlebar-light.png` and
  `/tmp/ptrack-titlebar.png` (1440 × 900, scale 1).
- Focused comparison: `/tmp/ptrack-titlebar-comparison.png`, with the reference
  and implemented title bars together. Reference width was reduced to 1440
  for the layout comparison; font sizes were also inspected at native scale.
- One compact, full-width bar; application menus on the left; native window
  controls on the right. Both light and dark themes render without a duplicate
  GTK menubar. Existing caption icons are reused.
- The source's chat-history buttons and Edit menu are not p-track features;
  the requested styling is applied to p-track's File, Project, View, Help menus.

## Native interactions

Verified against the real Tauri/WebKitGTK application under X11 with Openbox
and an isolated application profile, rather than a browser mock:

- File opens its native submenu below the label.
- Project → Settings dispatches the existing application action.
- Dragging moves the window from (0, 0) to (100, 100).
- Maximize changes 1100 × 700 to 1440 × 900; Restore returns it to its saved
  position and 1100 × 700 size.
- Minimize removes the window from the visible-window list.
- Close exits the application through its normal shutdown handler.

The empty test profile reports unavailable stored preferences in Settings;
this check verifies the menu dispatch, not preference persistence. The Xvfb
display uses software compositing and reports the expected absence of DRI3.
Wayland-specific window-manager behavior and other Linux distributions were
not exercised by this visual check. Release targets remain the existing
Ubuntu-built x86-64 and ARM64 Linux archives.

No unresolved title-bar P0/P1/P2 findings remain within this scope.
