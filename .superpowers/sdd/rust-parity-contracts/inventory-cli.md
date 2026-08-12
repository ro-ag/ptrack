# Inventory: CLI command surface (Go v0.21.0)

Scope: `internal/cli/*` + `main.go`. Binary name `ptrack`. Built with spf13/cobra.
Empirically spot-checked against installed binary (v0.20.0; behavior matches v0.21.0 source).

## Contracts

### Process-level conventions

- CLI-001: Every subcommand is non-interactive (no prompts, no stdin reads except `capability mcp`); success exits 0, any error exits exactly 1 — src: internal/cli/open.go:1-3 (package doc), main.go:44-47 — pinned by: none (asserted in docs; exercised by all tests) — verify: automated test
- CLI-002: All errors are printed to **stderr only**, as the bare error message followed by `\n`, with no usage text and no `Error:` prefix (cobra `SilenceErrors`/`SilenceUsage` set recursively on the whole tree; main.go does `fmt.Fprintln(os.Stderr, err); os.Exit(1)`) — src: main.go:44-47, internal/cli/root.go:40,67-79 — pinned by: none — verify: automated test (run any failing command, assert stderr/stdout/exit)
- CLI-003: All normal output goes to **stdout** via `cmd.OutOrStdout()`; nothing writes to stderr except errors (and `commit show`'s passthrough of git's stderr, CLI-058) — src: every command file (e.g. internal/cli/task.go:49), main.go:45 — pinned by: internal/cli/cli_test.go runCmd (captures out/err separately) — verify: automated test
- CLI-004: Unknown command produces cobra's `unknown command "<x>" for "ptrack"` plus `Did you mean this?` suggestion block on stderr, exit 1 — src: internal/cli/root.go:28-41 (no custom unknown-command handler; cobra default) — pinned by: none — verify: automated test
- CLI-005: Argument-count violations produce cobra's standard messages (e.g. `accepts 1 arg(s), received 0`, `requires at least 1 arg(s), only received 0`, `unknown command ... for "ptrack"`) on stderr, exit 1, no usage text — src: `Args:` validators throughout (e.g. internal/cli/task.go:24,115) + internal/cli/root.go:67-79 — pinned by: none — verify: automated test
- CLI-006: `-h`/`--help` prints cobra-generated help to stdout, exit 0; root also gets `-v`/`--version` printing `ptrack version <v>` (cobra default from `root.Version`) — src: internal/cli/root.go:29-41 — pinned by: none — verify: automated test
- CLI-007: Cobra's implicit `completion` (bash/zsh/fish/powershell) and `help` commands exist and appear in root help output; they are NOT in the pinned subcommand list — src: internal/cli/root.go:43-68 (no `CompletionOptions` disable) — pinned by: none — verify: automated test
- CLI-008: Root command set is exactly these 21 named subcommands: init, goal, summary, milestone (alias ms), plan, task, issue, note, commit, hook, context, guide, next, search, board, gui, status, projects, backup, capability, version — src: internal/cli/root.go:43-65 — pinned by: internal/cli/cli_test.go TestRootHasSubcommands (checks 16; exact set not asserted) — verify: automated test
- CLI-009: Root help Long text: `p-track keeps project plans alive across human and AI sessions. It stores\ngoals, plans, tasks, issues, milestones, notes, and commit context in an embedded\nbbolt database so a fresh agent can reload project context. Every subcommand is\nnon-interactive and exits non-zero on error.`; Short: `p-track keeps project plans alive across human and AI sessions` — src: internal/cli/root.go:29-35 — pinned by: none — verify: fixture
- CLI-010: `ptrack` with no subcommand: desktop build (main.go) launches the TUI; if no project found it prints the NoProjectHint text to **stdout** and exits 0 (not an error) — src: main.go:36-43 — pinned by: none (TUI path untestable in cli package) — verify: manual
- CLI-011: NoProjectHint exact text: `p-track  ·  persistent project memory\n──────────────────────────────────────\n\nNo p-track project here yet.\n\nGET STARTED\n  ptrack init                 create one in this directory (or the git root)\n  ptrack init --goal "..."     create one and set the goal\n  ptrack --help               browse all commands\n\nOnce a project exists, run `ptrack` to open the dashboard.\n` — src: internal/cli/hint.go:5-14 — pinned by: internal/cli/hint_test.go TestNoProjectHint (substring checks only) — verify: fixture
- CLI-012: Non-desktop fallback for no-args prints `ptrack: nothing to do. Run 'ptrack --help' for commands or 'ptrack status' for an overview.\n` to stdout, exit 0 — src: internal/cli/root.go:22-25 — pinned by: none — verify: manual
- CLI-013: Version resolution order: link-time `-X .../cli.Version` value, else Go module build-info version (unless empty/`(devel)`), else literal `dev` — src: internal/cli/version.go:13-27 — pinned by: none — verify: manual
- CLI-014: `ptrack version` prints `ptrack <resolved-version>\n` to stdout, exit 0; `ptrack --version` prints cobra's `ptrack version <v>\n` — src: internal/cli/version.go:34-43, root.go:39 — pinned by: none — verify: automated test
- CLI-015: Every command that opens a project walks up from cwd looking for `.ptrack/ptrack.db`; when none is found the error text is exactly `no ptrack project found (run 'ptrack init')` (stderr, exit 1) — src: internal/cli/open.go:17-35, internal/store/discovery.go:11 — pinned by: none — verify: automated test
- CLI-016: Every successful project-scoped command best-effort registers the project in the global registry (name = basename of project root, path, LastSeen refreshed); registry failures are silently ignored and never change exit code — src: internal/cli/open.go:13-46 — pinned by: none — verify: automated test
- CLI-017: Global home (registry, backups, guide template, broker state) honors the `PTRACK_HOME` env override — src: internal/cli/cli_test.go:44, cli_v2_test.go:16 (all tests isolate via `PTRACK_HOME`), store.GlobalHome — pinned by: every cli test via t.Setenv — verify: automated test

