use std::fmt;
use std::str::FromStr;

/// Version of the persistent capability record contract.
pub const CAPABILITY_MODEL_VERSION: u64 = 1;

/// An unrecognized persistent enum name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseEnumError {
    enum_name: &'static str,
    value: String,
}

impl ParseEnumError {
    const fn new(enum_name: &'static str, value: String) -> Self {
        Self { enum_name, value }
    }

    /// Returns the enum type whose value was rejected.
    #[must_use]
    pub const fn enum_name(&self) -> &'static str {
        self.enum_name
    }

    /// Returns the rejected value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for ParseEnumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} value {:?}",
            self.enum_name, self.value
        )
    }
}

impl std::error::Error for ParseEnumError {}

/// A SHA-256-sized digest.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Digest32(pub [u8; 32]);

impl Digest32 {
    /// The absent/invalid zero digest.
    pub const EMPTY: Self = Self([0; 32]);

    /// Reports whether all digest bytes are zero.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self == Self::EMPTY
    }
}

/// The persistent subset of Go's `time.Time` needed by ptrack.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Timestamp {
    /// Go's zero `time.Time` value.
    #[default]
    Zero,
    /// An instant with its original fixed UTC offset.
    Fixed {
        /// Seconds since the Unix epoch.
        seconds: i64,
        /// Nanoseconds within the second.
        nanoseconds: u32,
        /// Signed offset east of UTC, in seconds.
        offset_seconds: i32,
    },
}

impl Timestamp {
    /// Reports whether this is Go's zero time.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        matches!(self, Self::Zero)
    }

    /// Returns Unix nanoseconds for comparisons when the multiplication fits.
    #[must_use]
    pub fn unix_nanoseconds(self) -> Option<i128> {
        match self {
            Self::Zero => None,
            Self::Fixed {
                seconds,
                nanoseconds,
                ..
            } => Some(i128::from(seconds) * 1_000_000_000 + i128::from(nanoseconds)),
        }
    }

    /// Returns the stored calendar date using the timestamp's persisted fixed
    /// offset, without consulting the host timezone.
    #[must_use]
    pub fn stored_date(self) -> Option<StoredDate> {
        let Self::Fixed {
            seconds,
            offset_seconds,
            ..
        } = self
        else {
            return None;
        };
        let local_seconds = i128::from(seconds) + i128::from(offset_seconds);
        Some(StoredDate::from_unix_days(local_seconds.div_euclid(86_400)))
    }
}

/// A proleptic-Gregorian calendar date derived from a stored fixed offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoredDate {
    pub year: i64,
    pub month: u8,
    pub day: u8,
}

impl StoredDate {
    fn from_unix_days(days: i128) -> Self {
        // Howard Hinnant's civil-from-days algorithm, shifted from the Unix
        // epoch. i128 arithmetic keeps all possible Timestamp values safe.
        let shifted = days + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let mut year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_prime = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
        let month = month_prime + if month_prime < 10 { 3 } else { -9 };
        if month <= 2 {
            year += 1;
        }
        Self {
            year: i64::try_from(year).expect("i64 timestamp year fits i64"),
            month: u8::try_from(month).expect("civil month fits u8"),
            day: u8::try_from(day).expect("civil day fits u8"),
        }
    }
}

impl fmt::Display for StoredDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.year < 0 {
            write!(
                formatter,
                "-{:04}-{:02}-{:02}",
                self.year.unsigned_abs(),
                self.month,
                self.day
            )
        } else {
            write!(
                formatter,
                "{:04}-{:02}-{:02}",
                self.year, self.month, self.day
            )
        }
    }
}

