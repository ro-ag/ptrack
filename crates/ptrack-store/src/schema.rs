use std::cmp::Ordering;
use std::fmt;

use redb::TableDefinition;

use crate::{LEGACY_CODEC_GO_GOB, LEGACY_CODEC_RAW, StoreError, StoreResult};

pub(crate) const STORE_FAMILY: &[u8] = b"ptrack-redb";
pub(crate) const STORE_OWNER: &[u8] = b"ptrack-storage-tool";
pub(crate) const STORE_STATE_READY: &[u8] = b"ready";
pub(crate) const STORE_STATE_IMPORTING: &[u8] = b"importing";
/// The current application-level ptrack database schema.
pub const STORE_SCHEMA_VERSION: u32 = 1;

pub(crate) const MANIFEST_KEY_FAMILY: &[u8] = b"family";
pub(crate) const MANIFEST_KEY_OWNER: &[u8] = b"owner";
pub(crate) const MANIFEST_KEY_SCHEMA_VERSION: &[u8] = b"schema_version";
pub(crate) const MANIFEST_KEY_STATE: &[u8] = b"state";
pub(crate) const MANIFEST_KEY_STORE_KIND: &[u8] = b"store_kind";

pub(crate) const MANIFEST_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.schema");
pub(crate) const SEQUENCES_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.sequences");

const PROJECT_META_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.project.meta");
const PLANS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("ptrack.project.plans");
const TASKS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("ptrack.project.tasks");
const NOTES_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("ptrack.project.notes");
const MILESTONES_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.project.milestones");
const ISSUES_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("ptrack.project.issues");
const COMMITS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("ptrack.project.commits");
const CAPABILITIES_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.project.capabilities");
const CAPABILITY_AUDITS_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.project.capability_audits");
const MEMORY_WRITEBACKS_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.project.memory_writebacks");
const GLOBAL_CONFIG_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.global.config");
const GLOBAL_PROJECTS_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.global.projects");
const GLOBAL_BACKUPS_TABLE: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("ptrack.global.backups");

/// The two independently versioned ptrack database families.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreKind {
    Project,
    Global,
}

impl StoreKind {
    pub(crate) const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Project => b"project",
            Self::Global => b"global",
        }
    }

    pub(crate) fn from_bytes(value: &[u8]) -> StoreResult<Self> {
        match value {
            b"project" => Ok(Self::Project),
            b"global" => Ok(Self::Global),
            _ => Err(StoreError::InvalidManifest(format!(
                "unknown store kind bytes {value:02x?}"
            ))),
        }
    }
}

impl fmt::Display for StoreKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project => formatter.write_str("project"),
            Self::Global => formatter.write_str("global"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyKind {
    Singleton,
    Id,
    Bytes,
}

impl KeyKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Singleton => "singleton",
            Self::Id => "numeric ID",
            Self::Bytes => "byte string",
        }
    }
}

/// A closed set of persisted ptrack record collections.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Collection {
    ProjectMeta,
    Plans,
    Tasks,
    Notes,
    Milestones,
    Issues,
    Commits,
    Capabilities,
    CapabilityAudits,
    MemoryWritebacks,
    GlobalConfig,
    GlobalProjects,
    GlobalBackups,
}

pub(crate) const ALL_COLLECTIONS: [Collection; 13] = [
    Collection::ProjectMeta,
    Collection::Plans,
    Collection::Tasks,
    Collection::Notes,
    Collection::Milestones,
    Collection::Issues,
    Collection::Commits,
    Collection::Capabilities,
    Collection::CapabilityAudits,
    Collection::MemoryWritebacks,
    Collection::GlobalConfig,
    Collection::GlobalProjects,
    Collection::GlobalBackups,
];

