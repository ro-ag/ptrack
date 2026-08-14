# Build assets

This directory holds the native packaging assets consumed by the Rust/Tauri
build. Generated application binaries and temporary archive/DMG roots remain
ignored; the launcher, entitlements, and master icon are committed.

| Path | Purpose |
|---|---|
| `appicon.png` | 1024×1024 master icon. Regenerate with `python3 assets/brand/generate_icons.py`; Tauri uses the checked-in exports under `assets/brand/`. |
| `darwin/launcher` | Frozen Finder entry point. It runs the internal `ptrack gui` binary so direct CLI/TUI invocation remains available. |
| `darwin/entitlements.plist` | Empty hardened-runtime exception set used when signing the inner CLI and app bundle. WKWebView and child PTYs need no unsigned-memory entitlement. |
| `dmg/`, `archive/` | Temporary package roots. Ignored and removed after packaging. |

The release workflow and `Makefile` both build the Tauri bundle, install the
launcher as `p-track.app/Contents/MacOS/p-track`, retain the combined CLI/GUI
binary as `Contents/MacOS/ptrack`, and stamp one release version into the CLI,
desktop state, updater, About metadata, and bundle plist before signing.

Brand sources (icon generator, PNG exports, `.icns`, README banner, and social
card) live in [`assets/brand/`](../assets/brand/).
