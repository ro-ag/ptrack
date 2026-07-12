// Package model defines the persistent data types for ptrack. These structs are
// serialized with encoding/gob into bbolt and carry no behavior beyond their
// fields, so both the store and the CLI/TUI layers can share them freely.
package model

import "time"

// PlanStatus is the lifecycle state of a Plan.
type PlanStatus string

// TaskStatus is the lifecycle state of a Task.
type TaskStatus string

// NoteTarget names what a Note is attached to.
type NoteTarget string

const (
	// PlanActive marks a plan currently being worked on.
	PlanActive PlanStatus = "active"
	// PlanDone marks a completed plan.
	PlanDone PlanStatus = "done"
	// PlanArchived marks a plan set aside without completion.
	PlanArchived PlanStatus = "archived"

	// TaskTodo is an unstarted task.
	TaskTodo TaskStatus = "todo"
	// TaskDoing is a task in progress.
	TaskDoing TaskStatus = "doing"
	// TaskDone is a finished task.
	TaskDone TaskStatus = "done"
	// TaskBlocked is a task that cannot proceed.
	TaskBlocked TaskStatus = "blocked"

	// TargetProject attaches a note to the project itself.
	TargetProject NoteTarget = "project"
	// TargetPlan attaches a note to a plan.
	TargetPlan NoteTarget = "plan"
	// TargetTask attaches a note to a task.
	TargetTask NoteTarget = "task"
)

// Meta is the singleton per-project record: the north-star goal, a rolling
// context summary maintained across sessions, and the currently active plan.
type Meta struct {
	Goal       string
	Summary    string
	ActivePlan uint64
	CreatedAt  time.Time
	UpdatedAt  time.Time
	// FormatVersion is the on-disk schema version, used to gate migrations and
	// reject databases written by a newer ptrack. Zero means a pre-versioning
	// (v0.1.0) database, adopted as version 1 on first open.
	FormatVersion uint
	// LastWriteVersion is the ptrack semver that last wrote the database,
	// recorded for diagnostics only (never gates behavior).
	LastWriteVersion string
}

// Plan is an ordered unit of work within a project.
type Plan struct {
	ID        uint64
	Title     string
	Status    PlanStatus
	Order     int
	CreatedAt time.Time
	UpdatedAt time.Time
}

// Task is an actionable item belonging to a Plan.
type Task struct {
	ID        uint64
	PlanID    uint64
	Title     string
	Status    TaskStatus
	Order     int
	CreatedAt time.Time
	UpdatedAt time.Time
}

// Note is a timestamped decision or observation attached to the project, a
// plan, or a task.
type Note struct {
	ID        uint64
	Target    NoteTarget
	TargetID  uint64
	Body      string
	CreatedAt time.Time
}

// ProjectRef is a global-registry entry pointing at a known project directory.
type ProjectRef struct {
	Name     string
	Path     string
	LastSeen time.Time
}

// Counts is a project-wide inventory summary used for the bounded context
// footer: totals plus the breakdowns an agent needs to decide what to query.
type Counts struct {
	Plans        int
	PlansDone    int
	Tasks        int
	TasksDone    int
	TasksBlocked int
	TasksOpen    int // not done (todo/doing/blocked)
	Notes        int
}

// Open reports whether a task status counts as "open" (not done) for the
// purposes of the restore digest.
func (s TaskStatus) Open() bool {
	return s != TaskDone
}

// Ord exposes a Plan's Order for generic sorting.
func (p Plan) Ord() int { return p.Order }

// Ord exposes a Task's Order for generic sorting.
func (t Task) Ord() int { return t.Order }
