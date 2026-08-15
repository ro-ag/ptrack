//! Durable main-window geometry in the global store.
//!
//! One versioned JSON record lives under the `window-state` global config key,
//! with the same discipline as the `preferences` record: total normalization,
//! one transaction per write, and a malformed or newer record that reads as
//! defaults instead of being destroyed.
//!
//! The window is owned entirely by Rust. Nothing here is reachable from the
//! frontend: the desktop shell reads the record in its setup and writes it from
//! its window-event handler, and no IPC command exposes either side.
//!
//! Everything stored is in **logical** coordinates. A physical rect replayed at
//! a different scale factor lands in the wrong place and at the wrong size, so
//! the shell divides by the scale factor on capture and multiplies on restore.

use ptrack_store::{GlobalStore, StoreError};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{ActiveRuntime, AppError, AppResult};

const WINDOW_STATE_KEY: &[u8] = b"window-state";
const WINDOW_STATE_VERSION: u64 = 1;
/// The configured window size in `src-tauri/tauri.conf.json`.
const DEFAULT_WIDTH: f64 = 1_440.0;
const DEFAULT_HEIGHT: f64 = 900.0;
/// The configured window minimum in `src-tauri/tauri.conf.json`.
const MINIMUM_WIDTH: f64 = 880.0;
const MINIMUM_HEIGHT: f64 = 560.0;
/// A stored rect is replayed where it was only when it overlaps a work area by
/// at least this many logical pixels in both axes.
const OVERLAP_MINIMUM: f64 = 64.0;
/// Geometry is bounded so a garbage record cannot ask for an absurd window.
const COORDINATE_MAXIMUM: f64 = 1_000_000.0;
const SCALE_MINIMUM: f64 = 0.1;
const SCALE_MAXIMUM: f64 = 16.0;

/// Whether the returned document came from storage, from defaults, or from a
/// record this build cannot read.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowStateStorageV1 {
    Ok,
    Defaults,
    Unreadable,
}

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RectV1 {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One display, fingerprinted by its work area and scale factor. The monitor
/// name is deliberately not used: it is optional and not stable across replug.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayV1 {
    pub work_area: RectV1,
    pub scale_factor: f64,
}

/// The exact stored record. `version` is always the supported version because
/// an older record upgrades in memory and a newer one never decodes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowStateV1 {
    pub version: u64,
    pub logical: RectV1,
    pub scale_factor: f64,
    pub maximized: bool,
    pub fullscreen: bool,
    pub display: DisplayV1,
}

/// The stored record plus how it was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowStateDocumentV1 {
    pub storage: WindowStateStorageV1,
    #[serde(flatten)]
    pub window: WindowStateV1,
}

/// Where the shell puts the window on startup, in logical pixels, together with
/// the scale factor of the display it belongs to. Fullscreen is never part of a
/// placement: a window quit in fullscreen reopens windowed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementV1 {
    pub logical: RectV1,
    pub scale_factor: f64,
    pub maximized: bool,
}

/// Builds the record for one captured window geometry.
#[must_use]
pub const fn captured(
    logical: RectV1,
    scale_factor: f64,
    maximized: bool,
    fullscreen: bool,
    display: DisplayV1,
) -> WindowStateV1 {
    WindowStateV1 {
        version: WINDOW_STATE_VERSION,
        logical,
        scale_factor,
        maximized,
        fullscreen,
        display,
    }
}

/// Converts a physical rect to logical pixels. An unusable scale factor is
/// treated as 1.0 rather than producing an infinite or zeroed rect.
#[must_use]
pub fn logical_rect(physical: RectV1, scale_factor: f64) -> RectV1 {
    let scale = usable_scale(scale_factor);
    RectV1 {
        x: physical.x / scale,
        y: physical.y / scale,
        width: physical.width / scale,
        height: physical.height / scale,
    }
}

/// Converts a logical rect back to physical pixels for the target display.
#[must_use]
pub fn physical_rect(logical: RectV1, scale_factor: f64) -> RectV1 {
    let scale = usable_scale(scale_factor);
    RectV1 {
        x: logical.x * scale,
        y: logical.y * scale,
        width: logical.width * scale,
        height: logical.height * scale,
    }
}

