use super::manifest::decode_manifest;

#[test]
fn manifest_requires_compact_closed_json_and_exact_counts() {
    let valid = concat!(
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
    decode_manifest(valid.as_bytes()).expect("canonical manifest");

    let spaced = valid.replacen(":\"ptrack-db-stage\"", ": \"ptrack-db-stage\"", 1);
    decode_manifest(spaced.as_bytes()).expect("semantically canonical whitespace");
    let wrong_count = valid.replacen("\"database_count\":\"1\"", "\"database_count\":\"2\"", 1);
    assert!(decode_manifest(wrong_count.as_bytes()).is_err());
    let go_escaped = valid.replacen("/tmp/global.db", "/tmp/\\u2028", 1);
    decode_manifest(go_escaped.as_bytes()).expect("Go-compatible escaped Unicode");
}
