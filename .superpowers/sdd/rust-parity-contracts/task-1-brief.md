# Task brief — ptrack #59: Freeze the Go v0.21 compatibility contracts and record a full parity acceptance matrix

## Context

p-track is a Go/Wails desktop app + CLI (current release: v0.21.0, tag `v0.21.0`, CHANGELOG entry dated 2026-08-11).
Plan #8 ("Rust/Tauri rewrite with full current-feature parity") will rebuild the app in Rust/Tauri.
Before any Rust code is written, the externally observable behavior of the Go v0.21 app must be
frozen into written compatibility contracts, and every contract must be mapped into a parity
acceptance matrix that the Rust rewrite will later be validated against.

This task produces documentation only. No Go/frontend code changes. No new dependencies.

## Deliverable

A new doc `docs/rust-parity-matrix.md` (single file) that contains:

1. **Scope and method** — what v0.21.0 is (commit/tag), what "parity" means (externally observable
   behavior: bytes on disk, stdout/stderr, exit codes, IPC/DTO shapes, UI behavior, security
   properties), and what is explicitly out of scope (internal Go APIs, code structure).
2. **Compatibility contracts**, organized by subsystem:
   - CLI command surface: every command/subcommand, flags, positional args, output formats
     (human + any machine formats), exit-code conventions, non-interactive guarantees.
   - Storage: bbolt database file location(s), bucket schema, gob-encoded value formats,
     versioning/migrations, backup/atomic-write behavior, concurrency/locking guarantees.
   - Domain model invariants: goal → milestone → plan → task → issue/note hierarchy, statuses,
     ID allocation, validation rules that are observable through CLI/GUI.
   - TUI: screens, keybindings, navigation, edit flows (observable behavior level).
   - GUI bridge: every Wails-bound method (name, argument types, return types), emitted events,
     and the frontend's expectations (wailsjs generated surface as the contract source).
   - Terminal: PTY session lifecycle, profiles, shell integration, scrollback/stream bounds,
     security properties of the stream, resilience/recovery behavior.
   - Capabilities: deny-by-default HTTP/Git/SSH grants, audit records, approval flows.
   - Agent coordination: agent runs, associations, launch context (bounds/redaction), handoffs,
     drift detection, notifications, workflow proposals.
   - Git intelligence: repo/worktree detection, commit tracking behavior, subprocess bounds.
   - Updater: discovery, release manifest/checksum verification, staging, platform handoff,
     recovery; documented trust boundary (SHA-256 from co-hosted manifest is not a publisher
     signature).
   - Packaging/release: produced assets per platform, signing/notarization, CLI shell-command
     installation, version reporting.
3. **Parity acceptance matrix** — a table with one row per contract: ID, subsystem, contract
   summary, verification method (existing automated test / fixture / new test needed / manual
   acceptance), and source reference (file:line or test name in the Go tree).
4. **Acceptance gate definition** — what "parity achieved" means for plan #8: every matrix row
   verified on every supported platform/arch (macOS arm64, Linux, Windows per repo workflows),
   or explicitly waived with rationale.

## Sources of truth

- Nine inventory files at `.superpowers/sdd/rust-parity-contracts/inventory-*.md`, produced by
  exploration agents directly from the v0.21.0 codebase. Treat them as the raw material; verify
  surprising claims against the code before baking them into the contract.
- `docs/tauri-rust-recode.md` — the deferred design note (context only; do not treat its Go-sidecar
  sequence as current — task #60 will replace it).
- `docs/updater-acceptance.md`, `docs/terminal-acceptance.md`, `docs/updater-security.md`,
  `docs/ptrack-threat-model.md` — existing acceptance/security docs to align with, not duplicate.

## Style

- Follow the tone and structure of the existing docs in `docs/` (terse, declarative, no marketing).
- Contract IDs: stable, namespaced per subsystem (e.g. `CLI-001`, `STORE-003`), so later plan #8
  tasks can reference them.
- Every contract must be checkable: if it cannot be verified by a test, fixture, or defined manual
  procedure, it does not belong in the matrix.
- Do not invent behavior: if the inventories disagree with each other or with the code, check the
  code and record what the code actually does.