impl Collection {
    /// Returns every persisted collection in canonical schema order.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &ALL_COLLECTIONS
    }

    /// Iterates the complete collection set for one database family.
    pub fn for_store(kind: StoreKind) -> impl Iterator<Item = Self> {
        ALL_COLLECTIONS
            .into_iter()
            .filter(move |collection| collection.store_kind() == kind)
    }

    /// Resolves an exact legacy bbolt bucket name.
    #[must_use]
    pub fn from_legacy_name(name: &[u8]) -> Option<Self> {
        ALL_COLLECTIONS
            .into_iter()
            .find(|collection| collection.name().as_bytes() == name)
    }

    /// Returns the stable legacy bucket name used by migration and sequences.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ProjectMeta => "meta",
            Self::Plans => "plans",
            Self::Tasks => "tasks",
            Self::Notes => "notes",
            Self::Milestones => "milestones",
            Self::Issues => "issues",
            Self::Commits => "commits",
            Self::Capabilities => "capabilities",
            Self::CapabilityAudits => "capability_audits",
            Self::MemoryWritebacks => "memory_writebacks",
            Self::GlobalConfig => "config",
            Self::GlobalProjects => "projects",
            Self::GlobalBackups => "backups",
        }
    }

    /// Returns the database family that owns this collection.
    #[must_use]
    pub const fn store_kind(self) -> StoreKind {
        match self {
            Self::ProjectMeta
            | Self::Plans
            | Self::Tasks
            | Self::Notes
            | Self::Milestones
            | Self::Issues
            | Self::Commits
            | Self::Capabilities
            | Self::CapabilityAudits
            | Self::MemoryWritebacks => StoreKind::Project,
            Self::GlobalConfig | Self::GlobalProjects | Self::GlobalBackups => StoreKind::Global,
        }
    }

    /// Returns the stable codec used to preserve this legacy bucket's values.
    #[must_use]
    pub const fn legacy_codec(self) -> u16 {
        match self {
            Self::GlobalConfig | Self::GlobalBackups => LEGACY_CODEC_RAW,
            Self::ProjectMeta
            | Self::Plans
            | Self::Tasks
            | Self::Notes
            | Self::Milestones
            | Self::Issues
            | Self::Commits
            | Self::Capabilities
            | Self::CapabilityAudits
            | Self::MemoryWritebacks
            | Self::GlobalProjects => LEGACY_CODEC_GO_GOB,
        }
    }

    /// Reports whether the legacy bbolt bucket has an independent high-water mark.
    #[must_use]
    pub const fn is_sequenced(self) -> bool {
        matches!(
            self,
            Self::Plans
                | Self::Tasks
                | Self::Notes
                | Self::Milestones
                | Self::Issues
                | Self::Commits
                | Self::Capabilities
                | Self::CapabilityAudits
                | Self::MemoryWritebacks
        )
    }

    pub(crate) const fn key_kind(self) -> KeyKind {
        match self {
            Self::ProjectMeta => KeyKind::Singleton,
            Self::Plans
            | Self::Tasks
            | Self::Notes
            | Self::Milestones
            | Self::Issues
            | Self::Commits
            | Self::Capabilities
            | Self::CapabilityAudits => KeyKind::Id,
            Self::MemoryWritebacks
            | Self::GlobalConfig
            | Self::GlobalProjects
            | Self::GlobalBackups => KeyKind::Bytes,
        }
    }

    pub(crate) fn validate_store(self, actual: StoreKind) -> StoreResult<()> {
        let expected = self.store_kind();
        if expected == actual {
            Ok(())
        } else {
            Err(StoreError::CollectionStoreMismatch {
                collection: self.name(),
                expected,
                actual,
            })
        }
    }

    pub(crate) const fn table(self) -> TableDefinition<'static, &'static [u8], &'static [u8]> {
        match self {
            Self::ProjectMeta => PROJECT_META_TABLE,
            Self::Plans => PLANS_TABLE,
            Self::Tasks => TASKS_TABLE,
            Self::Notes => NOTES_TABLE,
            Self::Milestones => MILESTONES_TABLE,
            Self::Issues => ISSUES_TABLE,
            Self::Commits => COMMITS_TABLE,
            Self::Capabilities => CAPABILITIES_TABLE,
            Self::CapabilityAudits => CAPABILITY_AUDITS_TABLE,
            Self::MemoryWritebacks => MEMORY_WRITEBACKS_TABLE,
            Self::GlobalConfig => GLOBAL_CONFIG_TABLE,
            Self::GlobalProjects => GLOBAL_PROJECTS_TABLE,
            Self::GlobalBackups => GLOBAL_BACKUPS_TABLE,
        }
    }
}