macro_rules! persistent_enum {
    ($name:ident { $($variant:ident = $tag:literal => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        #[repr(u8)]
        pub enum $name {
            $($variant = $tag),+
        }

        impl $name {
            /// Returns the stable one-byte wire discriminant.
            #[must_use]
            pub const fn wire_tag(self) -> u8 {
                self as u8
            }

            /// Decodes a stable wire discriminant without accepting unknown values.
            #[must_use]
            pub const fn from_wire_tag(tag: u8) -> Option<Self> {
                match tag {
                    $($tag => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// Returns the stable Go-compatible string value.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }

            /// Parses the exact, case-sensitive Go-compatible string value.
            #[must_use]
            pub fn from_name(value: &str) -> Option<Self> {
                match value {
                    $($value => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::from_name(value)
                    .ok_or_else(|| ParseEnumError::new(stringify!($name), value.to_owned()))
            }
        }
    };
}

persistent_enum!(PlanStatus {
    Active = 1 => "active",
    Done = 2 => "done",
    Archived = 3 => "archived",
});
persistent_enum!(TaskStatus {
    Todo = 1 => "todo",
    Doing = 2 => "doing",
    Done = 3 => "done",
    Blocked = 4 => "blocked",
});
persistent_enum!(NoteTarget {
    Project = 1 => "project",
    Plan = 2 => "plan",
    Task = 3 => "task",
});
persistent_enum!(MemoryKind {
    Legacy = 0 => "",
    Decision = 1 => "decision",
    Blocker = 2 => "blocker",
    Handoff = 3 => "handoff",
    Summary = 4 => "summary",
});
persistent_enum!(MilestoneStatus { Open = 1 => "open", Done = 2 => "done" });
persistent_enum!(IssueStatus { Open = 1 => "open", Closed = 2 => "closed" });
persistent_enum!(Severity {
    Low = 1 => "low",
    Medium = 2 => "medium",
    High = 3 => "high",
    Critical = 4 => "critical",
});
persistent_enum!(CapabilityKind {
    Http = 1 => "http",
    Git = 2 => "git",
    Ssh = 3 => "ssh",
});

impl TaskStatus {
    /// Reports whether the task counts as open in reports and inventory.
    #[must_use]
    pub const fn is_open(self) -> bool {
        !matches!(self, Self::Done)
    }
}

/// Singleton project metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Meta {
    pub goal: String,
    pub summary: String,
    pub active_plan: u64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub format_version: u64,
    pub last_write_version: String,
}

/// A high-level project checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub id: u64,
    pub title: String,
    pub status: MilestoneStatus,
    pub due: Timestamp,
    /// Persisted Go `int`; native callers convert only after range checks.
    pub order: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Milestone {
    /// Returns the externally visible display order.
    #[must_use]
    pub const fn ord(&self) -> i64 {
        self.order
    }
}

/// An ordered unit of work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub id: u64,
    pub title: String,
    pub status: PlanStatus,
    pub milestone_id: u64,
    /// Persisted Go `int`; native callers convert only after range checks.
    pub order: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Single-line reason this plan is on hold, or `None` when it is running.
    pub hold_reason: Option<String>,
}

impl Plan {
    /// Returns the externally visible display order.
    #[must_use]
    pub const fn ord(&self) -> i64 {
        self.order
    }
}

/// A git commit linked to a plan or task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Commit {
    pub id: u64,
    pub sha: String,
    pub subject: String,
    pub plan_id: u64,
    pub task_id: u64,
    pub created_at: Timestamp,
}

/// A tracked project issue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Issue {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub status: IssueStatus,
    pub severity: Severity,
    pub task_id: u64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// An actionable item belonging to a plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Task {
    pub id: u64,
    pub plan_id: u64,
    pub title: String,
    pub status: TaskStatus,
    /// Persisted Go `int`; native callers convert only after range checks.
    pub order: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Single-line reason this task is on hold, or `None` when it is running.
    pub hold_reason: Option<String>,
}

impl Task {
    /// Returns the externally visible display order.
    #[must_use]
    pub const fn ord(&self) -> i64 {
        self.order
    }
}

/// A durable observation attached to a project, plan, or task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Note {
    pub id: u64,
    pub target: NoteTarget,
    pub target_id: u64,
    pub kind: MemoryKind,
    pub body: String,
    pub created_at: Timestamp,
}

/// Per-operation resource ceilings for a capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityLimits {
    pub timeout_seconds: i64,
    pub max_request_bytes: i64,
    pub max_response_bytes: i64,
    pub max_output_bytes: i64,
    pub max_redirects: i64,
    pub max_concurrent: i64,
}

/// Metadata-only audit retention settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityAuditPolicy {
    pub enabled: bool,
    pub retain_last: i64,
}

/// HTTP capability scope. Full URL/path normalization belongs to policy code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpScope {
    pub base_url: String,
    pub methods: Vec<String>,
    pub path_prefixes: Vec<String>,
}

/// Git capability scope. Full remote/ref normalization belongs to policy code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitScope {
    pub remote_name: String,
    pub remote_url: String,
    pub operations: Vec<String>,
    pub branches: Vec<String>,
    pub refspecs: Vec<String>,
    pub allow_tags: bool,
    pub allow_force_push: bool,
    pub allow_delete_refs: bool,
}