/// Decides the startup placement from the stored record and the displays that
/// exist now. `monitors` and `primary` are logical work areas.
///
/// Returns `None` when there is nothing trustworthy to restore, which leaves the
/// configured window untouched. Otherwise the size is always restored, clamped
/// to the configured minimum, and the position is kept only while some work
/// area still overlaps it by the minimum overlap in logical pixels in both axes
/// — a rect stranded on a removed display is centered on the primary display
/// instead, and only that recentered size is clamped to the target work area.
#[must_use]
pub fn restore_placement(
    document: &WindowStateDocumentV1,
    monitors: &[DisplayV1],
    primary: Option<DisplayV1>,
) -> Option<PlacementV1> {
    if document.storage != WindowStateStorageV1::Ok {
        return None;
    }
    let stored = document.window.logical;
    let overlapping = monitors
        .iter()
        .copied()
        .find(|display| overlaps(display.work_area, stored));
    let target = overlapping
        .or(primary)
        .or_else(|| monitors.first().copied())?;
    let work = target.work_area;
    let logical = if overlapping.is_some() {
        // The position is kept, so the size is kept with it. A window spanning
        // two displays overlaps the first one, and clamping it to that work
        // area would silently cut off the part living on the second.
        RectV1 {
            x: stored.x,
            y: stored.y,
            width: stored.width.max(MINIMUM_WIDTH),
            height: stored.height.max(MINIMUM_HEIGHT),
        }
    } else {
        let width = stored
            .width
            .clamp(MINIMUM_WIDTH, work.width.max(MINIMUM_WIDTH));
        let height = stored
            .height
            .clamp(MINIMUM_HEIGHT, work.height.max(MINIMUM_HEIGHT));
        RectV1 {
            x: work.x + ((work.width - width) / 2.0).max(0.0),
            y: work.y + ((work.height - height) / 2.0).max(0.0),
            width,
            height,
        }
    };
    Some(PlacementV1 {
        logical,
        scale_factor: target.scale_factor,
        maximized: document.window.maximized
            && work.width >= MINIMUM_WIDTH
            && work.height >= MINIMUM_HEIGHT,
    })
}

/// Reads the stored record and decides the startup placement. A missing,
/// unreadable, or unopenable record leaves the configured window untouched.
#[must_use]
pub fn saved_placement(
    version: &str,
    monitors: &[DisplayV1],
    primary: Option<DisplayV1>,
) -> Option<PlacementV1> {
    let store = global_store(version).ok()?;
    restore_placement(&window_state(&store), monitors, primary)
}

/// Stores one captured geometry, best effort. A window drag is never worth
/// failing over, and the next capture writes the same information again.
pub fn save_window_state(version: &str, state: &WindowStateV1) {
    if let Ok(store) = global_store(version) {
        drop(set_window_state(&store, state));
    }
}

/// Reads and totally normalizes the stored window-state record.
#[must_use]
pub fn window_state(store: &GlobalStore) -> WindowStateDocumentV1 {
    store.config(WINDOW_STATE_KEY).map_or_else(
        |_| document(WindowStateStorageV1::Unreadable, defaults()),
        |stored| decode(&stored),
    )
}

/// Stores one captured geometry as the whole record. The read, the merge, and
/// the write share one transaction, so a torn write is impossible.
///
/// # Errors
/// Returns an error when the record cannot be written.
pub fn set_window_state(
    store: &GlobalStore,
    state: &WindowStateV1,
) -> AppResult<WindowStateDocumentV1> {
    store
        .update_config(WINDOW_STATE_KEY, |stored| {
            let current = decode(stored);
            // Captures fire on the first drag of the first window, so a record
            // this build cannot read is kept byte for byte: running an older
            // build must not destroy a newer one's geometry. Restoring already
            // ignores such a record, so the window opens at its configured
            // default until Settings ▸ Reset Window Layout clears the key.
            if current.storage == WindowStateStorageV1::Unreadable {
                return Ok((stored.to_vec(), current));
            }
            let merged = retained(&current, state);
            let record = serde_json::to_value(merged)
                .map_err(|error| StoreError::InvalidManifest(error.to_string()))?;
            let window = normalize(&record).unwrap_or_else(defaults);
            let encoded = serde_json::to_vec(&window)
                .map_err(|error| StoreError::InvalidManifest(error.to_string()))?;
            Ok((encoded, document(WindowStateStorageV1::Ok, window)))
        })
        .map_err(AppError::from)
}

/// A maximized or fullscreen window reports the rect it fills, not the rect it
/// restores down to, so the last windowed rect and the display it was captured
/// on are kept.
fn retained(current: &WindowStateDocumentV1, state: &WindowStateV1) -> WindowStateV1 {
    if (state.maximized || state.fullscreen) && current.storage == WindowStateStorageV1::Ok {
        return WindowStateV1 {
            logical: current.window.logical,
            scale_factor: current.window.scale_factor,
            display: current.window.display,
            ..*state
        };
    }
    *state
}

