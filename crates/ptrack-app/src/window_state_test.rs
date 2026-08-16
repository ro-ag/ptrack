use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_store::{ActiveBinding, GlobalStore, StoreKind};
use serde_json::{Value, json};

use crate::window_state::{
    DisplayV1, PlacementV1, RectV1, WindowStateDocumentV1, WindowStateStorageV1, WindowStateV1,
    captured, logical_rect, physical_rect, restore_placement, set_window_state, window_state,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

const WINDOW_STATE_KEY: &[u8] = b"window-state";

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-window-state-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        ptrack_store::protect_private_directory(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn store(directory: &Temp) -> GlobalStore {
    let database = directory.0.join("global.redb");
    let binding = ActiveBinding {
        generation: 1,
        database_id: "window-state-test".to_owned(),
        kind: StoreKind::Global,
        canonical_path: database.clone(),
    };
    GlobalStore::create_new(&database, binding).unwrap()
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> RectV1 {
    RectV1 {
        x,
        y,
        width,
        height,
    }
}

fn display(work_area: RectV1, scale_factor: f64) -> DisplayV1 {
    DisplayV1 {
        work_area,
        scale_factor,
    }
}

/// A laptop display at scale 2.0, in logical pixels.
fn built_in() -> DisplayV1 {
    display(rect(0.0, 0.0, 1_728.0, 1_080.0), 2.0)
}

/// An external display to the right of the built-in one, at scale 1.0.
fn external() -> DisplayV1 {
    display(rect(1_728.0, 0.0, 2_560.0, 1_400.0), 1.0)
}

fn stored(document: &GlobalStore) -> Value {
    serde_json::to_value(window_state(document, "main")).unwrap()
}

fn record(state: WindowStateV1, storage: WindowStateStorageV1) -> WindowStateDocumentV1 {
    WindowStateDocumentV1 {
        storage,
        window: state,
    }
}

fn windowed(logical: RectV1) -> WindowStateV1 {
    captured(logical, 2.0, false, false, built_in())
}

fn defaults(storage: &str) -> Value {
    json!({
        "storage": storage,
        "version": 1,
        "logical": { "x": 0.0, "y": 0.0, "width": 1_440.0, "height": 900.0 },
        "scaleFactor": 1.0,
        "maximized": false,
        "fullscreen": false,
        "display": {
            "workArea": { "x": 0.0, "y": 0.0, "width": 1_440.0, "height": 900.0 },
            "scaleFactor": 1.0
        }
    })
}

#[test]
fn absent_record_reads_as_defaults_and_restores_nothing() {
    let directory = Temp::new("defaults");
    let store = store(&directory);
    assert_eq!(stored(&store), defaults("defaults"));
    assert!(store.config(WINDOW_STATE_KEY).unwrap().is_empty());
    assert_eq!(
        restore_placement(
            &window_state(&store, "main"),
            &[built_in()],
            Some(built_in())
        ),
        None
    );
}

#[test]
fn a_capture_stores_the_contract_shape_in_logical_coordinates() {
    let directory = Temp::new("capture");
    let store = store(&directory);
    let state = captured(
        rect(120.0, 80.0, 1_440.0, 900.0),
        2.0,
        false,
        false,
        display(rect(0.0, 0.0, 3_456.0, 2_160.0), 2.0),
    );
    let written = serde_json::to_value(set_window_state(&store, "main", &state).unwrap()).unwrap();
    let expected = json!({
        "storage": "ok",
        "version": 1,
        "logical": { "x": 120.0, "y": 80.0, "width": 1_440.0, "height": 900.0 },
        "scaleFactor": 2.0,
        "maximized": false,
        "fullscreen": false,
        "display": {
            "workArea": { "x": 0.0, "y": 0.0, "width": 3_456.0, "height": 2_160.0 },
            "scaleFactor": 2.0
        }
    });
    assert_eq!(written, expected);
    assert_eq!(stored(&store), expected);

    // The stored bytes are the record and nothing but the record.
    let mut record = expected;
    record.as_object_mut().unwrap().remove("storage");
    let bytes: Value = serde_json::from_slice(&store.config(WINDOW_STATE_KEY).unwrap()).unwrap();
    assert_eq!(bytes, record);
}

/// Contract section 5: a terminal window's capture must not overwrite the main
/// window's rect, and the record keeps one version and one transaction.
#[test]
fn per_window_entries_are_isolated_and_bounded() {
    let directory = Temp::new("per-window");
    let store = store(&directory);
    set_window_state(&store, "main", &windowed(rect(10.0, 20.0, 1_000.0, 700.0))).unwrap();

    // A window with no entry reads as defaults and restores nothing, so a
    // fresh terminal window opens at its configured geometry.
    assert_eq!(
        serde_json::to_value(window_state(&store, "terminal-1")).unwrap(),
        defaults("defaults")
    );
    assert_eq!(
        restore_placement(
            &window_state(&store, "terminal-1"),
            &[built_in()],
            Some(built_in())
        ),
        None
    );

    let popped = serde_json::to_value(
        set_window_state(
            &store,
            "terminal-1",
            &windowed(rect(400.0, 300.0, 900.0, 600.0)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(popped["storage"], "ok");
    assert_eq!(popped["logical"]["x"], 400.0);
    // The main window kept every field of its own rect.
    assert_eq!(
        serde_json::to_value(window_state(&store, "main")).unwrap()["logical"],
        json!({ "x": 10.0, "y": 20.0, "width": 1_000.0, "height": 700.0 })
    );

    // One version at the root, and the terminal entry carries none.
    let bytes: Value = serde_json::from_slice(&store.config(WINDOW_STATE_KEY).unwrap()).unwrap();
    assert_eq!(bytes["version"], 1);
    assert_eq!(bytes["terminal"]["version"], Value::Null);
    assert_eq!(bytes["terminal"]["logical"]["x"], 400.0);

    // Terminal windows are interchangeable and their labels restart at 1 every
    // run, so they share one entry: the record is bounded however many windows
    // a run pops out, and the last rect one was dragged to is the one the next
    // run's first pop-out opens at — whatever label it is minted with.
    for index in 2..=20 {
        set_window_state(
            &store,
            &format!("terminal-{index}"),
            &windowed(rect(f64::from(index), 0.0, 900.0, 600.0)),
        )
        .unwrap();
    }
    let bytes: Value = serde_json::from_slice(&store.config(WINDOW_STATE_KEY).unwrap()).unwrap();
    assert_eq!(bytes.as_object().unwrap().len(), 7);
    assert_eq!(bytes["logical"]["x"], 10.0);
    assert_eq!(bytes["terminal"]["logical"]["x"], 20.0);
    for label in ["terminal-1", "terminal-20", "terminal-99"] {
        assert_eq!(
            serde_json::to_value(window_state(&store, label)).unwrap()["logical"]["x"],
            20.0,
            "{label} must read the shared terminal entry"
        );
    }
}

/// A record written before the main window was ever captured has no main rect.
/// Reading defaults out of it as though they had been stored replays the
/// configured geometry as a placement: the first pop-out of a fresh install
/// would pin the main window to 0,0 at 1440×900 on the next launch.
#[test]
fn a_terminal_capture_never_materializes_a_main_window_rect() {
    let directory = Temp::new("terminal-first");
    let store = store(&directory);
    set_window_state(
        &store,
        "terminal-1",
        &windowed(rect(400.0, 300.0, 900.0, 600.0)),
    )
    .unwrap();

    assert_eq!(stored(&store), defaults("defaults"));
    assert_eq!(
        restore_placement(
            &window_state(&store, "main"),
            &[built_in()],
            Some(built_in())
        ),
        None
    );
    let bytes: Value = serde_json::from_slice(&store.config(WINDOW_STATE_KEY).unwrap()).unwrap();
    assert_eq!(bytes["logical"], Value::Null);
    assert_eq!(bytes["terminal"]["logical"]["x"], 400.0);

    // The main window's own capture still lands, and takes the terminal entry
    // with it rather than dropping it.
    set_window_state(&store, "main", &windowed(rect(10.0, 20.0, 1_000.0, 700.0))).unwrap();
    assert_eq!(stored(&store)["logical"]["x"], 10.0);
    assert_eq!(
        serde_json::to_value(window_state(&store, "terminal-2")).unwrap()["logical"]["x"],
        400.0
    );
}

/// A key that is not part of the record is dropped rather than kept, so a
/// garbage record cannot grow the document with arbitrary names.
#[test]
fn unknown_keys_are_dropped_by_normalization() {
    let directory = Temp::new("labels");
    let store = store(&directory);
    store
        .set_config(
            WINDOW_STATE_KEY,
            json!({
                "version": 1,
                "windows": { "terminal-1": { "logical": { "x": 9.0 } } },
                "../escape": { "logical": { "x": 9.0 } },
                "terminal": { "logical": { "x": 5.0, "y": 6.0, "width": 900.0, "height": 600.0 } }
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    set_window_state(&store, "main", &windowed(rect(1.0, 2.0, 1_000.0, 700.0))).unwrap();
    let bytes: Value = serde_json::from_slice(&store.config(WINDOW_STATE_KEY).unwrap()).unwrap();
    let mut keys = bytes.as_object().unwrap().keys().collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "display",
            "fullscreen",
            "logical",
            "maximized",
            "scaleFactor",
            "terminal",
            "version"
        ]
    );
    assert_eq!(bytes["logical"]["x"], 1.0);
    assert_eq!(bytes["terminal"]["logical"]["x"], 5.0);
}

#[test]
fn normalization_is_total_across_wrong_types_and_impossible_geometry() {
    let directory = Temp::new("normalize");
    let store = store(&directory);
    store
        .set_config(
            WINDOW_STATE_KEY,
            json!({
                "version": 1,
                "logical": { "x": "left", "y": 1e300, "width": 0, "height": -4 },
                "scaleFactor": 0,
                "maximized": "yes",
                "fullscreen": null,
                "display": { "workArea": [], "scaleFactor": 400 }
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    let normalized = stored(&store);
    assert_eq!(normalized["storage"], "ok");
    assert_eq!(normalized["logical"], defaults("ok")["logical"]);
    assert_eq!(normalized["scaleFactor"], 1.0);
    assert_eq!(normalized["maximized"], false);
    assert_eq!(normalized["fullscreen"], false);
    assert_eq!(normalized["display"]["workArea"], defaults("ok")["logical"]);
    assert_eq!(normalized["display"]["scaleFactor"], 16.0);
}

#[test]
fn unreadable_and_newer_records_read_as_defaults_and_are_never_rewritten() {
    let directory = Temp::new("unreadable");
    let store = store(&directory);
    for bytes in [
        br#"{"version":2,"logical":{"x":10,"y":10,"width":900,"height":600}}"#.to_vec(),
        br#"{"logical":{"x":10,"y":10,"width":900,"height":600}}"#.to_vec(),
        b"not json".to_vec(),
        br"[1440,900]".to_vec(),
    ] {
        store.set_config(WINDOW_STATE_KEY, &bytes).unwrap();
        assert_eq!(stored(&store), defaults("unreadable"));
        assert_eq!(store.config(WINDOW_STATE_KEY).unwrap(), bytes);
        // Nothing trustworthy to replay, so the configured window is left alone.
        assert_eq!(
            restore_placement(
                &window_state(&store, "main"),
                &[built_in()],
                Some(built_in())
            ),
            None
        );
    }
}

/// A window drag captures unprompted, so a capture onto a record this build
/// cannot read must leave the stored bytes alone: downgrading and grabbing the
/// window edge must not destroy a newer build's geometry.
#[test]
fn a_capture_onto_an_unreadable_record_keeps_the_stored_bytes() {
    let directory = Temp::new("unreadable-capture");
    let store = store(&directory);
    for bytes in [
        br#"{"version":2,"logical":{"x":10,"y":10,"width":900,"height":600}}"#.to_vec(),
        b"not json".to_vec(),
    ] {
        store.set_config(WINDOW_STATE_KEY, &bytes).unwrap();
        let written = set_window_state(&store, "main", &windowed(rect(40.0, 40.0, 1_000.0, 700.0)))
            .unwrap_or_else(|error| {
                panic!("an unreadable record must not fail the capture: {error}")
            });
        assert_eq!(
            serde_json::to_value(written).unwrap(),
            defaults("unreadable")
        );
        assert_eq!(store.config(WINDOW_STATE_KEY).unwrap(), bytes);
    }
}

#[test]
fn an_older_record_upgrades_in_memory_and_persists_on_the_next_capture() {
    let directory = Temp::new("upgrade");
    let store = store(&directory);
    store
        .set_config(
            WINDOW_STATE_KEY,
            br#"{"version":0,"logical":{"x":40,"y":40,"width":1000,"height":700}}"#,
        )
        .unwrap();
    let upgraded = stored(&store);
    assert_eq!(upgraded["storage"], "ok");
    assert_eq!(upgraded["version"], 1);
    assert_eq!(upgraded["logical"]["width"], 1_000.0);
    assert_eq!(
        store.config(WINDOW_STATE_KEY).unwrap(),
        br#"{"version":0,"logical":{"x":40,"y":40,"width":1000,"height":700}}"#
    );

    set_window_state(&store, "main", &windowed(rect(40.0, 40.0, 1_000.0, 700.0))).unwrap();
    let bytes: Value = serde_json::from_slice(&store.config(WINDOW_STATE_KEY).unwrap()).unwrap();
    assert_eq!(bytes["version"], 1);
}

#[test]
fn a_maximized_capture_keeps_the_last_windowed_rect() {
    let directory = Temp::new("maximized");
    let store = store(&directory);
    set_window_state(
        &store,
        "main",
        &windowed(rect(200.0, 150.0, 1_000.0, 700.0)),
    )
    .unwrap();

    // A maximized window reports the rect it fills, not the one it restores to.
    let filled = captured(
        rect(0.0, 0.0, 1_728.0, 1_080.0),
        2.0,
        true,
        false,
        built_in(),
    );
    let maximized =
        serde_json::to_value(set_window_state(&store, "main", &filled).unwrap()).unwrap();
    assert_eq!(maximized["maximized"], true);
    assert_eq!(
        maximized["logical"],
        json!({ "x": 200.0, "y": 150.0, "width": 1_000.0, "height": 700.0 })
    );

    // Fullscreen is stored, and it does not clobber the windowed rect either.
    let full = captured(
        rect(0.0, 0.0, 1_728.0, 1_117.0),
        2.0,
        false,
        true,
        built_in(),
    );
    let fullscreen =
        serde_json::to_value(set_window_state(&store, "main", &full).unwrap()).unwrap();
    assert_eq!(fullscreen["fullscreen"], true);
    assert_eq!(fullscreen["logical"]["width"], 1_000.0);
}

#[test]
fn an_overlapping_work_area_replays_the_stored_position() {
    let document = record(
        windowed(rect(1_800.0, 200.0, 1_200.0, 800.0)),
        WindowStateStorageV1::Ok,
    );
    assert_eq!(
        restore_placement(&document, &[built_in(), external()], Some(built_in())),
        Some(PlacementV1 {
            logical: rect(1_800.0, 200.0, 1_200.0, 800.0),
            scale_factor: 1.0,
            maximized: false,
        })
    );
}

/// A rect spanning both displays overlaps the built-in one first, so that is
/// the target display — but its work area must not cut the stored size, which
/// would shave the window a little narrower on every launch.
#[test]
fn a_window_spanning_two_displays_keeps_the_size_it_was_quit_at() {
    let document = record(
        windowed(rect(1_600.0, 100.0, 2_000.0, 800.0)),
        WindowStateStorageV1::Ok,
    );
    assert_eq!(
        restore_placement(&document, &[built_in(), external()], Some(built_in()))
            .unwrap()
            .logical,
        rect(1_600.0, 100.0, 2_000.0, 800.0)
    );
}

/// Contract section 8: display removal.
#[test]
fn a_removed_display_discards_the_position_and_centers_the_clamped_size() {
    let document = record(
        windowed(rect(2_400.0, 300.0, 2_000.0, 1_300.0)),
        WindowStateStorageV1::Ok,
    );
    // The external display is gone, so the stranded rect is recentered on the
    // primary display and its size is clamped to that work area.
    let placement = restore_placement(&document, &[built_in()], Some(built_in())).unwrap();
    assert_eq!(
        placement,
        PlacementV1 {
            logical: rect(0.0, 0.0, 1_728.0, 1_080.0),
            scale_factor: 2.0,
            maximized: false,
        }
    );

    // A smaller window keeps its size and is centered in the work area.
    let smaller = record(
        windowed(rect(2_400.0, 300.0, 1_228.0, 880.0)),
        WindowStateStorageV1::Ok,
    );
    assert_eq!(
        restore_placement(&smaller, &[built_in()], Some(built_in()))
            .unwrap()
            .logical,
        rect(250.0, 100.0, 1_228.0, 880.0)
    );
}

#[test]
fn a_barely_overlapping_rect_is_kept_and_one_pixel_less_is_discarded() {
    let work = built_in().work_area;
    let kept = record(
        windowed(rect(work.width - 64.0, 0.0, 1_200.0, 800.0)),
        WindowStateStorageV1::Ok,
    );
    assert_eq!(
        restore_placement(&kept, &[built_in()], Some(built_in()))
            .unwrap()
            .logical,
        rect(work.width - 64.0, 0.0, 1_200.0, 800.0)
    );

    let discarded = record(
        windowed(rect(work.width - 63.0, 0.0, 1_200.0, 800.0)),
        WindowStateStorageV1::Ok,
    );
    assert_eq!(
        restore_placement(&discarded, &[built_in()], Some(built_in()))
            .unwrap()
            .logical,
        rect(264.0, 140.0, 1_200.0, 800.0)
    );
}

#[test]
fn the_size_is_always_clamped_to_the_configured_minimum() {
    let tiny = record(
        windowed(rect(100.0, 100.0, 400.0, 200.0)),
        WindowStateStorageV1::Ok,
    );
    assert_eq!(
        restore_placement(&tiny, &[built_in()], Some(built_in()))
            .unwrap()
            .logical,
        rect(100.0, 100.0, 880.0, 560.0)
    );

    // A work area too small for the minimum never shrinks the window below it.
    let cramped = display(rect(0.0, 0.0, 600.0, 400.0), 1.0);
    assert_eq!(
        restore_placement(&tiny, &[cramped], Some(cramped))
            .unwrap()
            .logical,
        rect(100.0, 100.0, 880.0, 560.0)
    );
}

#[test]
fn fullscreen_is_never_restored_and_maximized_only_where_the_work_area_admits_it() {
    let state = captured(
        rect(100.0, 100.0, 1_200.0, 800.0),
        2.0,
        true,
        true,
        built_in(),
    );
    let document = record(state, WindowStateStorageV1::Ok);
    let placement = restore_placement(&document, &[built_in()], Some(built_in())).unwrap();
    // The placement carries no fullscreen at all, and maximized survives.
    assert!(placement.maximized);

    let cramped = display(rect(0.0, 0.0, 600.0, 400.0), 1.0);
    assert!(
        !restore_placement(&document, &[cramped], Some(cramped))
            .unwrap()
            .maximized
    );
}

/// Contract section 8: DPI change. A rect captured on a scale 2.0 display
/// replays at the same logical place on a scale 1.0 display.
#[test]
fn a_rect_captured_at_scale_two_restores_at_scale_one() {
    let physical = rect(400.0, 200.0, 2_880.0, 1_800.0);
    let logical = logical_rect(physical, 2.0);
    assert_eq!(logical, rect(200.0, 100.0, 1_440.0, 900.0));

    let document = record(
        captured(logical, 2.0, false, false, built_in()),
        WindowStateStorageV1::Ok,
    );
    let placement = restore_placement(&document, &[built_in()], Some(built_in())).unwrap();
    assert_eq!(
        physical_rect(placement.logical, placement.scale_factor),
        physical
    );

    // The same record on a scale 1.0 display keeps the logical rect and halves
    // the physical one.
    let single = display(built_in().work_area, 1.0);
    let replayed = restore_placement(&document, &[single], Some(single)).unwrap();
    assert_eq!(replayed.logical, logical);
    assert_eq!(
        physical_rect(replayed.logical, replayed.scale_factor),
        logical
    );
}

#[test]
fn an_unusable_scale_factor_is_treated_as_one() {
    for scale_factor in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            logical_rect(rect(10.0, 20.0, 30.0, 40.0), scale_factor),
            rect(10.0, 20.0, 30.0, 40.0)
        );
        assert_eq!(
            physical_rect(rect(10.0, 20.0, 30.0, 40.0), scale_factor),
            rect(10.0, 20.0, 30.0, 40.0)
        );
    }
}

#[test]
fn a_capture_with_no_display_at_all_still_restores_nothing() {
    let document = record(
        windowed(rect(100.0, 100.0, 1_200.0, 800.0)),
        WindowStateStorageV1::Ok,
    );
    assert_eq!(restore_placement(&document, &[], None), None);
}
