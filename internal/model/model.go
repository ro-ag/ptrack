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

// MilestoneStatus is the lifecycle state of a Milestone.
type MilestoneStatus string

// IssueStatus is the lifecycle state of an Issue.
type IssueStatus string

// Severity ranks an Issue's importance.
type Severity string

// CapabilityKind identifies the host service controlled by a capability.
type CapabilityKind string

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

	// MilestoneOpen marks a milestone still being worked toward.
	MilestoneOpen MilestoneStatus = "open"
	// MilestoneDone marks a reached milestone.
	MilestoneDone MilestoneStatus = "done"

	// IssueOpen marks an unresolved issue.
	IssueOpen IssueStatus = "open"
	// IssueClosed marks a resolved issue.
	IssueClosed IssueStatus = "closed"

	// SeverityLow, SeverityMedium, SeverityHigh, and SeverityCritical rank issues.
	SeverityLow      Severity = "low"
	SeverityMedium   Severity = "medium"
	SeverityHigh     Severity = "high"
	SeverityCritical Severity = "critical"

	// CapabilityHTTP grants a bounded HTTP request scope.
	CapabilityHTTP CapabilityKind = "http"
	// CapabilityGit grants bounded operations against one configured Git remote.
	CapabilityGit CapabilityKind = "git"
	// CapabilitySSH grants bounded SSH operations against one pinned host.
	CapabilitySSH CapabilityKind = "ssh"
)

// CapabilityModelVersion is the version of the capability record contract.
// It is independent of the project database format so records can be rejected
// or migrated individually in the future.
const CapabilityModelVersion uint = 1

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

// Milestone is a high-level checkpoint that groups plans toward a target,
// optionally with a due date.
type Milestone struct {
	ID        uint64
	Title     string
	Status    MilestoneStatus
	Due       time.Time // zero = no due date
	Order     int
	CreatedAt time.Time
	UpdatedAt time.Time
}

// Plan is an ordered unit of work within a project, optionally belonging to a
// milestone (MilestoneID 0 = unassigned).
type Plan struct {
	ID          uint64
	Title       string
	Status      PlanStatus
	MilestoneID uint64
	Order       int
	CreatedAt   time.Time
	UpdatedAt   time.Time
}

// Commit records a git commit in the project's audit trail, linked to a task
// (parsed from a #<id> reference) or a plan.
type Commit struct {
	ID        uint64
	SHA       string
	Subject   string
	PlanID    uint64
	TaskID    uint64
	CreatedAt time.Time
}

