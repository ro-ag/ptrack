# SDD ledger — plan: ptrack task #59 (plan #8 Rust/Tauri rewrite)

Branch: feat/rust-parity-contracts
BASE: 6e7f04ca472d616dac8bdcd5d344df3b649c2fb6

Task 1 (= ptrack #59): Freeze Go v0.21 compatibility contracts + parity acceptance matrix
- Phase A: parallel contract inventory (9 subsystem agents) → inventory-*.md — DONE (9 files, 709 tagged-release contracts: CLI 78, STORE 50, MODEL 40, TUI 58, GUI 125, TERM 106, CAP 93, AGENT/EVNT/INT/HND/HTTP/ASSOC/LCTX 108, GIT/RPT/GDE 51)
- Phase B: synthesize docs/rust-parity-matrix.md — DONE (749 rows: 709 inventory + 26 updater + 14 packaging/release)
- Phase C: coverage audit and task review → fix loop → final review — DONE (five subagent review passes; all 70 tag-to-head changed files reconciled; evidence classifications corrected)

Baseline correction: the task brief makes released tag v0.21.0 (b7727c5) the sole compatibility baseline. The extraction checkout 6e7f04c is four commits later. A full tag diff audit removed GUI-048/GUI-107, rewrote ten mixed GUI/terminal rows, and excludes the post-tag Help Center, expanded menus, runtime-call close fencing, overlay-aware renderer recovery, and accessibility/layout additions.

Review fixes applied: updater recovery skips invalid/wrong-host stages rather than blocking on them; release-note extraction records the workflow's unescaped-version awk regex instead of claiming exact heading equality; updater/release source ranges and partial-test classifications corrected; stale TERM-067 close fencing removed; all manual rows use stable MANUAL-ID procedures.