### Output-format conventions

- CLI-018: `--json` is the uniform machine-format flag on all report/list commands, registered as `BoolVar "json"` default false with usage `emit JSON instead of Markdown` — src: internal/cli/output.go:33-35 — pinned by: none — verify: automated test
- CLI-019: JSON emission is `json.MarshalIndent(v, "", "  ")` (2-space indent) plus a single trailing `\n`, to stdout — src: internal/cli/output.go:23-30 — pinned by: none — verify: automated test
- CLI-020: Default (non-JSON) format for report views (`context`, `next`, `search`, `board`, `task show`, `plan show`, `milestone show`, `issue show`) is the view's Markdown rendering written with no extra newline added — src: internal/cli/output.go:10-20 — pinned by: internal/cli/cli_v2_test.go TestBoardCommand (one Board substring only; other views and newline behavior unpinned) — verify: automated test
- CLI-021: Go's `encoding/json` semantics apply to JSON output: field order as declared, `omitempty` honored, HTML escaping of `<>&` in strings — src: internal/cli/output.go:24 — pinned by: none — verify: automated test

### init

- CLI-022: `ptrack init` (no positional args; flags `--goal <text>`, `--root <dir>`, `--force`, `--no-guide`) creates `.ptrack/ptrack.db` at explicit `--root` when provided; otherwise at the enclosing git root, falling back to cwd. It prints the absolute db path as a bare line on stdout. — src: internal/cli/init.go:17-82, internal/store/discovery.go:49-92 (print at init.go:73) — pinned by: internal/cli/cli_test.go TestIntegrationFlow (default path only; `--root` precedence unpinned) — verify: automated test
- CLI-023: Re-running `init` inside the same project does NOT error: prints `project already initialized at <dbPath>\n`, updates goal if `--goal` given, refreshes guide — src: internal/cli/init.go:45-49,87-101 — pinned by: internal/cli/cli_v2_test.go TestInitSyncsSameProject — verify: automated test
- CLI-024: Running `init` where a *different* ancestor project exists fails with `already inside ptrack project at <root>; run 'ptrack guide' to refresh docs, or 'ptrack init --force' to nest a new project` unless `--force` — src: internal/cli/init.go:50-52 — pinned by: internal/cli/cli_v2_test.go TestInitRefusesGenuineNesting — verify: automated test
- CLI-025: Unless `--no-guide`, init installs the agent guide into `AGENTS.md` and `CLAUDE.md` at the project root, printing `wrote agent guide to <file>\n` per written file, or `agent guide already up to date\n` when nothing changed — src: internal/cli/init.go:105-125 — pinned by: internal/cli/guide_test.go TestInitWritesGuide, TestInitNoGuideSkips — verify: automated test
- CLI-026: Guide content installed by init/guide appends the user's global template from `<global home>/guide.md` when that file exists — src: internal/cli/init.go:109, internal/cli/guide.go:13-29 — pinned by: none — verify: automated test

