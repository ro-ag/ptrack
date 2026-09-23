use crate::secrets::contains_url_credential;
use crate::{
    REDACTED_CREDENTIAL, contains_credential, contains_high_risk_secret,
    redact_credential_assignments, redact_credential_lines, redact_url_userinfo,
};

#[test]
fn credential_shapes_are_detected() {
    for value in [
        "password = secret",
        "toKen=secret",
        r#"{"password": "hunter2"}"#,
        r#"{"api_key":"abc123"}"#,
        r"'client_secret': 'abc'",
        "x-api-key: abc123",
        "api-key=abc123",
        "curl -H \"X-Api-Key: abc123\"",
        "Authorization: Basic dXNlcjpwYXNzd29yZA==",
        "Authorization: Token abc123",
        "authorization=bearer abc",
        "Proxy-Authorization: Bearer abc",
        "Bearer abcdefghijklmnopqrstuvwxyz0123",
        "DB_PASSWORD=hunter2",
        "export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        r#""SecretAccessKey": "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY""#,
        "OPENAI_API_KEY=abc123",
        "spring.datasource.password=hunter2",
        "GITHUB_TOKEN: abc",
        "mytoken: abc",
        "cookie: session=abc",
        "glpat-abcdefghijklmnopqrst",
        "xoxb-1234567890-abcdefghijkl",
        "xoxp-1234567890-abcdefghijkl",
        "sk_live_abcdefghijklmnop",
        "sk_test_abcdefghijklmnop",
        "sk-proj-abcdefghijklmnopqrstuvwx",
        "AIzaSyA1234567890abcdefghijklmnopqrstu",
        "ghp_abcdefghijklmnopqrstuvwxyz0123",
        "github_pat_abcdefghijklmnopqrstuvwxyz",
        "AKIAIOSFODNN7EXAMPLE",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
        "https://user:pass@example.com/repo",
        "postgres://app:hunter2@db.internal:5432/app",
        "-----BEGIN PRIVATE KEY-----\ncanary\n-----END PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----",
    ] {
        assert!(contains_credential(value), "missed credential in {value:?}");
    }
}

#[test]
fn ordinary_prose_is_not_a_credential() {
    for value in [
        "",
        "secret-store is a benign label",
        "Implement the password reset flow",
        "password reset flow: send the email first",
        "the token expires after 5 minutes",
        "tokenizer: fast",
        "max tokens: 4096",
        "secretary: Bob",
        "credentials are stored in the keychain",
        "credential.helper is osxkeychain",
        "The API key rotation runbook",
        "token:",
        "password:   ",
        "Bearer tokens are described in RFC 6750",
        "see https://example.com/docs?page=2#intro",
        "clone git@github.com:org/repo.git",
        "ssh://git@github.com/org/repo",
        "moved to https://",
        "sk-learn and task-abcdefghijklmnopqrstu",
        "3f2a9c1e0b7d4a6f8e2c1b0a9d8e7f6a5b4c3d2e",
        "src/components/HeaderView2/ButtonGroupItem.tsx",
    ] {
        assert!(!contains_credential(value), "false positive on {value:?}");
    }
}

#[test]
fn url_credential_scan_is_bounds_safe() {
    for (value, expected) in [
        ("moved to https://", false),
        ("://", false),
        ("a://@", false),
        ("a://:@", true),
        ("x://a@b://c", false),
        ("https:// user:pass@host", false),
        ("see https://a.example and ftp://u:p@b.example", true),
        ("https://a@b.example https://c@d.example https://", false),
        ("🦀 https://ü:ß@例え.jp 🦀", true),
        ("例え://", false),
        ("://例え", false),
        ("é://ü@", false),
    ] {
        assert_eq!(contains_url_credential(value), expected, "{value:?}");
        assert_eq!(contains_credential(value), expected, "{value:?}");
    }
}

#[test]
fn line_redaction_replaces_whole_lines_and_private_key_blocks() {
    assert_eq!(
        redact_credential_lines("safe\ntoken=TOP_SECRET\nsafe"),
        format!("safe\n{REDACTED_CREDENTIAL}\nsafe")
    );
    assert_eq!(
        redact_credential_lines(
            "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----\nafter"
        ),
        format!(
            "before\n{REDACTED_CREDENTIAL}\n{REDACTED_CREDENTIAL}\n{REDACTED_CREDENTIAL}\nafter"
        )
    );
    assert_eq!(redact_credential_lines("plain\nprose"), "plain\nprose");
}

#[test]
fn span_redaction_keeps_the_key_and_drops_the_value() {
    for (value, expected) in [
        ("Bearer TOP_SECRET", "Bearer [redacted]"),
        ("token=SECOND_SECRET", "token=[redacted]"),
        ("DB_PASSWORD=hunter2 next", "DB_PASSWORD=[redacted] next"),
        (
            "AWS_SECRET_ACCESS_KEY=wJalr/XUtn OPENAI_API_KEY=abc",
            "AWS_SECRET_ACCESS_KEY=[redacted] OPENAI_API_KEY=[redacted]",
        ),
        (r#""password": "hunter2""#, "password=[redacted]"),
        ("x-api-key: abc", "x-api-key=[redacted]"),
        ("Authorization: Basic dXNlcg==", "Authorization=[redacted]"),
        ("use glpat-abcdefghijklmnopqrst now", "use [redacted] now"),
        ("password reset flow", "password reset flow"),
    ] {
        assert_eq!(redact_credential_assignments(value), expected, "{value:?}");
    }
    assert_eq!(
        redact_url_userinfo("postgres://app:hunter2@db/app and https://a@b"),
        "postgres://[redacted]@db/app and https://a@b"
    );
}

#[test]
fn high_risk_secrets_are_provider_formats_and_private_keys_only() {
    assert!(contains_high_risk_secret("界sk-abcdefghijklmnop界"));
    assert!(contains_high_risk_secret("-----BEGIN PRIVATE KEY-----"));
    assert!(!contains_high_risk_secret("password=hunter2"));
    assert!(!contains_high_risk_secret(
        "Bearer abcdefghijklmnopqrstuvwxyz"
    ));
}
