use std::fmt;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::{Event, Run, Timestamp, process_alive};

#[cfg(unix)]
#[path = "persistence_unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "persistence_windows.rs"]
mod platform;

pub(crate) const PERSISTED_STATE_VERSION: u64 = 3;
pub(crate) const RUN_HISTORY_FILE_NAME: &str = "agent-runs.json";
const RAW_URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PersistenceError {
    FutureVersion { found: u64 },
    DescriptorNotFound,
    DescriptorStale { pid: i32 },
    InvalidDescriptorName,
    Message(String),
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FutureVersion { found } => write!(
                formatter,
                "AgentRun history is newer than supported: version {found} exceeds {PERSISTED_STATE_VERSION}"
            ),
            Self::DescriptorNotFound => formatter.write_str("AgentRun descriptor not found"),
            Self::DescriptorStale { pid } => write!(
                formatter,
                "AgentRun descriptor is stale: owning process {pid} is not running"
            ),
            Self::InvalidDescriptorName => {
                formatter.write_str("runtime descriptor name must be a base name")
            }
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PersistenceError {}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationDescriptor {
    pub project_root: String,
    pub url: String,
    pub generation: u64,
    pub registration_token: String,
    pub pid: i32,
}

impl fmt::Debug for IntegrationDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IntegrationDescriptor")
            .field("project_root", &self.project_root)
            .field("url", &self.url)
            .field("generation", &self.generation)
            .field("registration_token", &"[redacted]")
            .field("pid", &self.pid)
            .finish()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedRegistryState {
    pub version: u64,
    #[serde(default)]
    pub saved_at: Timestamp,
    #[serde(default)]
    pub runs: Vec<PersistedRecord>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedRecord {
    pub run: Run,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lease_token: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub last_source_sequence: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub next_host_sequence: u64,
}

pub(crate) enum WriteHistoryOutcome {
    Written,
    FutureVersion,
}

/// Returns the private per-project runtime directory without creating it.
///
/// # Errors
/// Returns an error if either input cannot be made absolute.
pub fn runtime_dir(
    global_home: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
) -> Result<PathBuf, PersistenceError> {
    let home = absolute_clean(global_home.as_ref(), "resolve AgentRun runtime home")?;
    let root = absolute_clean(project_root.as_ref(), "resolve AgentRun project root")?;
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use fmt::Write as _;
        write!(hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(home.join("runtime").join(hex))
}

/// Returns the versioned run-history location for one project.
///
/// # Errors
/// Returns the same path-resolution errors as [`runtime_dir`].
pub fn run_history_path(
    global_home: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
) -> Result<PathBuf, PersistenceError> {
    Ok(runtime_dir(global_home, project_root)?.join(RUN_HISTORY_FILE_NAME))
}

/// Atomically publishes private JSON under the per-project runtime directory.
///
/// # Errors
/// Returns a name, encoding, permission, locking, or atomic-publication error.
pub fn publish_runtime_json<T: Serialize>(
    global_home: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
    name: &str,
    value: &T,
) -> Result<PathBuf, PersistenceError> {
    validate_descriptor_name(name)?;
    let directory = runtime_dir(global_home, project_root)?;
    write_json_atomic(&directory.join(name), value, name)?;
    Ok(directory.join(name))
}

/// Removes one named runtime file, treating absence as success.
///
/// # Errors
/// Returns a name, path-resolution, or removal error.
pub fn remove_runtime_file(
    global_home: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
    name: &str,
) -> Result<(), PersistenceError> {
    validate_descriptor_name(name)?;
    let directory = runtime_dir(global_home, project_root)?;
    let pinned = match platform::PinnedRuntimeDir::open(&directory) {
        Ok(pinned) => pinned,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(PersistenceError::Message(error.to_string())),
    };
    match pinned.remove_file(name) {
        Ok(()) => {
            pinned
                .sync()
                .map_err(io_message("sync runtime directory"))?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PersistenceError::Message(error.to_string())),
    }
}

/// Removes a descriptor only when its parsed JSON is deeply equal to `expected`.
///
/// # Errors
/// Returns a name, path-resolution, locking, read, encoding, or removal error.
pub fn remove_runtime_json_if_equal<T: Serialize>(
    global_home: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
    name: &str,
    expected: &T,
) -> Result<(), PersistenceError> {
    validate_descriptor_name(name)?;
    let directory = runtime_dir(global_home, project_root)?;
    let pinned = match platform::PinnedRuntimeDir::open(&directory) {
        Ok(pinned) => pinned,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(PersistenceError::Message(error.to_string())),
    };
    let _guard = match pinned.lock_private_descriptor() {
        Ok(guard) => guard,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(PersistenceError::Message(error.to_string())),
    };
    let path = directory.join(name);
    let contents = match pinned.read_private_file(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(PersistenceError::Message(error.to_string())),
    };
    let Ok(actual) = serde_json::from_slice::<serde_json::Value>(&contents) else {
        return Ok(());
    };
    let expected = serde_json::to_value(expected)
        .map_err(|error| PersistenceError::Message(error.to_string()))?;
    if !json_deep_equal(&actual, &expected) {
        return Ok(());
    }
    match pinned.remove_path(&path) {
        Ok(()) => pinned.sync().map_err(io_message("sync runtime directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PersistenceError::Message(error.to_string())),
    }
}

pub(crate) fn remove_integration_descriptor_if_owned(
    global_home: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
    generation: u64,
    registration_token: &str,
) -> Result<(), PersistenceError> {
    let directory = runtime_dir(global_home, project_root)?;
    let pinned = match platform::PinnedRuntimeDir::open(&directory) {
        Ok(pinned) => pinned,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(PersistenceError::Message(error.to_string())),
    };
    let _guard = match pinned.lock_private_descriptor() {
        Ok(guard) => guard,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(PersistenceError::Message(error.to_string())),
    };
    let path = directory.join("agent-registry.json");
    let contents = match pinned.read_private_file(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(PersistenceError::Message(format!(
                "read AgentRun descriptor for cleanup: {error}"
            )));
        }
    };
    let Ok(descriptor) = serde_json::from_slice::<IntegrationDescriptor>(&contents) else {
        return Ok(());
    };
    if descriptor.generation != generation
        || !constant_time_equal(&descriptor.registration_token, registration_token)
    {
        return Ok(());
    }
    match pinned.remove_path(&path) {
        Ok(()) => pinned.sync().map_err(io_message("sync runtime directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PersistenceError::Message(format!(
            "remove AgentRun descriptor: {error}"
        ))),
    }
}

/// Reads the integration locator and rejects descriptors owned by dead processes.
///
/// # Errors
/// Returns not-found, stale, private-read, or JSON-decoding errors.
pub fn read_integration_descriptor(
    global_home: impl AsRef<Path>,
    project_root: impl AsRef<Path>,
) -> Result<IntegrationDescriptor, PersistenceError> {
    let path = runtime_dir(global_home, project_root)?.join("agent-registry.json");
    let contents = platform::read_private_file(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            PersistenceError::DescriptorNotFound
        } else {
            PersistenceError::Message(format!("read AgentRun descriptor: {error}"))
        }
    })?;
    let descriptor: IntegrationDescriptor = serde_json::from_slice(&contents).map_err(|error| {
        PersistenceError::Message(format!("decode AgentRun descriptor: {error}"))
    })?;
    if !process_alive(descriptor.pid) {
        return Err(PersistenceError::DescriptorStale {
            pid: descriptor.pid,
        });
    }
    Ok(descriptor)
}

pub(crate) fn read_history(
    path: &Path,
) -> Result<Option<PersistedRegistryState>, PersistenceError> {
    let contents = match platform::read_private_file(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(PersistenceError::Message(format!(
                "read AgentRun history: {error}"
            )));
        }
    };
    if let Some(found) = strict_history_version(&contents)? {
        return Err(PersistenceError::FutureVersion { found });
    }
    let state: PersistedRegistryState = serde_json::from_slice(&contents)
        .map_err(|error| PersistenceError::Message(format!("decode AgentRun history: {error}")))?;
    Ok(Some(state))
}