### goal / summary

- CLI-027: `ptrack goal` with no subcommand behaves as `goal show`: prints the raw goal text + `\n` (empty line when unset). `goal set <text...>` joins args with single spaces and prints nothing on success — src: internal/cli/goal.go:18-56 — pinned by: internal/cli/cli_test.go TestIntegrationFlow (indirect) — verify: automated test
- CLI-028: `ptrack summary` mirrors goal exactly against the Summary field (`summary` bare = `summary show`; `summary set` silent) — src: internal/cli/summary.go:12-55 — pinned by: none — verify: automated test

### milestone (alias: ms)

- CLI-029: `milestone add <title...>` joins args; `--due YYYY-MM-DD` optional; invalid date errors `invalid --due %q (want YYYY-MM-DD): %w`; success prints `milestone #<id> <title>\n` — src: internal/cli/milestone.go:24-55 — pinned by: internal/cli/milestone_issue_test.go TestMilestoneCommands — verify: automated test
- CLI-030: `milestone list` human format per line: `#<id> [<status>] <title>` + ` (due YYYY-MM-DD)` when set + `\n`; `--json` emits raw `[]model.Milestone` (Go JSON tags of the model type, including zero times) — src: internal/cli/milestone.go:58-86 — pinned by: TestMilestoneCommands (substring) — verify: automated test
- CLI-031: `milestone show <id>` → report.ShowMilestone view, Markdown default / `--json` — src: internal/cli/milestone.go:89-110 — pinned by: TestMilestoneCommands — verify: automated test
- CLI-032: `milestone done <id>` / `milestone open <id>` print nothing on success — src: internal/cli/milestone.go:112-123,172-185 — pinned by: TestMilestoneCommands — verify: automated test
- CLI-033: `milestone due <id> <YYYY-MM-DD|->`: `-` clears the date; invalid date errors `invalid date %q (want YYYY-MM-DD): %w`; silent on success — src: internal/cli/milestone.go:125-148 — pinned by: none — verify: automated test
- CLI-034: `milestone rename <id> <title...>` joins args, silent on success — src: internal/cli/milestone.go:150-166 — pinned by: none — verify: automated test

### plan

- CLI-035: `plan add <title...>` joins args; `--milestone <id>` optionally assigns; prints `plan #<id> <title>\n` — src: internal/cli/plan.go:20-43 — pinned by: internal/cli/cli_test.go TestIntegrationFlow — verify: automated test
- CLI-036: `plan list` human format per line: `#<id> [<status>] <mark> <title>\n` where mark is `*` for the active plan, space otherwise; `--json` emits array of `{"id":N,"title":"...","status":"...","active":bool}` — src: internal/cli/plan.go:46-88 — pinned by: internal/cli/milestone_issue_test.go TestRenameCommands — verify: automated test
- CLI-037: `plan show <id>` → report.ShowPlan, Markdown default / `--json` — src: internal/cli/plan.go:91-112 — pinned by: none — verify: automated test
- CLI-038: `plan done <id>`, `plan use <id>` (set active plan), `plan rename <id> <title...>`: all silent on success — src: internal/cli/plan.go:114-166 — pinned by: TestIntegrationFlow, TestRenameCommands — verify: automated test
- CLI-039: All `<id>` arguments across commands parse as base-10 uint64; failure errors `invalid id %q: %w` (wrapped strconv message) — src: internal/cli/plan.go:184-191 — pinned by: internal/cli/cli_test.go TestParseIDInvalid — verify: automated test

