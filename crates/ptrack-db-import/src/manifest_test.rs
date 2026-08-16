use super::manifest::decode_manifest;

// The fixture source path must be absolute and clean on the platform running
// the test; both constants are the JSON-escaped text as it appears inside the
// manifest bytes.
#[cfg(unix)]
const SOURCE_PATH_JSON: &str = "/tmp/global.db";
#[cfg(windows)]
const SOURCE_PATH_JSON: &str = r"C:\\tmp\\global.db";
#[cfg(unix)]
const GO_ESCAPED_SOURCE_PATH_JSON: &str = r"/tmp/\u2028";
#[cfg(windows)]
const GO_ESCAPED_SOURCE_PATH_JSON: &str = r"C:\\tmp\\\u2028";

#[test]
fn manifest_requires_compact_closed_json_and_exact_counts() {
    let unix_valid = concat!(
        "{\"format\":\"ptrack-db-stage\",\"version\":\"1\",\"database_count\":\"1\",",
        "\"quarantine_count\":\"0\",\"registry\":[],\"databases\":[{\"id\":\"global\",\"kind\":\"global\",",
        "\"project_root\":null,\"source_path\":\"/tmp/global.db\",\"source_format\":\"0\",",
        "\"source_identity\":{\"device\":\"1\",\"inode\":\"2\",\"size\":\"3\",",
        "\"mtime_seconds\":\"-1\",\"mtime_nanos\":\"4\",\"sha256\":\"",
        "0000000000000000000000000000000000000000000000000000000000000000\"},",
        "\"data\":{\"path\":\"databases/0000-global.jsonl\",\"sha256\":\"",
        "0000000000000000000000000000000000000000000000000000000000000000\",",
        "\"bytes\":\"1\",\"record_count\":\"0\",\"bucket_count\":\"3\"}}]}\n"
    );
    #[cfg(unix)]
    let valid = unix_valid.to_owned();
    #[cfg(windows)]
    let valid = {
        let substituted = unix_valid.replacen("/tmp/global.db", SOURCE_PATH_JSON, 1);
        assert_ne!(substituted, unix_valid, "fixture path substitution missed");
        substituted
    };
    decode_manifest(valid.as_bytes()).expect("canonical manifest");

    let spaced = valid.replacen(":\"ptrack-db-stage\"", ": \"ptrack-db-stage\"", 1);
    assert_ne!(spaced, valid, "spaced mutation missed its target");
    decode_manifest(spaced.as_bytes()).expect("semantically canonical whitespace");
    let wrong_count = valid.replacen("\"database_count\":\"1\"", "\"database_count\":\"2\"", 1);
    assert_ne!(wrong_count, valid, "wrong_count mutation missed its target");
    assert!(decode_manifest(wrong_count.as_bytes()).is_err());
    let go_escaped = valid.replacen(SOURCE_PATH_JSON, GO_ESCAPED_SOURCE_PATH_JSON, 1);
    assert_ne!(go_escaped, valid, "go_escaped mutation missed its target");
    decode_manifest(go_escaped.as_bytes()).expect("Go-compatible escaped Unicode");
}
