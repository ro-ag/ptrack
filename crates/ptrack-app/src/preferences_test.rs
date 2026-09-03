use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_store::{ActiveBinding, GlobalStore, StoreKind};
use serde_json::{Value, json};

use crate::preferences::{PreferencesStorageV1, preferences, reset_preferences, set_preferences};

static NEXT: AtomicU64 = AtomicU64::new(1);

const PREFERENCES_KEY: &[u8] = b"preferences";

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-preferences-{label}-{}-{}",
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
        database_id: "preferences-test".to_owned(),
        kind: StoreKind::Global,
        canonical_path: database.clone(),
    };
    GlobalStore::create_new(&database, binding).unwrap()
}

fn document(store: &GlobalStore) -> Value {
    serde_json::to_value(preferences(store)).unwrap()
}

fn defaults(storage: &str) -> Value {
    json!({
        "storage": storage,
        "version": 2,
        "appearance": { "theme": "system", "density": "comfortable", "reducedMotion": "system" },
        "terminal": {
            "defaultProfileId": null,
            "fontFamily": "monospace",
            "fontSize": 14,
            "unicodeMode": "modern",
            "scrollback": 25_000,
            "renderer": "auto"
        },
        "startup": { "restoreLastProject": false, "lastProjectRoot": null },
        "notifications": {
            "handoffArrival": false,
            "runFailureOrDrift": false,
            "runCompletion": false
        }
    })
}

#[test]
fn absent_record_reads_as_defaults_without_writing_anything() {
    let directory = Temp::new("defaults");
    let store = store(&directory);
    assert_eq!(document(&store), defaults("defaults"));
    assert!(store.config(PREFERENCES_KEY).unwrap().is_empty());
}

