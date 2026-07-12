# ptrack MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the ptrack CLI MVP — persist AI planning state (goal, plans, tasks, notes, rolling summary) in bbolt and emit a restore digest via `ptrack context`.

**Architecture:** Layered Go: `model` (pure structs) → `store` (bbolt project + global stores, gob-encoded values) → `report` (context digest) → `cli` (cobra subcommands, agent-facing) and `tui` (bubbletea, human-facing). `cli`/`tui` depend on `store`+`report`; `store` depends on `model`; `model` depends on nothing internal.

**Tech Stack:** Go 1.26, `go.etcd.io/bbolt`, `encoding/gob`, `github.com/spf13/cobra`, `github.com/charmbracelet/{bubbletea,lipgloss,bubbles}`.

## Global Constraints

- Module path `github.com/ro-ag/ptrack`; Go 1.26; pure Go, no CGO.
- DB values encoded with `encoding/gob`; JSON only at `--json` output boundaries.
- Project DB `.ptrack/ptrack.db` (discovered by walking up for `.ptrack/` or `.git/`); global DB `~/.ptrack/global.db`.
- Per-project IDs are incremental `uint64` via bbolt `NextSequence`.
- Tests: stdlib `testing`, sibling `*_test.go`, each store test uses `t.TempDir()`. No AI attribution in commits.
- Every subcommand exits non-zero on error with a clear message.

---

## Interfaces (locked — implementers code against these exact signatures)

### package `model` — `internal/model/model.go`

```go
type PlanStatus string
type TaskStatus string
type NoteTarget string

const (
	PlanActive   PlanStatus = "active"
	PlanDone     PlanStatus = "done"
	PlanArchived PlanStatus = "archived"

	TaskTodo    TaskStatus = "todo"
	TaskDoing   TaskStatus = "doing"
	TaskDone    TaskStatus = "done"
	TaskBlocked TaskStatus = "blocked"

	TargetProject NoteTarget = "project"
	TargetPlan    NoteTarget = "plan"
	TargetTask    NoteTarget = "task"
)

type Meta struct {
	Goal       string
	Summary    string
	ActivePlan uint64
	CreatedAt  time.Time
	UpdatedAt  time.Time
}

type Plan struct {
	ID        uint64
	Title     string
	Status    PlanStatus
	Order     int
	CreatedAt time.Time
	UpdatedAt time.Time
}

type Task struct {
	ID        uint64
	PlanID    uint64
	Title     string
	Status    TaskStatus
	Order     int
	CreatedAt time.Time
	UpdatedAt time.Time
}

type Note struct {
	ID        uint64
	Target    NoteTarget
	TargetID  uint64
	Body      string
	CreatedAt time.Time
}

type ProjectRef struct {
	Name     string
	Path     string
	LastSeen time.Time
}
```

### package `store` — `internal/store/`

`store.go` (project store):
```go
type Store struct { /* wraps *bbolt.DB */ }

func Open(dbPath string) (*Store, error)   // opens/creates buckets: meta, plans, tasks, notes
func (s *Store) Close() error

// meta
func (s *Store) GetMeta() (model.Meta, error)
func (s *Store) SetGoal(goal string) error
func (s *Store) SetSummary(summary string) error
func (s *Store) SetActivePlan(id uint64) error

// plans
func (s *Store) AddPlan(title string) (model.Plan, error)      // Status=active, Order=append
func (s *Store) ListPlans() ([]model.Plan, error)              // ordered by Order asc
func (s *Store) GetPlan(id uint64) (model.Plan, error)         // ErrNotFound if missing
func (s *Store) SetPlanStatus(id uint64, st model.PlanStatus) error

// tasks
func (s *Store) AddTask(planID uint64, title string) (model.Task, error) // Status=todo
func (s *Store) ListTasks() ([]model.Task, error)
func (s *Store) ListTasksByPlan(planID uint64) ([]model.Task, error)
func (s *Store) GetTask(id uint64) (model.Task, error)
func (s *Store) SetTaskStatus(id uint64, st model.TaskStatus) error

// notes
func (s *Store) AddNote(target model.NoteTarget, targetID uint64, body string) (model.Note, error)
func (s *Store) ListNotes() ([]model.Note, error)             // ordered by CreatedAt asc
func (s *Store) RecentNotes(n int) ([]model.Note, error)      // newest n, newest-first

var ErrNotFound = errors.New("not found")
```

`discovery.go`:
```go
// FindProjectDB walks up from start looking for an existing .ptrack/ptrack.db;
// stops at a .git/ boundary or filesystem root. Returns ErrNoProject if none.
func FindProjectDB(start string) (dbPath string, err error)
// InitProject creates <dir>/.ptrack/ptrack.db (dir defaults to a git root if present, else cwd).
func InitProject(dir string) (dbPath string, err error)
var ErrNoProject = errors.New("no ptrack project found (run 'ptrack init')")
```

`global.go` (global store):
```go
type Global struct { /* wraps *bbolt.DB */ }
func OpenGlobal() (*Global, error)                       // ~/.ptrack/global.db, buckets: config, projects, backups
func (g *Global) Close() error
func (g *Global) SetConfig(key, val string) error
func (g *Global) GetConfig(key string) (string, error)  // "" if absent
func (g *Global) RegisterProject(name, path string) error
func (g *Global) ListProjects() ([]model.ProjectRef, error)
func (g *Global) RecordBackup(projectPath, backupPath string) error

// BackupProject copies a project DB file to ~/.ptrack/backups/<name>-<unixts>.db.
// ts passed in (no time.Now in store core) — caller supplies via time param.
func BackupProject(projectDBPath, destDir string, ts int64) (backupPath string, err error)
```