### task

- CLI-040: `task add <title...>` joins args; `--plan <id>` optional, defaults to the active plan; with no active plan errors `no active plan; set one with 'ptrack plan use <id>' or pass --plan`; success prints `task #<id> <title> (plan <planID>)\n` — src: internal/cli/task.go:21-53 — pinned by: TestIntegrationFlow — verify: automated test
- CLI-041: `task list` human format per line: `#<id> [<status>] <title> (plan <planID>)\n`; flags `--plan <id>`, `--status <csv>` (comma-separated subset of todo,doing,done,blocked; invalid entry errors `invalid status %q (want todo,doing,done,blocked)`); `--json` emits array of `{"id":N,"plan_id":N,"title":"...","status":"..."}` — src: internal/cli/task.go:59-109,240-256 — pinned by: internal/cli/cli_v2_test.go TestTaskListStatusFilter — verify: automated test
- CLI-042: `task show <id>` → report.ShowTask, Markdown default / `--json` — src: internal/cli/task.go:112-133 — pinned by: none — verify: automated test
- CLI-043: `task start|done|block <id>` set status doing/done/blocked; silent on success — src: internal/cli/task.go:135-160,273-284 — pinned by: internal/cli/cli_v2_test.go seedProject — verify: automated test
- CLI-044: `task rename <id> <title...>` joins args; silent on success — src: internal/cli/task.go:162-178 — pinned by: none — verify: automated test
- CLI-045: `task move <id> --plan <plan>`: missing/zero `--plan` errors `pass the target plan with --plan <id>`; nonexistent target plan fails with store not-found error and leaves the task unmoved; success prints `task #<id> moved to plan <planID>\n` — src: internal/cli/task.go:180-208 — pinned by: internal/cli/task_transform_test.go TestTaskMoveAndConvertCommands, TestTaskMoveRequiresExistingTargetPlan — verify: automated test
- CLI-046: `task convert <id>` (alias `promote`) converts the task into a plan (task is deleted); prints `task #<id> converted to plan #<newPlanID> <title>\n` — src: internal/cli/task.go:210-232 — pinned by: internal/cli/task_transform_test.go TestTaskMoveAndConvertCommands — verify: automated test

### issue

- CLI-047: `issue add <title...>` joins args; flags `--severity low|medium|high|critical` (empty = store default medium), `--task <id>` link, `--body <text>`; invalid severity errors `invalid severity %q (want low, medium, high, critical)`; success prints `issue #<id> [<severity>] <title>\n` — src: internal/cli/issue.go:19-48,186-195 — pinned by: internal/cli/milestone_issue_test.go TestIssueCommands — verify: automated test
- CLI-048: `issue list` human format per line: `#<id> [<severity>] <status> <title>` + ` (task <id>)` when linked + `\n`; `--status open|closed` filters (anything else errors `invalid --status %q (want open or closed)`); `--json` emits raw `[]model.Issue` — src: internal/cli/issue.go:50-102 — pinned by: TestIssueCommands — verify: automated test
- CLI-049: `issue show <id>` → report.ShowIssue, Markdown default / `--json` — src: internal/cli/issue.go:104-126 — pinned by: none — verify: automated test
- CLI-050: `issue close <id>`, `issue open <id>`, `issue severity <id> <low|medium|high|critical>`, `issue rename <id> <title...>`: silent on success; severity validated by same parser as add — src: internal/cli/issue.go:128-179,197-210 — pinned by: TestIssueCommands (close path) — verify: automated test

### note