// Issue is a tracked problem or bug, optionally linked to a task.
type Issue struct {
	ID        uint64
	Title     string
	Body      string
	Status    IssueStatus
	Severity  Severity
	TaskID    uint64 // 0 = not linked to a task
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

// CapabilityLimits bounds host work even after a capability has been enabled.
// Missing byte/time/concurrency values receive safe defaults during
// normalization. A zero redirect limit is an explicit deny-all-redirects
// policy.
type CapabilityLimits struct {
	TimeoutSeconds   int   `json:"timeout_seconds"`
	MaxRequestBytes  int64 `json:"max_request_bytes"`
	MaxResponseBytes int64 `json:"max_response_bytes"`
	MaxOutputBytes   int64 `json:"max_output_bytes"`
	MaxRedirects     int   `json:"max_redirects"`
	MaxConcurrent    int   `json:"max_concurrent"`
}

// CapabilityAuditPolicy controls bounded metadata retention. Audit records
// never contain request/response bodies, headers, credentials, terminal
// contents, or raw secret-bearing arguments.
type CapabilityAuditPolicy struct {
	Enabled    bool `json:"enabled"`
	RetainLast int  `json:"retain_last"`
}

// HTTPScope grants methods and path prefixes beneath one normalized base URL.
type HTTPScope struct {
	BaseURL      string   `json:"base_url"`
	Methods      []string `json:"methods"`
	PathPrefixes []string `json:"path_prefixes"`
}

// GitScope grants named operations against one exact normalized remote URL.
type GitScope struct {
	RemoteName      string   `json:"remote_name"`
	RemoteURL       string   `json:"remote_url"`
	Operations      []string `json:"operations"`
	Branches        []string `json:"branches"`
	Refspecs        []string `json:"refspecs"`
	AllowTags       bool     `json:"allow_tags"`
	AllowForcePush  bool     `json:"allow_force_push"`
	AllowDeleteRefs bool     `json:"allow_delete_refs"`
}

// SSHScope grants access to one host identity. HostKey is a known_hosts-style
// public key (for example "ssh-ed25519 AAAA...") used with strict checking.
// High-risk operations remain independent and false by default. The
// interactive-shell field is reserved for a future duplex broker transport;
// current normalization rejects it.
type SSHScope struct {
	Alias                 string   `json:"alias"`
	Host                  string   `json:"host"`
	Port                  uint16   `json:"port"`
	User                  string   `json:"user"`
	HostKey               string   `json:"host_key"`
	AllowGit              bool     `json:"allow_git"`
	RemoteCommands        []string `json:"remote_commands"`
	AllowUpload           bool     `json:"allow_upload"`
	AllowDownload         bool     `json:"allow_download"`
	UploadRoots           []string `json:"upload_roots"`
	DownloadRoots         []string `json:"download_roots"`
	UploadRemoteRoots     []string `json:"upload_remote_roots"`
	DownloadRemoteRoots   []string `json:"download_remote_roots"`
	AllowInteractiveShell bool     `json:"allow_interactive_shell"`
	LocalForwardTargets   []string `json:"local_forward_targets"`
	RemoteForwardTargets  []string `json:"remote_forward_targets"`
}

// Capability is a project-local, agent-profile-scoped host grant. A record is
// usable only when Enabled is true, its approval has not expired, its profile
// exactly matches the caller, and the kind-specific scope authorizes the
// requested operation.
type Capability struct {
	ID                      uint64                `json:"id"`
	ModelVersion            uint                  `json:"model_version"`
	Revision                uint64                `json:"revision"`
	Name                    string                `json:"name"`
	Kind                    CapabilityKind        `json:"kind"`
	AgentProfile            string                `json:"agent_profile"`
	Enabled                 bool                  `json:"enabled"`
	ApprovalDurationSeconds int64                 `json:"approval_duration_seconds"`
	ApprovedAt              time.Time             `json:"approved_at"`
	ExpiresAt               time.Time             `json:"expires_at"`
	ScopeDigest             string                `json:"scope_digest"`
	Limits                  CapabilityLimits      `json:"limits"`
	Audit                   CapabilityAuditPolicy `json:"audit"`
	HTTP                    *HTTPScope            `json:"http,omitempty"`
	Git                     *GitScope             `json:"git,omitempty"`
	SSH                     *SSHScope             `json:"ssh,omitempty"`
	CreatedAt               time.Time             `json:"created_at"`
	UpdatedAt               time.Time             `json:"updated_at"`
}

// CapabilityAudit is deliberately metadata-only. Target is the normalized,
// non-secret scope target (host, remote, or HTTP origin). Diagnostic detail stays
// transient; only an allowlisted error class may be persisted.
type CapabilityAudit struct {
	ID             uint64         `json:"id"`
	CapabilityID   uint64         `json:"capability_id"`
	AgentProfile   string         `json:"agent_profile"`
	Kind           CapabilityKind `json:"kind"`
	Operation      string         `json:"operation"`
	Target         string         `json:"target"`
	Success        bool           `json:"success"`
	ErrorClass     string         `json:"error_class"`
	DurationMillis int64          `json:"duration_millis"`
	RequestBytes   int64          `json:"request_bytes"`
	ResponseBytes  int64          `json:"response_bytes"`
	Redirects      int            `json:"redirects"`
	CreatedAt      time.Time      `json:"created_at"`
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
	Milestones     int
	MilestonesDone int
	Plans          int
	PlansDone      int
	Tasks          int
	TasksDone      int
	TasksBlocked   int
	TasksOpen      int // not done (todo/doing/blocked)
	Issues         int
	IssuesOpen     int
	Commits        int
	Notes          int
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

// Ord exposes a Milestone's Order for generic sorting.
func (m Milestone) Ord() int { return m.Order }
