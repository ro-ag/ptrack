# Build assets

This directory holds the platform packaging assets consumed by `wails build`.
Everything here **is committed** except `build/bin/` (build output) — see the
`/build/*` negation rules in `.gitignore`.

| Path | Purpose |
|---|---|
| `appicon.png` | 1024×1024 master app icon. Regenerate with `python3 assets/brand/generate_icons.py`; Wails compiles it into the bundle's `iconfile.icns`. |
| `darwin/Info.plist` | Production bundle metadata: bundle id `com.ro-ag.ptrack`, macOS 12.0 minimum, developer-tools category. Wails expands the `{{...}}` template fields from `wails.json`. |
| `darwin/Info.dev.plist` | Bundle metadata used by `wails dev`. |
| `darwin/entitlements.plist` | Hardened-runtime entitlements for signed/notarized releases (unsigned local builds ignore it). |
| `bin/` | Build output. Ignored by git. |

Brand sources (icon generator, PNG exports, `.icns`, README banner, social
card) live in [`assets/brand/`](../assets/brand/).