`codec.go`: `gobEncode(v any) ([]byte, error)` / `gobDecode(data []byte, v any) error`.

### package `report` — `internal/report/report.go`

```go
// Markdown builds the token-efficient restore digest:
// goal, summary, active plan + its open tasks (todo/doing/blocked), recent notes.
func Markdown(s *store.Store) (string, error)
// JSON builds the same tree as indented JSON.
func JSON(s *store.Store) ([]byte, error)
```

### package `cli` — `internal/cli/`

```go
func Execute() error   // root cobra cmd; main.go calls this
```
One file per command group: `root.go`, `init.go`, `goal.go`, `summary.go`, `plan.go`, `task.go`, `note.go`, `context.go`, `status.go`, `projects.go`, `backup.go`, plus `open.go` helper `openProject() (*store.Store, error)` (discovery + Open, touches global registry LastSeen).

### package `tui` — `internal/tui/tui.go`

```go
func Run() error   // launches bubbletea dashboard for the current project
```

### `main.go`

```go
func main() { if err := cli.Execute(); err != nil { fmt.Fprintln(os.Stderr, err); os.Exit(1) } }
```

---

## Tasks

### Task 1: `model` package
**Files:** Create `internal/model/model.go`, `internal/model/model_test.go`.
- [ ] Write structs + consts exactly as locked above.
- [ ] Test: gob round-trip of each struct preserves fields (register types, encode, decode, `reflect.DeepEqual`).
- [ ] `go test ./internal/model/` passes. Commit.

### Task 2: `store` codec + discovery
**Files:** Create `internal/store/codec.go`, `internal/store/discovery.go`, `internal/store/discovery_test.go`.
- [ ] `gobEncode`/`gobDecode`.
- [ ] `FindProjectDB` walks up, honors `.ptrack/ptrack.db`, stops at `.git/` or root, returns `ErrNoProject`. `InitProject` creates dir tree.
- [ ] Tests (using `t.TempDir()`): init then find in same dir; find from nested subdir; `ErrNoProject` when absent. Commit.

### Task 3: `store` project CRUD
**Files:** Create `internal/store/store.go`, `internal/store/store_test.go`.
- [ ] `Open`, `Close`, bucket creation, `NextSequence` IDs.
- [ ] Meta ops (goal/summary/active plan; set CreatedAt on first open, UpdatedAt on writes).
- [ ] Plan/Task/Note ops per locked signatures; `ErrNotFound` on missing IDs; ordering guarantees.
- [ ] Tests: add/list/get/status transitions for plans+tasks; notes recent ordering; ErrNotFound cases. Commit.

### Task 4: `store` global
**Files:** Create `internal/store/global.go`, `internal/store/global_test.go`.
- [ ] `OpenGlobal` (respect `PTRACK_HOME` env override of `~/.ptrack` for testability), config KV, project registry, `BackupProject` file copy, `RecordBackup`.
- [ ] Tests with `PTRACK_HOME=t.TempDir()`: config set/get, register+list, backup copies file & records. Commit.

### Task 5: `report` digest
**Files:** Create `internal/report/report.go`, `internal/report/report_test.go`.
- [ ] `Markdown` + `JSON` over a seeded store.
- [ ] Tests: seed store (goal, summary, active plan, mixed-status tasks, notes) → assert markdown contains goal/summary/active-plan title/open task titles and excludes done tasks; JSON unmarshals to expected shape. Commit.

### Task 6: `cli` commands
**Files:** Create `internal/cli/*.go` (root, open, init, goal, summary, plan, task, note, context, status, projects, backup) + `internal/cli/cli_test.go`.
- [ ] cobra tree; `openProject` helper; each command wired to store/report; `context --json`.
- [ ] Integration test: drive commands against a temp `PTRACK_HOME`+temp cwd via exported `Execute`-style test harness (or test command RunE funcs directly): init → set goal → add plan → add task → context contains task title. Commit.

### Task 7: `tui` dashboard
**Files:** Create `internal/tui/tui.go` (+ styles). Read-only v1: show goal, summary, plans list, active plan tasks; keybindings q/quit, arrows navigate; config & backup actions.
- [ ] `Run()` opens current project, renders model with lipgloss; graceful message if no project.
- [ ] Manual smoke (TUI hard to unit test); keep logic thin, delegate data to store. Commit.

### Task 8: `main.go` + wiring + build
**Files:** Create `main.go`; `go mod tidy`.
- [ ] `main` calls `cli.Execute()`; no-args path launches `tui.Run()` (wired in root `RunE`).
- [ ] `go build ./...` and `go test ./...` green. Commit.

## Self-Review Notes
- Spec coverage: meta/plan/task/note (T1,T3), two-tier store (T3,T4), discovery (T2), context md+json (T5,T6), agent subcommands (T6), TUI (T7), backup/registry (T4,T6,T7). All covered.
- Time injection: store core avoids `time.Now()` where determinism matters (`BackupProject` takes `ts`); CRUD timestamps use `time.Now()` internally (acceptable; tests assert non-zero, not exact).
- `PTRACK_HOME` env override added for global-store testability — reflected in Task 4/6.
