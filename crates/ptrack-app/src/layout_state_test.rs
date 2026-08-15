use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_store::{ActiveBinding, GlobalStore, StoreKind};
use serde_json::{Value, json};

use crate::layout_state::{
    layout_state, reset_layout_state, reset_window_layout, set_layout_state,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

const LAYOUT_STATE_KEY: &[u8] = b"layout-state";
const WINDOW_STATE_KEY: &[u8] = b"window-state";

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-layout-{label}-{}-{}",
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
        database_id: "layout-test".to_owned(),
        kind: StoreKind::Global,
        canonical_path: database.clone(),
    };
    GlobalStore::create_new(&database, binding).unwrap()
}

fn document(store: &GlobalStore) -> Value {
    serde_json::to_value(layout_state(store)).unwrap()
}

fn defaults(storage: &str) -> Value {
    json!({
        "storage": storage,
        "version": 1,
        "sidebar": { "width": 248, "hidden": false },
        "panels": { "boardHidden": false, "terminalHidden": false },
        "projects": {}
    })
}

#[test]
fn absent_record_reads_as_defaults_without_writing_anything() {
    let directory = Temp::new("defaults");
    let store = store(&directory);
    assert_eq!(document(&store), defaults("defaults"));
    assert!(store.config(LAYOUT_STATE_KEY).unwrap().is_empty());
}