- CLI-051: `note add <text...>` joins args; `--task <id>` / `--plan <id>` target flags (when both given, --task wins, no error); default target is the project; prints `note #<id> <body>\n` — src: internal/cli/note.go:19-60 — pinned by: internal/cli/cli_v2_test.go TestNoteListCommand — verify: automated test
- CLI-052: `note list`: flags `--plan <id>`, `--task <id>` (both set → error `--plan and --task are mutually exclusive`), `--limit <n>` default 20, 0 = all; output is newest-first reversal of insertion order — src: internal/cli/note.go:62-128,134-143 — pinned by: TestNoteListCommand — verify: automated test
- CLI-053: `note list` human format: `#<id> (<kind> · <target>) <body>\n` for project-scoped, `#<id> (<kind> · <target> #<targetID>) <body>\n` for task/plan-scoped; the `<kind> · ` prefix is omitted entirely for legacy notes with empty kind — src: internal/cli/note.go:110-122 — pinned by: internal/cli/note_test.go TestNoteListLabelsTypedMemoryAndKeepsLegacyShape — verify: automated test
- CLI-054: `note list --json` emits array of `{"id":N,"target":"project|plan|task","target_id":N,"kind":"...","body":"..."}` with `kind` omitted (omitempty) when empty — src: internal/cli/note.go:96-109 — pinned by: TestNoteListLabelsTypedMemoryAndKeepsLegacyShape (asserts exactly one `"kind"` key) — verify: automated test

### commit

- CLI-055: `commit add <sha> <subject...>`: subject args joined; link resolution: `--task` → task's plan + task; else `--plan`; else the active plan; success prints `commit <sha8> recorded\n` where sha8 = first 8 chars (full sha if shorter) — src: internal/cli/commit.go:37-60,200-224 — pinned by: none — verify: automated test
- CLI-056: `commit record` (hook-facing): `--sha` required (error `--sha is required` when empty), `--subject` optional; parses the FIRST `#<digits>` in the subject as a task ref (regex `#(\d+)`); if that task exists, links commit to the task and its plan, else to the active plan; prints NOTHING on success — src: internal/cli/commit.go:14-24,66-98 — pinned by: internal/cli/milestone_issue_test.go TestCommitRecordParsesTaskRef — verify: automated test
- CLI-057: `commit list`: `--task <id>` / `--plan <id>` filters (when both set, --task wins, no error); human format per line: `<sha8> <subject>` + ` (task <id>)` or ` (plan <id>)` (task takes precedence) + `\n`; `--json` emits raw `[]model.Commit` — src: internal/cli/commit.go:100-145 — pinned by: TestCommitRecordParsesTaskRef — verify: automated test
- CLI-058: `commit show <id|sha> [--stat]`: a purely numeric arg matching a tracked commit id is resolved to its SHA; any other arg passes through as a git ref; then executes external `git -C <projectRoot> show [--stat] <ref>` with stdout forwarded to ptrack's stdout and git's stderr to os.Stderr; git's exit failure propagates as exit 1 — src: internal/cli/commit.go:147-198 — pinned by: internal/cli/milestone_issue_test.go TestCommitShowResolvesRef (resolution only) — verify: manual

### hook

- CLI-059: `hook install` writes `<projectRoot>/.git/hooks/post-commit` (mode 0755, dirs created 0755); a fresh file gets `#!/bin/sh\n` + the managed block; the block is exactly `# ptrack:begin\ncommand -v ptrack >/dev/null 2>&1 && ptrack commit record --sha "$(git rev-parse HEAD)" --subject "$(git log -1 --pretty=%s)" >/dev/null 2>&1 || true\n# ptrack:end\n`; existing foreign content is preserved and an existing ptrack block is refreshed in place — src: internal/cli/hook.go:13-17,26-53,128-150 — pinned by: none — verify: fixture
- CLI-060: `hook install` prints `installed post-commit hook at <path>\n` when content changed, else `post-commit hook already up to date\n` — src: internal/cli/hook.go:46-50 — pinned by: none — verify: automated test
- CLI-061: `hook uninstall` removes the ptrack block; deletes the file entirely when only the shebang (or nothing) remains; prints `removed ptrack post-commit hook\n`; when no hook file exists prints `no post-commit hook\n` and exits 0 — src: internal/cli/hook.go:55-84,152-168 — pinned by: none — verify: automated test
- CLI-062: `hook status` prints `installed: <path>\n` when the marker is present, else `not installed (run 'ptrack hook install')\n`; both exit 0 — src: internal/cli/hook.go:86-103 — pinned by: none — verify: automated test
- CLI-063: All hook subcommands error with `.git is not a directory at <path> — install the hook manually` when the project root's `.git` is missing or not a plain directory (worktrees/submodules); note the em-dash in the message — src: internal/cli/hook.go:109-126 — pinned by: none — verify: automated test