pub(crate) fn write_history(
    path: &Path,
    state: &PersistedRegistryState,
) -> Result<WriteHistoryOutcome, PersistenceError> {
    let directory = path.parent().ok_or_else(|| {
        PersistenceError::Message("runtime descriptor has no parent directory".to_owned())
    })?;
    platform::prepare_private_runtime_dir(directory)
        .map_err(io_message("prepare private AgentRun runtime directory"))?;
    let pinned = platform::PinnedRuntimeDir::open(directory)
        .map_err(io_message("pin private AgentRun runtime directory"))?;
    invoke_after_pin_hook();
    let _guard = pinned
        .lock_private_descriptor()
        .map_err(io_message("lock AgentRun descriptor"))?;
    match pinned.read_private_file(path) {
        Ok(contents) => {
            if future_history_version(&contents).is_some() {
                return Ok(WriteHistoryOutcome::FutureVersion);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(PersistenceError::Message(format!(
                "read AgentRun history: {error}"
            )));
        }
    }
    write_json_atomic_pinned(&pinned, path, state, "agent-runs")
        .map_err(|error| PersistenceError::Message(format!("write AgentRun history: {error}")))?;
    Ok(WriteHistoryOutcome::Written)
}

fn write_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
    temporary_stem: &str,
) -> Result<(), PersistenceError> {
    let directory = path.parent().ok_or_else(|| {
        PersistenceError::Message("runtime descriptor has no parent directory".to_owned())
    })?;
    platform::prepare_private_runtime_dir(directory)
        .map_err(io_message("prepare private AgentRun runtime directory"))?;
    let pinned = platform::PinnedRuntimeDir::open(directory)
        .map_err(io_message("pin private AgentRun runtime directory"))?;
    invoke_after_pin_hook();
    let _guard = pinned
        .lock_private_descriptor()
        .map_err(io_message("lock AgentRun descriptor"))?;
    write_json_atomic_pinned(&pinned, path, value, temporary_stem)
}

