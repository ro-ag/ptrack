# Deferred Tauri and Rust architecture

Status: design note only. The current Go/Wails application remains the active
implementation while its embedded terminal is improved.

## Objective

Rebuild the desktop host around Tauri without discarding the existing
HTML/CSS/TypeScript workspace. A Rust host would use `native-ipc` directly and
run native libraries in small, disposable helper processes rather than linking
their unsafe ABI into the main application.

Ghostty is the first intended integration. The design should also form a
careful internal runner pattern for future libraries without prematurely
becoming a public plug-in system.

## Process topology

```text
Tauri WebView
    | commands and ordered channels
    v
Rust p-track host
    |- project/application services
    |- PTY and shell ownership
    |- presentation and renderer ownership
    |- native-ipc coordinator
    `- runner registry
             |
             | authenticated control + directional shared memory
             v
      Rust library runner
             `- thin unsafe adapter -> third-party native library
```

The host owns the PTY. A Ghostty runner receives PTY bytes, parses them through
`libghostty-vt`, encodes keyboard and mouse events, and publishes bounded
renderer-state changes. This keeps the shell alive if the terminal engine
runner fails and lets the runner remain a disposable library host consistent
with the `native-ipc` integration model.

## Workspace shape

```text
frontend/                    existing workspace UI
src-tauri/                   desktop host and command boundary
crates/terminal-protocol/    terminal control and render-state schema
crates/ptrack-runner-sdk/    shared runner lifecycle conventions
crates/ghostty-runner/       libghostty-vt adapter and receiver
```

During migration, the existing Go `ptrack` executable can remain a bundled,
long-lived sidecar behind a bounded JSON protocol. This avoids cgo and avoids a
big-bang rewrite of the project model, storage, CLI, and Git integrations.
Services should move to Rust only when the move has a measured benefit.

## Ghostty boundary

The first integration targets `libghostty-vt`, not an assumed embeddable
Ghostty GUI widget. The library supplies terminal parsing, Unicode behavior,
input encoding, terminal state, and renderer state; p-track still owns drawing,
window integration, selection UI, search UI, clipboard policy, IME, and
accessibility.

Use native-ipc control records for lifecycle, resize, focus, input, and
configuration. Use direction-specific shared-memory rings for PTY streams and
render-state snapshots or damage records. Do not make full RGBA frame transfer
the final design: semantic cell/render deltas retain more information and avoid
continuous framebuffer copies.

If a future renderer needs GPU surfaces produced in the runner, add explicit
platform capability support rather than disguising a GPU handle as portable
shared memory. IOSurface/Mach rights, dma-buf file descriptors, and Windows
shared graphics handles have different lifetime and authority rules.

## Reusable runner conventions

Generalize only after Ghostty proves the boundary. A later internal runner SDK
may standardize:

- protocol and adapter versions;
- declared regions, directions, limits, and capabilities;
- startup negotiation and compatibility rejection;
- deadlines, health reporting, shutdown, and restart policy;
- bounded diagnostics with no host secrets in shared payloads;
- signing, packaging, and exact executable identity; and
- platform-specific resource and sandbox policy.

Each native library still requires a small purpose-built Rust adapter and a
versioned domain protocol. `native-ipc` supplies the safe process and memory
boundary; it does not make arbitrary ABIs or native UI surfaces portable.

## Migration sequence

1. Run the current frontend unchanged inside a minimal Tauri shell.
2. Put Wails and Tauri implementations behind one TypeScript backend adapter.
3. Keep the Go core as a sidecar while matching current desktop behavior.
4. Add a Rust PTY host and a `native-ipc` Ghostty runner.
5. Add an `xterm | Ghostty` terminal-engine setting and retain xterm fallback.
6. Implement and measure Ghostty render-state presentation, Unicode, input,
   images, sustained output, resize, sleep/wake, and recovery behavior.
7. Port remaining Go services selectively after terminal parity is proven.

## Stability boundaries

Keep both changing dependencies behind narrow adapters: the `native-ipc`
vNext session API is pre-1.0, and the libghostty API is not yet versioned.
Protocol fixtures and compatibility tests should protect p-track from changes
on either side. The current Go/Wails build remains the production fallback
until the Tauri host passes the complete desktop and terminal acceptance
matrix.
