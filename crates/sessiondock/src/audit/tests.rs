use super::*;
use serde_json::{Value, json};

#[test]
fn python_limits_and_sanitization_are_preserved() {
    let limits = Limits::default();
    assert_eq!(limits.body_bytes, 4 * 1024 * 1024);
    assert_eq!(limits.max_events, 100);
    assert_eq!(limits.queue_batches, 20_000);

    let value = json!({
        "password": "secret",
        "path": "/kept/like/python",
        "long": "x".repeat(20_000),
        "array": (0..500).collect::<Vec<_>>()
    });
    let clean = intake::sanitize(&value, 0);
    assert_eq!(clean["password"], "<redacted>");
    assert_eq!(clean["path"], "/kept/like/python");
    assert_eq!(clean["long"].as_str().unwrap().len(), 20_000);
    assert_eq!(clean["array"].as_array().unwrap().len(), 500);

    let mut nested = json!("leaf");
    for _ in 0..14 {
        nested = json!([nested]);
    }
    let clean = intake::sanitize(&nested, 0);
    let mut cursor = &clean;
    for _ in 0..13 {
        cursor = &cursor[0];
    }
    assert_eq!(cursor, "<depth-limit>");
}

#[test]
fn browser_intake_coerces_and_truncates_envelope_fields_like_python() {
    let sequence = AtomicU64::new(1);
    let body = json!({
        "page_id": 7,
        "uid": false,
        "events": [{"event": 42, "data": "not-an-object"}]
    });
    let prepared = intake::prepare(
        body.to_string().as_bytes(),
        "127.0.0.1".parse().unwrap(),
        100,
        &sequence,
        SystemTime::UNIX_EPOCH,
    )
    .unwrap();
    assert_eq!(prepared.skipped, 0);
    let line = prepared.batch.unwrap().bytes;
    let row: Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(row["event"], "browser.42");
    assert_eq!(row["page_id"], "7");
    assert_eq!(row["data"], json!({}));
}

#[test]
fn configured_directory_is_created_and_normalized() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested/audit");
    let normalized = validate_directory(&path).unwrap();
    assert!(normalized.is_dir());
}
