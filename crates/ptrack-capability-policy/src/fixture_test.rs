use serde::Deserialize;

use super::normalize::{canonical_scope_json, normalize};
use super::wire::{CapabilityWire, encode_digest};

#[derive(Deserialize)]
struct FixtureFile {
    fixtures: Vec<Fixture>,
}

#[derive(Deserialize)]
struct Fixture {
    name: String,
    draft: CapabilityWire,
    canonical_json: String,
    digest: String,
}

#[test]
fn go_produced_http_git_ssh_digest_fixtures_are_byte_exact() {
    let fixtures: FixtureFile = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/canonical_digest_fixtures.json"
    )))
    .unwrap();
    assert_eq!(fixtures.fixtures.len(), 3);
    for fixture in fixtures.fixtures {
        let draft = ptrack_core::Capability::try_from(fixture.draft).unwrap();
        let preview = normalize(&draft).unwrap();
        assert_eq!(
            canonical_scope_json(&preview.capability).unwrap(),
            fixture.canonical_json,
            "{} canonical JSON",
            fixture.name
        );
        assert_eq!(
            encode_digest(preview.scope_digest),
            fixture.digest,
            "{} digest",
            fixture.name
        );
    }
}