fn write_json_atomic_pinned<T: Serialize>(
    pinned: &platform::PinnedRuntimeDir,
    path: &Path,
    value: &T,
    temporary_stem: &str,
) -> Result<(), PersistenceError> {
    let token = random_opaque_value()?;
    let temporary_name = format!(".{temporary_stem}-{token}");
    let mut file = pinned
        .create_private_file(&temporary_name)
        .map_err(io_message("create private AgentRun descriptor"))?;
    let owned_identity = platform::file_identity(&file)
        .map_err(io_message("identify private AgentRun descriptor"))?;
    let write_result = (|| -> Result<(), PersistenceError> {
        serde_json::to_writer(&mut file, value)
            .map_err(|error| PersistenceError::Message(error.to_string()))?;
        file.write_all(b"\n")
            .map_err(|error| PersistenceError::Message(error.to_string()))?;
        file.sync_all()
            .map_err(|error| PersistenceError::Message(error.to_string()))?;
        Ok(())
    })();
    drop(file);
    if let Err(error) = write_result {
        let _ = pinned.remove_owned_file(&temporary_name, &owned_identity);
        return Err(error);
    }
    invoke_before_rename_hook();
    if let Err(error) = pinned.replace_private_descriptor(&temporary_name, path) {
        let _ = pinned.remove_owned_file(&temporary_name, &owned_identity);
        return Err(PersistenceError::Message(error.to_string()));
    }
    invoke_after_rename_hook();
    if let Err(error) = pinned.secure_published_descriptor(path) {
        let _ = pinned.remove_owned_path(path, &owned_identity);
        return Err(io_message("secure AgentRun descriptor")(error));
    }
    pinned
        .sync()
        .map_err(io_message("sync AgentRun runtime directory"))
}

fn strict_history_version(contents: &[u8]) -> Result<Option<u64>, PersistenceError> {
    #[derive(Deserialize)]
    struct VersionOnly {
        version: u64,
    }
    let version: VersionOnly = serde_json::from_slice(contents)
        .map_err(|error| PersistenceError::Message(format!("decode AgentRun history: {error}")))?;
    Ok((version.version > PERSISTED_STATE_VERSION).then_some(version.version))
}

fn future_history_version(contents: &[u8]) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_slice(contents).ok()?;
    let version = value.get("version")?.as_u64()?;
    (version > PERSISTED_STATE_VERSION).then_some(version)
}

fn validate_descriptor_name(name: &str) -> Result<(), PersistenceError> {
    let mut components = Path::new(name).components();
    if name.is_empty()
        || name == "."
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(PersistenceError::InvalidDescriptorName);
    }
    Ok(())
}

pub(crate) fn absolute_clean(path: &Path, label: &str) -> Result<PathBuf, PersistenceError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| PersistenceError::Message(format!("{label}: {error}")))?
            .join(path)
    };
    let mut clean = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            _ => clean.push(component.as_os_str()),
        }
    }
    Ok(clean)
}

fn random_opaque_value() -> Result<String, PersistenceError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        PersistenceError::Message(format!("create AgentRun opaque value: {error}"))
    })?;
    let mut result = String::with_capacity(43);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        result.push(char::from(RAW_URL_ALPHABET[usize::from(first >> 2)]));
        result.push(char::from(
            RAW_URL_ALPHABET[usize::from(((first & 3) << 4) | (second >> 4))],
        ));
        if chunk.len() > 1 {
            result.push(char::from(
                RAW_URL_ALPHABET[usize::from(((second & 15) << 2) | (third >> 6))],
            ));
        }
        if chunk.len() > 2 {
            result.push(char::from(RAW_URL_ALPHABET[usize::from(third & 63)]));
        }
    }
    Ok(result)
}

fn json_deep_equal(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    match (left, right) {
        (serde_json::Value::Number(left), serde_json::Value::Number(right)) => {
            left.as_f64() == right.as_f64()
        }
        (serde_json::Value::Array(left), serde_json::Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_deep_equal(left, right))
        }
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| json_deep_equal(left, right))
                })
        }
        _ => left == right,
    }
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    left.len() == right.len() && bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

fn io_message(label: &'static str) -> impl FnOnce(io::Error) -> PersistenceError {
    move |error| PersistenceError::Message(format!("{label}: {error}"))
}

#[cfg(test)]
thread_local! {
    static AFTER_PIN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static BEFORE_RENAME_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static AFTER_RENAME_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn install_after_pin_hook(hook: impl FnOnce() + 'static) {
    AFTER_PIN_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn install_before_rename_hook(hook: impl FnOnce() + 'static) {
    BEFORE_RENAME_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn install_after_rename_hook(hook: impl FnOnce() + 'static) {
    AFTER_RENAME_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn invoke_after_pin_hook() {
    AFTER_PIN_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn invoke_before_rename_hook() {
    BEFORE_RENAME_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn invoke_after_rename_hook() {
    AFTER_RENAME_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn invoke_after_pin_hook() {}

#[cfg(not(test))]
fn invoke_before_rename_hook() {}

#[cfg(not(test))]
fn invoke_after_rename_hook() {}

const fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}
