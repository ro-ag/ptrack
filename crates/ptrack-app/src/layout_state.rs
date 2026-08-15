//! Durable layout and navigation state in the global store.
//!
//! One versioned JSON record lives under the `layout-state` global config key,
//! separate from `preferences` so a resize storm never rewrites user intent and
//! a corrupt layout can never cost the user their settings. It shares the
//! `preferences` discipline exactly: total normalization, clamping, and a
//! malformed or newer record that reads as defaults and is never rewritten
//! until the user changes something.
//!
//! Task drawer selection is deliberately absent: reopening onto a task the user
//! has since finished is worse than reopening onto the board.

use std::collections::BTreeMap;

use ptrack_store::{GlobalStore, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::preferences::{
    PATH_MAX_BYTES, PreferencesStorageV1, clamped, enumerated, field, merge, valid_text,
};
use crate::{AppError, AppResult};

const LAYOUT_STATE_KEY: &[u8] = b"layout-state";
/// Another module owns this record; a reset deletes the key directly rather
/// than depending on the window module.
const WINDOW_STATE_KEY: &[u8] = b"window-state";
const LAYOUT_STATE_VERSION: u64 = 1;
/// Matches the frontend `layout.ts` bounds. The viewport-responsive maximum
/// stays in `clampSidebarWidth`, which clamps again on load.
const SIDEBAR_WIDTH_DEFAULT: u64 = 248;
const SIDEBAR_WIDTH_MINIMUM: u64 = 180;
const SIDEBAR_WIDTH_MAXIMUM: u64 = 420;
const PROJECT_LIMIT: usize = 32;
const LANES: [&str; 4] = ["blocked", "doing", "done", "todo"];

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutViewV1 {
    #[default]
    Board,
    Overview,
    Capabilities,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidebarLayoutV1 {
    pub width: u64,
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelsLayoutV1 {
    pub board_hidden: bool,
    pub terminal_hidden: bool,
}

/// Per-project navigation context. `used_at` is a write counter, not a clock,
/// so the bounded map can evict the least recently used entry without trusting
/// a host clock that can jump backwards.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLayoutV1 {
    pub view: LayoutViewV1,
    pub plan_id: u64,
    pub folded_lanes: Vec<String>,
    pub used_at: u64,
}

/// The exact stored record. `version` is always the supported version because
/// an older record upgrades in memory and a newer one never decodes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutStateV1 {
    pub version: u64,
    pub sidebar: SidebarLayoutV1,
    pub panels: PanelsLayoutV1,
    pub projects: BTreeMap<String, ProjectLayoutV1>,
}

/// The stored record plus how it was obtained.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutStateDocumentV1 {
    pub storage: PreferencesStorageV1,
    #[serde(flatten)]
    pub layout: LayoutStateV1,
}

/// Reads and totally normalizes the stored layout record.
pub fn layout_state(store: &GlobalStore) -> LayoutStateDocumentV1 {
    store.config(LAYOUT_STATE_KEY).map_or_else(
        |_| document(PreferencesStorageV1::Unreadable, defaults()),
        |stored| decode(&stored),
    )
}

/// Merges a partial patch onto the current record and stores the whole
/// document. Unknown members are dropped and every value is renormalized. The
/// read, the merge, and the write share one transaction, so a concurrent save
/// cannot merge onto a stale record and drop the other panel.
///
/// # Errors
/// Returns an error when the patch is not a JSON object, or when the record
/// cannot be read or written. A record that cannot be read is left exactly as
/// it is stored and the patch is dropped.
pub fn set_layout_state(store: &GlobalStore, patch: &Value) -> AppResult<LayoutStateDocumentV1> {
    if !patch.is_object() {
        return Err(AppError::Message(
            "layout patch must be a JSON object".to_owned(),
        ));
    }
    store
        .update_config(LAYOUT_STATE_KEY, |stored| {
            let current = decode(stored);
            // Layout writes fire automatically on the first project open, so a
            // record this build cannot read is kept byte for byte: merely
            // opening a project on an older build must not destroy it.
            if current.storage == PreferencesStorageV1::Unreadable {
                return Ok((stored.to_vec(), current));
            }
            let mut record = serde_json::to_value(current.layout)
                .map_err(|error| StoreError::InvalidManifest(error.to_string()))?;
            merge(&mut record, patch);
            // A patch never rewrites the record version.
            record["version"] = json!(LAYOUT_STATE_VERSION);
            touch(&mut record, patch);
            let layout = normalize(&record).unwrap_or_else(defaults);
            let encoded = serde_json::to_vec(&layout)
                .map_err(|error| StoreError::InvalidManifest(error.to_string()))?;
            Ok((encoded, document(PreferencesStorageV1::Ok, layout)))
        })
        .map_err(AppError::from)
}

/// Deletes the stored record so the next read returns defaults.
///
/// # Errors
/// Returns an error when the delete fails.
pub fn reset_layout_state(store: &GlobalStore) -> AppResult<LayoutStateDocumentV1> {
    store.delete_config(LAYOUT_STATE_KEY)?;
    Ok(document(PreferencesStorageV1::Defaults, defaults()))
}

