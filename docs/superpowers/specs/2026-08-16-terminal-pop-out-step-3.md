# Terminal pop-out, step 3: whole tabs, splits, and synchronized state

Status: implementation contract for plan #15 task #139 (remainder) and #142, plus the
child-window part of #143. Extends `2026-08-15-terminal-pop-out-window-contract.md` and
`2026-08-15-terminal-pop-out-step-2.md`; every decision there stays binding except the
single-pane restriction (§7 of step 2), which this step supersedes.

**This step must ship a feature the user can operate.** Popping out a tab moves the whole
tab — its split tree, all its running sessions, its title and its association — into the
terminal window, and closing that window brings the whole tab back. State the user can see
in both windows (theme, profiles, settings, association labels) means the same thing in
both.

## 1. Transfer unit

The unit of movement is a **tab**, not a pane. The pop-out control moves from the pane to
the tab and is present for any tab whose panes are all running with sessions; step 2's
single-pane gate disappears rather than surviving as a special case. A tab with one pane is
simply the smallest tab.

The terminal window renders the tab's split tree with the same split renderer the main
window uses. It hosts exactly one tab and no tab bar: it cannot create, close, or receive
another tab. Splitting an existing pane inside the terminal window is allowed — the split
tree is the tab's own state and moves back with it.

## 2. Assignment

The in-memory assignment map (label → session) becomes label → **the tab's sessions**, in
pane order, together with the serialized tab shape (split tree, ratios, titles, association
pointer, per-pane recorded sequences). Still per run, still never persisted; a restarted
app opens with no terminal windows. `expire` and `drain` semantics are unchanged — they now
yield every session of every expired window.

`OpenTerminalWindow` takes the session list plus the serialized tab shape and returns the
label. `GetTerminalWindowSession` becomes `GetTerminalWindowTab` returning the shape (or
`null` for a stale label). `ClaimTerminalStream` is unchanged — the window claims once per
pane.

## 3. The move

Pop out, in order:

1. The main window releases every pane's lease, recording each pane's last rendered
   sequence, and tears its renderers down. Every PTY keeps running.
2. `OpenTerminalWindow(sessions, shape)` records the assignment and opens the window.
3. The window claims a ticket per pane from its recorded sequence and attaches each. Gap
   notices are per pane.

Failure is **all-or-nothing at the tab level**: if the open fails, or the window fails any
claim, the whole tab returns — the window closes itself (or never opened), the assignment
is cleared, and the main window re-claims every session into the tab it was about to
leave. A partial move — some panes in each window — must be unrepresentable, not merely
avoided. The step-2 rule stands: no failure may leave any session with no owner.

The tab left behind in the main window is a **held tab**: it renders the step-2 notice,
holds its place in the tab strip, and cannot be closed while held (the step-2 refusal
copy applies). Its panes have no sessions; the shape lives in the assignment.

Pop in on window destroy returns the tab exactly as the assignment last knew it —
including splits created or resized inside the terminal window. The main window re-claims
each session from its recorded sequence.

## 4. Synchronized state (#142)

Windows still never talk to each other; the shared runtime is the only truth. Two flows:

- **Into terminal windows.** Theme (already per-window since step 2), terminal profile
  settings, unicode/preferences, and association renames reach terminal windows by the
  same broadcast events the main window consumes; the terminal window applies the subset
  that touches sessions it hosts and ignores the rest. No new event permissions — the
  capability file's permission array stays exactly `allow-listen` + `allow-unlisten`.
- **Out of terminal windows.** The per-session surfaces move with the pane and act on the
  shared runtime directly: search, paste guard, writeback preview/apply, diagnostics, and
  shell-integration state all work in the terminal window identically. Project chrome does
  not move: the association **editor**, linked-launch, the board, palette, and dialogs stay
  main-window-only. A terminal window shows its tab's association read-only; editing it is
  a main-window act, and the change arrives by broadcast.

Capability prompts raised by a session in a terminal window surface in the **main** window
(the broker and its UI live there), which is raised the way menu commands already raise
it. The prompt names the terminal window's tab so the user knows which shell asked.

## 5. Child-window shell (#143 share)

Step 2 landed per-window geometry and main-targeted menu commands. This step adds what a
multi-pane child window makes real: pane focus traversal and split-resize keyboard
handling inside the terminal window (same bindings as the main window), correct focus
restoration on pop-out and pop-in, and screen-reader labels on the window ("Terminal —
<tab title>") and its panes. VoiceOver/NVDA/Orca acceptance stays in #149 and is not
claimed here.

## 6. Honest scope

- One tab per terminal window; several terminal windows may each hold one tab.
- No drag-and-drop of tabs between windows — pop out and close are the only moves.
- Capability prompt routing sends the user to the main window rather than prompting in
  place; prompting in place is future work if it ever earns its keep.
- Session-restore (plan #14) still records terminal windows as absent by design.

## 7. Verification

- Rust: multi-session assignment lifecycle, all-or-nothing open (a failed claim closes
  the window and frees every session), expire/drain over multi-session windows, shape
  round-trip including splits changed inside the child window.
- Frontend: tab-level pop-out/pop-in against a fake bridge — success, open-failure, and
  claim-failure paths; held-tab close refusal; per-pane gap notices; broadcast application
  in terminal-window mode.
- Manual, and stated as manual: pop out a two-pane split running output in both, type in
  both windows, resize the split in the child, close the child, confirm both shells alive
  with the resized split intact.
- `make test` green and the Windows VM run for the terminal crates.
