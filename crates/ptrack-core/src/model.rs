/// Version of the persistent capability record contract.
pub const CAPABILITY_MODEL_VERSION: u64 = 1;

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
}

macro_rules! persistent_enum {
    ($name:ident { $($variant:ident = $tag:literal),+ $(,)? }) => {
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
        }
    };
}

persistent_enum!(PlanStatus {
    Active = 1,
    Done = 2,
    Archived = 3,
});
persistent_enum!(TaskStatus {
    Todo = 1,
    Doing = 2,
    Done = 3,
    Blocked = 4,
});
persistent_enum!(NoteTarget {
    Project = 1,
    Plan = 2,
    Task = 3,
});
persistent_enum!(MemoryKind {
    Legacy = 0,
    Decision = 1,
    Blocker = 2,
    Handoff = 3,
    Summary = 4,
});
persistent_enum!(MilestoneStatus { Open = 1, Done = 2 });
persistent_enum!(IssueStatus { Open = 1, Closed = 2 });
persistent_enum!(Severity {
    Low = 1,
    Medium = 2,
    High = 3,
    Critical = 4,
});
persistent_enum!(CapabilityKind {
    Http = 1,
    Git = 2,
    Ssh = 3,
});

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

persistent_enum!(RecordKind {
    Meta = 1,
    Plan = 2,
    Task = 3,
    Note = 4,
    Milestone = 5,
    Issue = 6,
    Commit = 7,
    Capability = 8,
    CapabilityAudit = 9,
    MemoryWriteback = 10,
    ProjectRef = 11,
    GlobalConfig = 12,
    GlobalBackup = 13,
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