/// SSH capability scope. Full host/path normalization belongs to policy code.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct SshScope {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub host_key: String,
    pub allow_git: bool,
    pub remote_commands: Vec<String>,
    pub allow_upload: bool,
    pub allow_download: bool,
    pub upload_roots: Vec<String>,
    pub download_roots: Vec<String>,
    pub upload_remote_roots: Vec<String>,
    pub download_remote_roots: Vec<String>,
    pub allow_interactive_shell: bool,
    pub local_forward_targets: Vec<String>,
    pub remote_forward_targets: Vec<String>,
}

/// A project-local, profile-scoped host grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capability {
    pub id: u64,
    pub model_version: u64,
    pub revision: u64,
    pub name: String,
    pub kind: CapabilityKind,
    pub agent_profile: String,
    pub enabled: bool,
    pub approval_duration_seconds: i64,
    pub approved_at: Timestamp,
    pub expires_at: Timestamp,
    pub scope_digest: Digest32,
    pub limits: CapabilityLimits,
    pub audit: CapabilityAuditPolicy,
    pub http: Option<HttpScope>,
    pub git: Option<GitScope>,
    pub ssh: Option<SshScope>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Bounded metadata for one capability operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityAudit {
    pub id: u64,
    pub capability_id: u64,
    pub agent_profile: String,
    pub kind: CapabilityKind,
    pub operation: String,
    pub target: String,
    pub success: bool,
    pub error_class: String,
    pub duration_millis: i64,
    pub request_bytes: i64,
    pub response_bytes: i64,
    /// Persisted Go `int`; native callers convert only after range checks.
    pub redirects: i64,
    pub created_at: Timestamp,
}

/// Idempotency receipt for a terminal memory write-back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryWritebackRecord {
    pub digest: Digest32,
    pub sequence: u64,
    pub kind: MemoryKind,
    pub note_id: u64,
}

/// Global registry pointer to a known project directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRef {
    pub name: String,
    pub path: String,
    pub last_seen: Timestamp,
}

/// Project-wide inventory totals used by the bounded context footer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    pub milestones: usize,
    pub milestones_done: usize,
    pub plans: usize,
    pub plans_done: usize,
    /// Plans carrying a hold reason; orthogonal to `plans_done`.
    pub plans_on_hold: usize,
    pub tasks: usize,
    pub tasks_done: usize,
    pub tasks_blocked: usize,
    /// Every task not in the done state.
    pub tasks_open: usize,
    /// Tasks carrying a hold reason; orthogonal to every status total above.
    pub tasks_on_hold: usize,
    pub issues: usize,
    pub issues_open: usize,
    pub commits: usize,
    pub notes: usize,
}

persistent_enum!(RecordKind {
    Meta = 1 => "meta",
    Plan = 2 => "plan",
    Task = 3 => "task",
    Note = 4 => "note",
    Milestone = 5 => "milestone",
    Issue = 6 => "issue",
    Commit = 7 => "commit",
    Capability = 8 => "capability",
    CapabilityAudit = 9 => "capability_audit",
    MemoryWriteback = 10 => "memory_writeback",
    ProjectRef = 11 => "project_ref",
    GlobalConfig = 12 => "global_config",
    GlobalBackup = 13 => "global_backup",
});

/// One typed persistent ptrack value.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum NativeRecord {
    Meta(Meta),
    Plan(Plan),
    Task(Task),
    Note(Note),
    Milestone(Milestone),
    Issue(Issue),
    Commit(Commit),
    Capability(Capability),
    CapabilityAudit(CapabilityAudit),
    MemoryWriteback(MemoryWritebackRecord),
    ProjectRef(ProjectRef),
}

impl NativeRecord {
    /// Returns the stable binary discriminant for this record.
    #[must_use]
    pub const fn kind(&self) -> RecordKind {
        match self {
            Self::Meta(_) => RecordKind::Meta,
            Self::Plan(_) => RecordKind::Plan,
            Self::Task(_) => RecordKind::Task,
            Self::Note(_) => RecordKind::Note,
            Self::Milestone(_) => RecordKind::Milestone,
            Self::Issue(_) => RecordKind::Issue,
            Self::Commit(_) => RecordKind::Commit,
            Self::Capability(_) => RecordKind::Capability,
            Self::CapabilityAudit(_) => RecordKind::CapabilityAudit,
            Self::MemoryWriteback(_) => RecordKind::MemoryWriteback,
            Self::ProjectRef(_) => RecordKind::ProjectRef,
        }
    }
}