### context / next / search / board

- CLI-064: `context` (no args): report.Context digest; Markdown default, `--json` for JSON — src: internal/cli/context.go:10-31 — pinned by: internal/cli/cli_test.go TestIntegrationFlow, internal/cli/milestone_issue_test.go TestContextShowsOpenIssues (substring-level) — verify: automated test
- CLI-065: `next` (no args): single most-actionable task (doing first, else todo, in the active plan); Markdown default, `--json` — src: internal/cli/next.go:9-30 — pinned by: internal/cli/cli_v2_test.go TestNextCommand — verify: automated test
- CLI-066: `search <term...>`: term args joined with spaces; substring match across plan/task titles and note bodies; Markdown default, `--json` — src: internal/cli/search.go:10-31 — pinned by: internal/cli/cli_v2_test.go TestSearchCommand — verify: automated test
- CLI-067: `board`: `--plan <id>` (default: active plan; no active plan → same `no active plan; ...` error as task add); Markdown kanban with column headers `Todo (n)`, `Doing (n)`, `Blocked (n)`, `Done (n)`; `--json` — src: internal/cli/board.go:13-55 — pinned by: internal/cli/cli_v2_test.go TestBoardCommand — verify: automated test
- CLI-068: `board --gui`: opens the desktop window with (empty path, planID) instead of rendering; `--gui` combined with `--json` errors `--gui and --json cannot be used together` — src: internal/cli/board.go:23-29 — pinned by: internal/cli/cli_v2_test.go TestBoardGUIOption — verify: automated test

### gui

- CLI-069: `gui [PATH]` accepts at most one positional arg (more → cobra arg error); invokes the GUI with (path-or-empty, planID 0); in builds without GUI support the error is `GUI support is unavailable in this build` — src: internal/cli/gui.go:7-19, internal/cli/root.go:14-19 — pinned by: internal/cli/cli_v2_test.go TestGUICommand — verify: automated test
- CLI-070: In the desktop build, main.go wires `gui`/`board --gui` to gui.Run with embedded `frontend/dist` assets — src: main.go:19-31 — pinned by: none — verify: manual

### status

- CLI-071: `status` human output is exactly four lines: `goal: <first line of goal, trimmed>` (or `goal: (no goal set)`), `active plan: <title>` (or `active plan: (no active plan)`), `tasks: <t> todo, <d> doing, <n> done, <b> blocked\n`, `plans: <count>\n` — src: internal/cli/status.go:67-97 — pinned by: none — verify: automated test
- CLI-072: `status --json` emits `{"goal":"...","active_plan":N,"active_plan_title":"...","plans":N,"todo":N,"doing":N,"done":N,"blocked":N}` — src: internal/cli/status.go:50-64 — pinned by: none — verify: automated test

### projects

- CLI-073: `projects` lists the global registry as tab-separated lines `<name>\t<path>\t<YYYY-MM-DD HH:MM:SS>\n` (local time via time.Format); `--json` emits raw `[]store.ProjectRef` — src: internal/cli/projects.go:12-40 — pinned by: none — verify: automated test

### backup

- CLI-074: `backup` copies the current project DB into `<global home>/backups` (filename derived from the unix timestamp at call time, via store.BackupProject), best-effort records the backup in the global store (record errors ignored), and prints the absolute backup path as a bare line — src: internal/cli/backup.go:16-49 — pinned by: none — verify: automated test

### capability