/// Totally normalizes the exact stored bytes. Empty bytes mean no record yet.
fn decode(stored: &[u8]) -> WindowStateDocumentV1 {
    if stored.is_empty() {
        return document(WindowStateStorageV1::Defaults, defaults());
    }
    serde_json::from_slice::<Value>(stored)
        .ok()
        .as_ref()
        .and_then(normalize)
        .map_or_else(
            || document(WindowStateStorageV1::Unreadable, defaults()),
            |window| document(WindowStateStorageV1::Ok, window),
        )
}

/// Returns the normalized record, or `None` when this build cannot read it.
fn normalize(value: &Value) -> Option<WindowStateV1> {
    let record = value.as_object()?;
    // A record without a readable version, or from a newer version, is
    // unreadable. An older version upgrades in memory and persists on write.
    if record.get("version").and_then(Value::as_u64)? > WINDOW_STATE_VERSION {
        return None;
    }
    let fallback = defaults();
    Some(WindowStateV1 {
        version: WINDOW_STATE_VERSION,
        logical: rect(record.get("logical"), fallback.logical),
        scale_factor: scale(record.get("scaleFactor")),
        maximized: flag(record.get("maximized")),
        fullscreen: flag(record.get("fullscreen")),
        display: display_of(record.get("display"), fallback.display),
    })
}

const fn document(storage: WindowStateStorageV1, window: WindowStateV1) -> WindowStateDocumentV1 {
    WindowStateDocumentV1 { storage, window }
}

const fn defaults() -> WindowStateV1 {
    let area = RectV1 {
        x: 0.0,
        y: 0.0,
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
    };
    WindowStateV1 {
        version: WINDOW_STATE_VERSION,
        logical: area,
        scale_factor: 1.0,
        maximized: false,
        fullscreen: false,
        display: DisplayV1 {
            work_area: area,
            scale_factor: 1.0,
        },
    }
}

fn display_of(value: Option<&Value>, fallback: DisplayV1) -> DisplayV1 {
    let section = value.and_then(Value::as_object);
    DisplayV1 {
        work_area: rect(field(section, "workArea"), fallback.work_area),
        scale_factor: scale(field(section, "scaleFactor")),
    }
}

fn rect(value: Option<&Value>, fallback: RectV1) -> RectV1 {
    let section = value.and_then(Value::as_object);
    RectV1 {
        x: coordinate(field(section, "x"), fallback.x),
        y: coordinate(field(section, "y"), fallback.y),
        width: length(field(section, "width"), fallback.width),
        height: length(field(section, "height"), fallback.height),
    }
}

fn field<'a>(section: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a Value> {
    section.and_then(|section| section.get(key))
}

fn coordinate(value: Option<&Value>, fallback: f64) -> f64 {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && value.abs() <= COORDINATE_MAXIMUM)
        .unwrap_or(fallback)
}

fn length(value: Option<&Value>, fallback: f64) -> f64 {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0 && *value <= COORDINATE_MAXIMUM)
        .unwrap_or(fallback)
}

fn flag(value: Option<&Value>) -> bool {
    value.and_then(Value::as_bool).unwrap_or(false)
}

fn scale(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).map_or(1.0, usable_scale)
}

fn usable_scale(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value.clamp(SCALE_MINIMUM, SCALE_MAXIMUM)
    } else {
        1.0
    }
}

/// Overlap along one axis, negative when the two spans are disjoint.
fn span(first: f64, first_length: f64, second: f64, second_length: f64) -> f64 {
    (first + first_length).min(second + second_length) - first.max(second)
}

fn overlaps(work_area: RectV1, window: RectV1) -> bool {
    span(work_area.x, work_area.width, window.x, window.width) >= OVERLAP_MINIMUM
        && span(work_area.y, work_area.height, window.y, window.height) >= OVERLAP_MINIMUM
}

/// Opens the global store for project-independent application state. The home
/// is the same fixed platform home the host resolved at startup.
fn global_store(version: &str) -> AppResult<GlobalStore> {
    let home = crate::resolve_global_home()?;
    let runtime = ActiveRuntime::load(&home, version)?.ok_or_else(|| {
        AppError::Message("p-track runtime is not initialized (run 'ptrack init')".to_owned())
    })?;
    let bindings = runtime.global_bindings(runtime.global_home())?;
    Ok(GlobalStore::open_existing(
        &bindings.global_database,
        &bindings.global_binding,
    )?)
}
