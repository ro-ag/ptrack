//! Durable application preferences in the global store.
//!
//! One versioned JSON record lives under the `preferences` global config key.
//! Every read normalizes totally: an unknown enum, an out-of-range number, a
//! wrong type, or a missing field falls back to its documented default. A
//! malformed record, or one written by a newer version, reads as defaults and
//! is never rewritten until the user changes a setting, so a downgrade cannot
//! silently destroy a newer record.

use ptrack_store::{GlobalStore, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{AppError, AppResult};

const PREFERENCES_KEY: &[u8] = b"preferences";
const PREFERENCES_VERSION: u64 = 1;
const FONT_FAMILY_DEFAULT: &str = "monospace";
const FONT_SIZE_DEFAULT: u64 = 14;
const FONT_SIZE_MINIMUM: u64 = 10;
const FONT_SIZE_MAXIMUM: u64 = 24;
const SCROLLBACK_DEFAULT: u64 = 25_000;
const SCROLLBACK_MINIMUM: u64 = 1_000;
const SCROLLBACK_MAXIMUM: u64 = 200_000;
const TEXT_MAX_BYTES: usize = 128;

/// Whether the returned document came from storage, from defaults, or from a
/// record this build cannot read.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreferencesStorageV1 {
    Ok,
    Defaults,
    Unreadable,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeV1 {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DensityV1 {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReducedMotionV1 {
    #[default]
    System,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UnicodeModeV1 {
    #[default]
    Modern,
    Legacy,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RendererV1 {
    #[default]
    Auto,
    Webgl,
    Canvas,
    Dom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearancePreferencesV1 {
    pub theme: ThemeV1,
    pub density: DensityV1,
    pub reduced_motion: ReducedMotionV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPreferencesV1 {
    pub default_profile_id: Option<String>,
    pub font_family: String,
    pub font_size: u64,
    pub unicode_mode: UnicodeModeV1,
    pub scrollback: u64,
    pub renderer: RendererV1,
}

/// The exact stored record. `version` is always the supported version because
/// an older record upgrades in memory and a newer one never decodes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesV1 {
    pub version: u64,
    pub appearance: AppearancePreferencesV1,
    pub terminal: TerminalPreferencesV1,
}

/// The stored record plus how it was obtained.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesDocumentV1 {
    pub storage: PreferencesStorageV1,
    #[serde(flatten)]
    pub preferences: PreferencesV1,
}

/// Reads and totally normalizes the stored preferences record.
pub fn preferences(store: &GlobalStore) -> PreferencesDocumentV1 {
    store.config(PREFERENCES_KEY).map_or_else(
        |_| document(PreferencesStorageV1::Unreadable, defaults()),
        |stored| decode(&stored),
    )
}

/// Merges a partial patch onto the current record and stores the whole
/// document. Unknown members are dropped and every value is renormalized. The
/// read, the merge, and the write share one transaction, so a concurrent save
/// cannot merge onto a stale record and drop the other setting.
///
/// # Errors
/// Returns an error when the patch is not a JSON object, or when the record
/// cannot be read or written. A record that cannot be read is never replaced
/// by defaults.
pub fn set_preferences(store: &GlobalStore, patch: &Value) -> AppResult<PreferencesDocumentV1> {
    if !patch.is_object() {
        return Err(AppError::Message(
            "preferences patch must be a JSON object".to_owned(),
        ));
    }
    store
        .update_config(PREFERENCES_KEY, |stored| {
            let mut record = serde_json::to_value(decode(stored).preferences)
                .map_err(|error| StoreError::InvalidManifest(error.to_string()))?;
            merge(&mut record, patch);
            // A patch never rewrites the record version.
            record["version"] = json!(PREFERENCES_VERSION);
            let preferences = normalize(&record).unwrap_or_else(defaults);
            let encoded = serde_json::to_vec(&preferences)
                .map_err(|error| StoreError::InvalidManifest(error.to_string()))?;
            Ok((encoded, document(PreferencesStorageV1::Ok, preferences)))
        })
        .map_err(AppError::from)
}

/// Deletes the stored record so the next read returns defaults.
///
/// # Errors
/// Returns an error when the delete fails.
pub fn reset_preferences(store: &GlobalStore) -> AppResult<PreferencesDocumentV1> {
    store.delete_config(PREFERENCES_KEY)?;
    Ok(document(PreferencesStorageV1::Defaults, defaults()))
}

/// Totally normalizes the exact stored bytes. Empty bytes mean no record yet.
fn decode(stored: &[u8]) -> PreferencesDocumentV1 {
    if stored.is_empty() {
        return document(PreferencesStorageV1::Defaults, defaults());
    }
    serde_json::from_slice::<Value>(stored)
        .ok()
        .as_ref()
        .and_then(normalize)
        .map_or_else(
            || document(PreferencesStorageV1::Unreadable, defaults()),
            |preferences| document(PreferencesStorageV1::Ok, preferences),
        )
}

const fn document(
    storage: PreferencesStorageV1,
    preferences: PreferencesV1,
) -> PreferencesDocumentV1 {
    PreferencesDocumentV1 {
        storage,
        preferences,
    }
}

fn defaults() -> PreferencesV1 {
    PreferencesV1 {
        version: PREFERENCES_VERSION,
        appearance: appearance(None),
        terminal: terminal(None),
    }
}

/// Returns the normalized record, or `None` when this build cannot read it.
fn normalize(value: &Value) -> Option<PreferencesV1> {
    let record = value.as_object()?;
    // A record without a readable version, or from a newer version, is
    // unreadable. An older version upgrades in memory and persists on write.
    if record.get("version").and_then(Value::as_u64)? > PREFERENCES_VERSION {
        return None;
    }
    Some(PreferencesV1 {
        version: PREFERENCES_VERSION,
        appearance: appearance(record.get("appearance")),
        terminal: terminal(record.get("terminal")),
    })
}

fn appearance(section: Option<&Value>) -> AppearancePreferencesV1 {
    let section = section.and_then(Value::as_object);
    AppearancePreferencesV1 {
        theme: enumerated(field(section, "theme")),
        density: enumerated(field(section, "density")),
        reduced_motion: enumerated(field(section, "reducedMotion")),
    }
}

fn terminal(section: Option<&Value>) -> TerminalPreferencesV1 {
    let section = section.and_then(Value::as_object);
    TerminalPreferencesV1 {
        // A stored profile id is never coerced; the UI reports one that no
        // longer resolves as unavailable.
        default_profile_id: text(field(section, "defaultProfileId")),
        font_family: text(field(section, "fontFamily"))
            .unwrap_or_else(|| FONT_FAMILY_DEFAULT.to_owned()),
        font_size: clamped(
            field(section, "fontSize"),
            FONT_SIZE_DEFAULT,
            FONT_SIZE_MINIMUM,
            FONT_SIZE_MAXIMUM,
        ),
        unicode_mode: enumerated(field(section, "unicodeMode")),
        scrollback: clamped(
            field(section, "scrollback"),
            SCROLLBACK_DEFAULT,
            SCROLLBACK_MINIMUM,
            SCROLLBACK_MAXIMUM,
        ),
        renderer: enumerated(field(section, "renderer")),
    }
}

fn field<'a>(section: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a Value> {
    section.and_then(|section| section.get(key))
}

fn enumerated<T: Default + for<'de> Deserialize<'de>>(value: Option<&Value>) -> T {
    value
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// Clamps an integer into its documented range. A non-integer is a wrong type
/// and falls back to the default; a negative value clamps to the minimum.
fn clamped(value: Option<&Value>, default: u64, minimum: u64, maximum: u64) -> u64 {
    value.and_then(Value::as_i64).map_or(default, |number| {
        u64::try_from(number).map_or(minimum, |number| number.clamp(minimum, maximum))
    })
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| {
            !text.is_empty() && text.len() <= TEXT_MAX_BYTES && !text.contains(char::is_control)
        })
        .map(ToOwned::to_owned)
}

/// Recursively merges `patch` into `target` object by object.
fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                merge(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}