- CLI-075: `capability call <tool>`: `--arguments` (default `{}`) must be exactly one JSON object, else error `--arguments must be one JSON object`; on success prints the broker's raw JSON result + `\n` to stdout — src: internal/cli/capability.go:26-56 — pinned by: internal/cli/capability_test.go TestCapabilityCallUsesActiveHostBroker — verify: automated test
- CLI-076: `capability call`/`mcp` require env `PTRACK_CAPABILITY_TOKEN`; when unset the error is `capability broker token is unavailable; launch this command from an agent terminal in p-track`; session env (`PTRACK_CAPABILITY_PROJECT`, `PTRACK_CAPABILITY_GENERATION`) is validated against the broker descriptor before any call — src: internal/cli/capability.go:75-105 — pinned by: TestCapabilityCallUsesActiveHostBroker (happy path only) — verify: automated test
- CLI-077: `capability mcp` serves provider-compatible MCP tools over stdio (stdin/stdout), bridging each tool call to the host broker; this is the one command that consumes stdin and runs until the MCP session ends — src: internal/cli/capability.go:58-73 — pinned by: none — verify: manual

### guide

- CLI-078: `guide` installs/refreshes the agent guide into `AGENTS.md`/`CLAUDE.md` at the project root (requires an existing project); prints `wrote agent guide to <file>\n` per file or `agent guide already up to date\n`; `--print` instead writes the rendered guide (with global extra appended) to stdout and touches no files — src: internal/cli/guide.go:31-73 — pinned by: internal/cli/guide_test.go TestGuidePrint — verify: automated test

## Notes / surprises

- **Version flag vs subcommand disagree in format**: `ptrack version` → `ptrack <v>` but `ptrack --version` → `ptrack version <v>` (cobra default template). Both are observable; the installed 0.20.0 binary confirms both.
- **Cobra implicit surface is part of the contract**: `completion {bash,zsh,fish,powershell}` and `help` commands exist even though no Go file defines them; a Rust rewrite that ships only the 20 declared commands would diverge. Root help output (CLI-009) is cobra-formatted and sorted alphabetically.
- **Silent-success commands**: most mutations (`goal set`, `plan use`, `task start/done/block/rename`, `issue close/open/severity`, `milestone done/open/due/rename`, `commit record`) print nothing at all — only exit 0. Scripts may depend on the empty stdout.
- **Precedence quirks**: `note add` with both `--task` and `--plan` silently prefers `--task`; `commit list` with both filters silently prefers `--task`; `note list` with both ERRORS. Inconsistent but observable.
- **JSON shapes are split-brain**: `task list`/`plan list`/`note list`/`status` use hand-rolled row structs with curated keys, while `milestone list`/`issue list`/`commit list`/`projects` marshal the raw model/store types — so their JSON keys, zero-value handling, and time formats are whatever Go's `encoding/json` produces for those structs (defined in internal/model, internal/store; cross-check those inventories before freezing schemas).
- **`commit show` shells out to `git`** and forwards git's stdout/stderr verbatim; exact bytes are git's, not ptrack's, but the ref-resolution rule (numeric → tracked SHA lookup, else passthrough) and the `git -C <projectRoot>` working directory are ptrack's contract.
- **`openProject` has a side effect on every command**: it refreshes the global registry LastSeen timestamp (CLI-016). Failure is swallowed, so a read-only command still writes to the global DB when it can.
- **Error text includes wrapped system errors** (`invalid id %q: %w`, `invalid --due ...: %w`) — the `%w` tail is Go's `strconv`/`time` error wording (e.g. `strconv.ParseUint: parsing "abc": invalid syntax`). Byte-identical parity would require mimicking Go stdlib error strings; consider pinning only the prefix.
- **`hook install` content is byte-exact** (shebang, marker lines, single command line with `|| true`, 0755 mode) — pin with a fixture, not substring checks.
- **`milestone` alias `ms` and `task convert` alias `promote`** are easy to miss; both appear in help/error text only via cobra.
- **`main.go` has build tag `desktop && (production || dev)`** and `main_bindings.go` has tag `bindings` — a plain `go build` of module root compiles no main package. The CLI entry wiring (RunGUI/RunNoArgs overrides, WriterVersion stamping at main.go:24) exists only in the desktop-tagged build.
- **Board/no-active-plan error string is duplicated verbatim** in task.go:41 and board.go:41 — one contract, two sites.