/// Clears the window and layout records together and returns the layout
/// defaults the caller applies live. Non-destructive to user data.
///
/// # Errors
/// Returns an error when either delete fails.
pub fn reset_window_layout(store: &GlobalStore) -> AppResult<LayoutStateDocumentV1> {
    store.delete_config(WINDOW_STATE_KEY)?;
    reset_layout_state(store)
}

/// Totally normalizes the exact stored bytes. Empty bytes mean no record yet.
fn decode(stored: &[u8]) -> LayoutStateDocumentV1 {
    if stored.is_empty() {
        return document(PreferencesStorageV1::Defaults, defaults());
    }
    serde_json::from_slice::<Value>(stored)
        .ok()
        .as_ref()
        .and_then(normalize)
        .map_or_else(
            || document(PreferencesStorageV1::Unreadable, defaults()),
            |layout| document(PreferencesStorageV1::Ok, layout),
        )
}

const fn document(storage: PreferencesStorageV1, layout: LayoutStateV1) -> LayoutStateDocumentV1 {
    LayoutStateDocumentV1 { storage, layout }
}

fn defaults() -> LayoutStateV1 {
    LayoutStateV1 {
        version: LAYOUT_STATE_VERSION,
        sidebar: sidebar(None),
        panels: panels(None),
        projects: BTreeMap::new(),
    }
}

/// Returns the normalized record, or `None` when this build cannot read it.
fn normalize(value: &Value) -> Option<LayoutStateV1> {
    let record = value.as_object()?;
    // A record without a readable version, or from a newer version, is
    // unreadable. An older version upgrades in memory and persists on write.
    if record.get("version").and_then(Value::as_u64)? > LAYOUT_STATE_VERSION {
        return None;
    }
    Some(LayoutStateV1 {
        version: LAYOUT_STATE_VERSION,
        sidebar: sidebar(record.get("sidebar")),
        panels: panels(record.get("panels")),
        projects: projects(record.get("projects")),
    })
}

fn sidebar(section: Option<&Value>) -> SidebarLayoutV1 {
    let section = section.and_then(Value::as_object);
    SidebarLayoutV1 {
        width: clamped(
            field(section, "width"),
            SIDEBAR_WIDTH_DEFAULT,
            SIDEBAR_WIDTH_MINIMUM,
            SIDEBAR_WIDTH_MAXIMUM,
        ),
        hidden: flag(field(section, "hidden")),
    }
}

fn panels(section: Option<&Value>) -> PanelsLayoutV1 {
    let section = section.and_then(Value::as_object);
    PanelsLayoutV1 {
        board_hidden: flag(field(section, "boardHidden")),
        terminal_hidden: flag(field(section, "terminalHidden")),
    }
}

/// Keeps at most the 32 most recently used projects. A tie keeps the entries
/// the map already ordered, so eviction is deterministic for a hand-written
/// record that carries no write counter at all.
fn projects(section: Option<&Value>) -> BTreeMap<String, ProjectLayoutV1> {
    let mut entries = section
        .and_then(Value::as_object)
        .map_or_else(Vec::new, |section| {
            section
                .iter()
                .filter(|(root, _)| valid_text(root, PATH_MAX_BYTES))
                .map(|(root, entry)| (root.clone(), project(entry)))
                .collect()
        });
    if entries.len() > PROJECT_LIMIT {
        entries.sort_by_key(|(_, entry)| std::cmp::Reverse(entry.used_at));
        entries.truncate(PROJECT_LIMIT);
    }
    entries.into_iter().collect()
}

fn project(entry: &Value) -> ProjectLayoutV1 {
    let entry = entry.as_object();
    ProjectLayoutV1 {
        view: enumerated(field(entry, "view")),
        // A stored plan is a hint, never an authority. The backend resolves it
        // and falls back to the active plan when it no longer resolves.
        plan_id: field(entry, "planId")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        folded_lanes: folded_lanes(field(entry, "foldedLanes")),
        used_at: field(entry, "usedAt")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    }
}

/// Keeps only known board lanes, sorted and deduplicated, which bounds the
/// list at four entries without a separate limit.
fn folded_lanes(value: Option<&Value>) -> Vec<String> {
    let mut lanes = value
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |lanes| {
            lanes
                .iter()
                .filter_map(Value::as_str)
                .filter(|lane| LANES.contains(lane))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        });
    lanes.sort_unstable();
    lanes.dedup();
    lanes
}

fn flag(value: Option<&Value>) -> bool {
    value.and_then(Value::as_bool).unwrap_or_default()
}

/// Stamps every project the patch touched as most recently used, so the
/// bounded map evicts the projects the user has stopped visiting.
fn touch(record: &mut Value, patch: &Value) {
    let Some(touched) = patch.get("projects").and_then(Value::as_object) else {
        return;
    };
    let Some(projects) = record.get_mut("projects").and_then(Value::as_object_mut) else {
        return;
    };
    let mut next = projects
        .values()
        .filter_map(|entry| entry.get("usedAt").and_then(Value::as_u64))
        .max()
        .unwrap_or_default();
    for root in touched.keys() {
        next = next.saturating_add(1);
        if let Some(entry) = projects.get_mut(root).and_then(Value::as_object_mut) {
            entry.insert("usedAt".to_owned(), json!(next));
        }
    }
}