/// A borrowed record key whose representation is checked against its collection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKey<'a> {
    Singleton,
    Id(u64),
    Bytes(&'a [u8]),
}

impl RecordKey<'_> {
    fn kind(self) -> KeyKind {
        match self {
            Self::Singleton => KeyKind::Singleton,
            Self::Id(_) => KeyKind::Id,
            Self::Bytes(_) => KeyKind::Bytes,
        }
    }

    pub(crate) fn encode(self, collection: Collection) -> StoreResult<Vec<u8>> {
        let actual = self.kind();
        let expected = collection.key_kind();
        if actual != expected {
            return Err(StoreError::KeyKindMismatch {
                collection: collection.name(),
                expected: expected.name(),
                actual: actual.name(),
            });
        }

        Ok(match self {
            Self::Singleton => b"meta".to_vec(),
            Self::Id(id) => id.to_be_bytes().to_vec(),
            Self::Bytes(bytes) => bytes.to_vec(),
        })
    }
}

/// An owned key returned by collection scans.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnedRecordKey {
    Singleton,
    Id(u64),
    Bytes(Vec<u8>),
}

impl OwnedRecordKey {
    /// Borrows this owned key without changing its representation.
    #[must_use]
    pub fn as_borrowed(&self) -> RecordKey<'_> {
        match self {
            Self::Singleton => RecordKey::Singleton,
            Self::Id(id) => RecordKey::Id(*id),
            Self::Bytes(bytes) => RecordKey::Bytes(bytes),
        }
    }

    pub(crate) fn validated_encoded_len(&self, collection: Collection) -> StoreResult<usize> {
        let borrowed = self.as_borrowed();
        let actual = borrowed.kind();
        let expected = collection.key_kind();
        if actual != expected {
            return Err(StoreError::KeyKindMismatch {
                collection: collection.name(),
                expected: expected.name(),
                actual: actual.name(),
            });
        }
        Ok(match self {
            Self::Singleton => b"meta".len(),
            Self::Id(_) => size_of::<u64>(),
            Self::Bytes(bytes) => bytes.len(),
        })
    }

    pub(crate) fn compare_encoded(
        &self,
        other: &Self,
        collection: Collection,
    ) -> StoreResult<Ordering> {
        self.validated_encoded_len(collection)?;
        other.validated_encoded_len(collection)?;
        Ok(match (self, other) {
            (Self::Singleton, Self::Singleton) => Ordering::Equal,
            (Self::Id(left), Self::Id(right)) => left.cmp(right),
            (Self::Bytes(left), Self::Bytes(right)) => left.cmp(right),
            _ => unreachable!("both keys were validated against one collection"),
        })
    }

    pub(crate) fn matches_encoded(
        &self,
        collection: Collection,
        encoded: &[u8],
    ) -> StoreResult<bool> {
        self.validated_encoded_len(collection)?;
        Ok(match self {
            Self::Singleton => encoded == b"meta",
            Self::Id(id) => encoded == id.to_be_bytes(),
            Self::Bytes(bytes) => encoded == bytes,
        })
    }
}

pub(crate) fn decode_key(collection: Collection, encoded: &[u8]) -> StoreResult<OwnedRecordKey> {
    match collection.key_kind() {
        KeyKind::Singleton if encoded == b"meta" => Ok(OwnedRecordKey::Singleton),
        KeyKind::Singleton => Err(StoreError::InvalidManifest(format!(
            "collection {} contains an invalid singleton key",
            collection.name()
        ))),
        KeyKind::Id if encoded.len() == 8 => Ok(OwnedRecordKey::Id(u64::from_be_bytes(
            encoded
                .try_into()
                .expect("the numeric key length was checked"),
        ))),
        KeyKind::Id => Err(StoreError::InvalidManifest(format!(
            "collection {} contains a {}-byte numeric key",
            collection.name(),
            encoded.len()
        ))),
        KeyKind::Bytes => Ok(OwnedRecordKey::Bytes(encoded.to_vec())),
    }
}

pub(crate) fn collections_for(kind: StoreKind) -> impl Iterator<Item = Collection> {
    Collection::for_store(kind)
}
