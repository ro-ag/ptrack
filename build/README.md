# Build assets

This directory holds the native packaging assets consumed by the Rust/Tauri
build. Generated application binaries and temporary archive/DMG roots remain
ignored; the launcher, entitlements, and master icon are committed.

| Path | Purpose |
|---|---|
| `appicon.png` | 1024×1024 master icon. Regenerate with `python3 assets/brand/generate_icons.py`; Tauri uses the checked-in exports under `assets/brand/`. |
| `darwin/launcher` | Frozen Finder entry point. It runs the internal `ptrack gui` binary so direct CLI/TUI invocation remains available. |
| `darwin/entitlements.plist` | Empty hardened-runtime exception set used when signing the inner CLI and app bundle. WKWebView and child PTYs need no unsigned-memory entitlement. |
| `windows/p-track.wxs` | WiX 3.14 source of the per-user MSI: `%LOCALAPPDATA%\Programs\p-track`, user `PATH`, Start menu entry, HKCU key paths only, never elevated. Its UpgradeCode and component GUIDs are frozen. |
| `windows/package.ps1` | Builds the MSI (with a pinned, digest-checked WiX download when none is supplied), verifies it is per-user and unelevated through Windows Installer, and writes the portable ZIP. |
| `dmg/`, `archive/` | Temporary package roots. Ignored and removed after packaging. |

The release workflow and `Makefile` both build the Tauri bundle, install the
launcher as `p-track.app/Contents/MacOS/p-track`, retain the combined CLI/GUI
binary as `Contents/MacOS/ptrack`, and stamp one release version into the CLI,
desktop state, updater, About metadata, and bundle plist before signing.

On Windows the release builds `ptrack.exe` with Tauri and the console-free
`p-track.exe` launcher from `crates/ptrack-launcher`, then packages three
assets per architecture: the frozen CLI ZIP (exactly `ptrack.exe`,
`README.md`, `LICENSE`, which installed updaters require), the per-user MSI,
and the portable ZIP that adds the launcher. To package locally:

`ptrack.exe` must come from `tauri build`: a plain `cargo build` produces a
development binary whose window loads the Vite dev server (`localhost`)
instead of the embedded frontend.

```powershell
npm --prefix frontend ci
npm --prefix frontend run tauri -- build --no-bundle --ci -- --locked
cargo build --locked --release -p ptrack-launcher
./build/windows/package.ps1 -Version 0.41.3 -Arch amd64 -BinDir target/release -OutDir dist
```

Brand sources (icon generator, PNG exports, `.icns`, README banner, and social
card) live in [`assets/brand/`](../assets/brand/).