#[test]
fn normalization_is_total_across_unknown_views_wrong_types_and_ranges() {
    let directory = Temp::new("normalize");
    let store = store(&directory);
    store
        .set_config(
            LAYOUT_STATE_KEY,
            json!({
                "version": 1,
                "sidebar": { "width": 4_000, "hidden": "yes" },
                "panels": { "boardHidden": 7, "terminalHidden": true },
                "projects": {
                    "/a": { "view": "settings", "planId": -3, "foldedLanes": ["done", "moon", "done"] },
                    "/b": { "view": "overview", "planId": 13, "foldedLanes": "done" },
                    "": { "view": "board" },
                    "/c\u{7}": { "view": "board" }
                }
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    let normalized = document(&store);
    assert_eq!(normalized["storage"], "ok");
    assert_eq!(
        normalized["sidebar"],
        json!({ "width": 420, "hidden": false })
    );
    assert_eq!(
        normalized["panels"],
        json!({ "boardHidden": false, "terminalHidden": true })
    );
    // An unknown view falls back to the board, a negative plan hint to the
    // active plan, and an unknown lane is dropped along with its duplicate.
    assert_eq!(
        normalized["projects"]["/a"],
        json!({ "view": "board", "planId": 0, "foldedLanes": ["done"], "usedAt": 0 })
    );
    assert_eq!(
        normalized["projects"]["/b"],
        json!({ "view": "overview", "planId": 13, "foldedLanes": [], "usedAt": 0 })
    );
    // An empty or control-bearing project key is not a path.
    assert_eq!(normalized["projects"].as_object().unwrap().len(), 2);

    store
        .set_config(LAYOUT_STATE_KEY, br#"{"version":1,"sidebar":{"width":20}}"#)
        .unwrap();
    assert_eq!(document(&store)["sidebar"]["width"], 180);
}

#[test]
fn writes_merge_onto_the_whole_record_and_drop_unknown_members() {
    let directory = Temp::new("merge");
    let store = store(&directory);
    let first = serde_json::to_value(
        set_layout_state(
            &store,
            &json!({
                "sidebar": { "width": 300 },
                "projects": { "/a": { "view": "capabilities", "planId": 4 } },
                // The task drawer selection is deliberately not persisted.
                "taskId": 91,
                "version": 99
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(first["storage"], "ok");
    assert_eq!(first["version"], 1);
    assert_eq!(first["sidebar"]["width"], 300);
    assert_eq!(first["projects"]["/a"]["view"], "capabilities");
    assert_eq!(first["taskId"], Value::Null);

    let second = serde_json::to_value(
        set_layout_state(&store, &json!({ "panels": { "terminalHidden": true } })).unwrap(),
    )
    .unwrap();
    assert_eq!(second["sidebar"]["width"], 300);
    assert_eq!(second["projects"]["/a"]["planId"], 4);
    assert_eq!(second["panels"]["terminalHidden"], true);

    // The stored document is the whole record and nothing but the record.
    let mut expected = second.clone();
    expected.as_object_mut().unwrap().remove("storage");
    let stored: Value = serde_json::from_slice(&store.config(LAYOUT_STATE_KEY).unwrap()).unwrap();
    assert_eq!(stored, expected);
    assert_eq!(document(&store), second);
}

#[test]
fn the_project_map_is_bounded_to_thirty_two_least_recently_used_entries() {
    let directory = Temp::new("bounded");
    let store = store(&directory);
    for index in 0..40 {
        set_layout_state(
            &store,
            &json!({ "projects": { format!("/project-{index:02}"): { "planId": index } } }),
        )
        .unwrap();
    }
    let settled = document(&store);
    let projects = settled["projects"].as_object().unwrap();
    assert_eq!(projects.len(), 32);
    // The eight oldest are gone and the newest survived.
    assert!(!projects.contains_key("/project-07"));
    assert!(projects.contains_key("/project-08"));
    assert_eq!(projects["/project-39"]["planId"], 39);

    // Revisiting an old project keeps it past the next eviction round.
    set_layout_state(&store, &json!({ "projects": { "/project-08": {} } })).unwrap();
    for index in 40..48 {
        set_layout_state(
            &store,
            &json!({ "projects": { format!("/project-{index:02}"): {} } }),
        )
        .unwrap();
    }
    let revisited = document(&store);
    assert!(
        revisited["projects"]
            .as_object()
            .unwrap()
            .contains_key("/project-08")
    );
    assert!(
        !revisited["projects"]
            .as_object()
            .unwrap()
            .contains_key("/project-09")
    );
}

const ROUNDS: u64 = 200;

/// Writes one panel repeatedly and watches the other writer's project entry,
/// whose plan hint only ever grows. A merge onto a stale record carries an
/// older value forward, so the watched value is seen going backwards.
fn hammer(store: &GlobalStore, own: &str, other: &str) {
    let mut seen = 0;
    for round in 1..=ROUNDS {
        let written = serde_json::to_value(
            set_layout_state(store, &json!({ "projects": { own: { "planId": round } } })).unwrap(),
        )
        .unwrap();
        assert_eq!(written["projects"][own]["planId"], round);
        let observed = written["projects"][other]["planId"].as_u64().unwrap_or(0);
        assert!(
            observed >= seen,
            "{other} went backwards: {seen} to {observed}"
        );
        seen = observed;
    }
}

#[test]
fn concurrent_writes_never_drop_each_others_layout() {
    let directory = Temp::new("concurrent");
    let store = store(&directory);
    std::thread::scope(|scope| {
        scope.spawn(|| hammer(&store, "/left", "/right"));
        hammer(&store, "/right", "/left");
    });
    let settled = document(&store);
    assert_eq!(settled["projects"]["/left"]["planId"], ROUNDS);
    assert_eq!(settled["projects"]["/right"]["planId"], ROUNDS);
}

#[test]
fn a_patch_must_be_an_object() {
    let directory = Temp::new("patch-kind");
    let store = store(&directory);
    assert!(set_layout_state(&store, &json!("board")).is_err());
    assert!(store.config(LAYOUT_STATE_KEY).unwrap().is_empty());
}

#[test]
fn unreadable_and_newer_records_read_as_defaults_and_are_never_rewritten() {
    let directory = Temp::new("unreadable");
    let store = store(&directory);
    for record in [
        br#"{"version":2,"sidebar":{"width":300}}"#.to_vec(),
        br#"{"sidebar":{"width":300}}"#.to_vec(),
        b"not json".to_vec(),
        br#"["board"]"#.to_vec(),
    ] {
        store.set_config(LAYOUT_STATE_KEY, &record).unwrap();
        assert_eq!(document(&store), defaults("unreadable"));
        assert_eq!(store.config(LAYOUT_STATE_KEY).unwrap(), record);
    }
}

/// Layout writes fire automatically from the first snapshot load, so a write
/// onto a record this build cannot read must leave the stored bytes alone.
#[test]
fn a_write_onto_an_unreadable_record_keeps_the_stored_bytes() {
    let directory = Temp::new("unreadable-write");
    let store = store(&directory);
    for record in [
        br#"{"version":2,"sidebar":{"width":300}}"#.to_vec(),
        b"not json".to_vec(),
    ] {
        store.set_config(LAYOUT_STATE_KEY, &record).unwrap();
        let written = set_layout_state(&store, &json!({ "sidebar": { "width": 200 } }))
            .unwrap_or_else(|error| {
                panic!("an unreadable record must not fail the write: {error}")
            });
        assert_eq!(
            serde_json::to_value(written).unwrap(),
            defaults("unreadable")
        );
        assert_eq!(store.config(LAYOUT_STATE_KEY).unwrap(), record);
    }
}

#[test]
fn an_older_record_upgrades_in_memory_and_persists_on_the_next_write() {
    let directory = Temp::new("upgrade");
    let store = store(&directory);
    store
        .set_config(
            LAYOUT_STATE_KEY,
            br#"{"version":0,"sidebar":{"width":320}}"#,
        )
        .unwrap();
    let upgraded = document(&store);
    assert_eq!(upgraded["storage"], "ok");
    assert_eq!(upgraded["version"], 1);
    assert_eq!(upgraded["sidebar"]["width"], 320);
    // Reading never rewrites the record.
    assert_eq!(
        store.config(LAYOUT_STATE_KEY).unwrap(),
        br#"{"version":0,"sidebar":{"width":320}}"#
    );

    set_layout_state(&store, &json!({})).unwrap();
    let stored: Value = serde_json::from_slice(&store.config(LAYOUT_STATE_KEY).unwrap()).unwrap();
    assert_eq!(stored["version"], 1);
    assert_eq!(stored["sidebar"]["width"], 320);
}

#[test]
fn resetting_the_window_layout_clears_both_records_and_leaves_project_data_alone() {
    let directory = Temp::new("reset");
    let store = store(&directory);
    set_layout_state(&store, &json!({ "sidebar": { "hidden": true } })).unwrap();
    store
        .set_config(WINDOW_STATE_KEY, br#"{"version":1}"#)
        .unwrap();
    store
        .set_config(b"preferences", br#"{"version":1}"#)
        .unwrap();

    let reset = serde_json::to_value(reset_window_layout(&store).unwrap()).unwrap();
    assert_eq!(reset, defaults("defaults"));
    assert!(store.config(LAYOUT_STATE_KEY).unwrap().is_empty());
    assert!(store.config(WINDOW_STATE_KEY).unwrap().is_empty());
    // The window layout reset is not an application-state reset.
    assert_eq!(store.config(b"preferences").unwrap(), br#"{"version":1}"#);
    assert_eq!(document(&store), defaults("defaults"));

    // Both resets are idempotent.
    reset_window_layout(&store).unwrap();
    assert_eq!(
        serde_json::to_value(reset_layout_state(&store).unwrap()).unwrap(),
        defaults("defaults")
    );
}