#[test]
fn normalization_is_total_across_unknown_enums_wrong_types_and_ranges() {
    let directory = Temp::new("normalize");
    let store = store(&directory);
    store
        .set_config(
            PREFERENCES_KEY,
            json!({
                "version": 1,
                "appearance": { "theme": "midnight", "density": 7, "reducedMotion": null },
                "terminal": {
                    "defaultProfileId": "   ",
                    "fontFamily": "",
                    "fontSize": 400,
                    "unicodeMode": "ancient",
                    "scrollback": -12,
                    "renderer": true
                }
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    let normalized = document(&store);
    assert_eq!(normalized["storage"], "ok");
    assert_eq!(normalized["appearance"], defaults("ok")["appearance"]);
    assert_eq!(normalized["terminal"]["defaultProfileId"], Value::Null);
    assert_eq!(normalized["terminal"]["fontFamily"], "monospace");
    assert_eq!(normalized["terminal"]["fontSize"], 24);
    assert_eq!(normalized["terminal"]["scrollback"], 1_000);
    assert_eq!(normalized["terminal"]["renderer"], "auto");

    store
        .set_config(
            PREFERENCES_KEY,
            br#"{"version":1,"terminal":{"fontSize":9,"scrollback":900000}}"#,
        )
        .unwrap();
    let clamped = document(&store);
    assert_eq!(clamped["terminal"]["fontSize"], 10);
    assert_eq!(clamped["terminal"]["scrollback"], 200_000);
}

#[test]
fn startup_restoration_is_off_by_default_and_the_last_project_root_clears_to_null() {
    let directory = Temp::new("startup");
    let store = store(&directory);
    assert_eq!(document(&store)["startup"], defaults("defaults")["startup"]);

    let opted_in = serde_json::to_value(
        set_preferences(
            &store,
            &json!({ "startup": { "restoreLastProject": true, "lastProjectRoot": "/work/app" } }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(opted_in["startup"]["restoreLastProject"], true);
    assert_eq!(opted_in["startup"]["lastProjectRoot"], "/work/app");

    // An explicit project close clears the root without touching the opt-in,
    // and a wrong type or a control character is not a path.
    for root in [json!(null), json!(7), json!("  "), json!("/work/\u{7}app")] {
        let cleared = serde_json::to_value(
            set_preferences(&store, &json!({ "startup": { "lastProjectRoot": root } })).unwrap(),
        )
        .unwrap();
        assert_eq!(cleared["startup"]["lastProjectRoot"], Value::Null);
        assert_eq!(cleared["startup"]["restoreLastProject"], true);
    }

    // A theme change disturbs neither.
    let themed = serde_json::to_value(
        set_preferences(&store, &json!({ "appearance": { "theme": "dark" } })).unwrap(),
    )
    .unwrap();
    assert_eq!(themed["startup"]["restoreLastProject"], true);
}

#[test]
fn notification_categories_are_independent_and_off_by_default() {
    let directory = Temp::new("notifications");
    let store = store(&directory);
    assert_eq!(
        document(&store)["notifications"],
        defaults("defaults")["notifications"]
    );

    let enabled = serde_json::to_value(
        set_preferences(
            &store,
            &json!({
                "notifications": {
                    "handoffArrival": true,
                    "runFailureOrDrift": "yes",
                    "runCompletion": true
                }
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(enabled["notifications"]["handoffArrival"], true);
    assert_eq!(enabled["notifications"]["runFailureOrDrift"], false);
    assert_eq!(enabled["notifications"]["runCompletion"], true);
}

#[test]
fn writes_merge_onto_the_whole_record_and_drop_unknown_members() {
    let directory = Temp::new("merge");
    let store = store(&directory);
    let first = serde_json::to_value(
        set_preferences(
            &store,
            &json!({
                "appearance": { "theme": "dark" },
                "terminal": { "fontSize": 18, "defaultProfileId": "zsh" },
                "unknown": { "nested": true },
                "version": 99
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(first["storage"], "ok");
    assert_eq!(first["version"], 2);
    assert_eq!(first["appearance"]["theme"], "dark");
    assert_eq!(first["terminal"]["fontSize"], 18);
    assert_eq!(first["terminal"]["defaultProfileId"], "zsh");

    let second = serde_json::to_value(
        set_preferences(&store, &json!({ "appearance": { "density": "compact" } })).unwrap(),
    )
    .unwrap();
    assert_eq!(second["appearance"]["theme"], "dark");
    assert_eq!(second["appearance"]["density"], "compact");
    assert_eq!(second["terminal"]["fontSize"], 18);

    // The stored document is the whole record and nothing but the record.
    let mut expected = second.clone();
    expected.as_object_mut().unwrap().remove("storage");
    let stored: Value = serde_json::from_slice(&store.config(PREFERENCES_KEY).unwrap()).unwrap();
    assert_eq!(stored, expected);
    assert_eq!(document(&store), second);
}

const ROUNDS: u64 = 200;

/// Writes one setting repeatedly and watches the other writer's setting, whose
/// value only ever grows. A merge onto a stale record carries an older value
/// forward, so the watched value is seen going backwards.
fn hammer(store: &GlobalStore, own: &str, other: &str) {
    let mut seen = String::new();
    for round in 0..ROUNDS {
        let value = format!("v{round:06}");
        let written = serde_json::to_value(
            set_preferences(store, &json!({ "terminal": { own: value } })).unwrap(),
        )
        .unwrap();
        assert_eq!(written["terminal"][own], value);
        let observed = written["terminal"][other]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            observed >= seen,
            "{other} went backwards: {seen} to {observed}"
        );
        seen = observed;
    }
}

#[test]
fn concurrent_writes_never_drop_each_others_settings() {
    let directory = Temp::new("concurrent");
    let store = store(&directory);
    std::thread::scope(|scope| {
        scope.spawn(|| hammer(&store, "fontFamily", "defaultProfileId"));
        hammer(&store, "defaultProfileId", "fontFamily");
    });
    let settled = document(&store);
    let last = format!("v{:06}", ROUNDS - 1);
    assert_eq!(settled["terminal"]["fontFamily"], last);
    assert_eq!(settled["terminal"]["defaultProfileId"], last);
}

#[test]
fn a_patch_must_be_an_object() {
    let directory = Temp::new("patch-kind");
    let store = store(&directory);
    assert!(set_preferences(&store, &json!("dark")).is_err());
    assert!(store.config(PREFERENCES_KEY).unwrap().is_empty());
}

#[test]
fn unreadable_and_newer_records_read_as_defaults_and_are_never_rewritten() {
    let directory = Temp::new("unreadable");
    let store = store(&directory);
    for record in [
        br#"{"version":3,"appearance":{"theme":"dark"}}"#.to_vec(),
        br#"{"appearance":{"theme":"dark"}}"#.to_vec(),
        b"not json".to_vec(),
        br#"["dark"]"#.to_vec(),
    ] {
        store.set_config(PREFERENCES_KEY, &record).unwrap();
        assert_eq!(document(&store), defaults("unreadable"));
        assert_eq!(store.config(PREFERENCES_KEY).unwrap(), record);
    }
}

#[test]
fn an_older_record_upgrades_in_memory_and_persists_on_the_next_write() {
    let directory = Temp::new("upgrade");
    let store = store(&directory);
    store
        .set_config(
            PREFERENCES_KEY,
            br#"{"version":0,"terminal":{"fontSize":20}}"#,
        )
        .unwrap();
    let upgraded = document(&store);
    assert_eq!(upgraded["storage"], "ok");
    assert_eq!(upgraded["version"], 2);
    assert_eq!(upgraded["notifications"], defaults("ok")["notifications"]);
    assert_eq!(upgraded["terminal"]["fontSize"], 20);
    // Reading never rewrites the record.
    assert_eq!(
        store.config(PREFERENCES_KEY).unwrap(),
        br#"{"version":0,"terminal":{"fontSize":20}}"#
    );

    set_preferences(&store, &json!({})).unwrap();
    let stored: Value = serde_json::from_slice(&store.config(PREFERENCES_KEY).unwrap()).unwrap();
    assert_eq!(stored["version"], 2);
    assert_eq!(stored["terminal"]["fontSize"], 20);
}

#[test]
fn reset_deletes_the_record_so_the_next_read_returns_defaults() {
    let directory = Temp::new("reset");
    let store = store(&directory);
    set_preferences(&store, &json!({ "appearance": { "theme": "light" } })).unwrap();
    assert!(!store.config(PREFERENCES_KEY).unwrap().is_empty());

    let reset = serde_json::to_value(reset_preferences(&store).unwrap()).unwrap();
    assert_eq!(reset, defaults("defaults"));
    assert!(store.config(PREFERENCES_KEY).unwrap().is_empty());
    assert_eq!(document(&store), defaults("defaults"));
    assert_eq!(preferences(&store).storage, PreferencesStorageV1::Defaults);
    reset_preferences(&store).unwrap();
}
