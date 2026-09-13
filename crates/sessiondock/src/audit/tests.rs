use std::{
    net::{IpAddr, Ipv4Addr},
    path::Path,
    sync::{Arc, atomic::AtomicU64},
    time::{Duration, Instant, SystemTime},
};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    AuditService, Gate, Limits, Outcome, Refusal, Rejection,
    intake::{self, looks_like_absolute_path, sanitize, valid_event_name},
    limiter::Limiter,
    writer::{list_segments, parse_segment_name, segment_name},
};

const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn private_dir(root: &Path, name: &str) -> std::path::PathBuf {
    let path = root.join(name);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&path).unwrap();
    path
}

fn small_limits() -> Limits {
    Limits {
        shutdown_deadline: Duration::from_secs(3),
        ..Limits::default()
    }
}

fn service(directory: &Path, limits: Limits) -> AuditService {
    AuditService::open(directory.to_path_buf(), limits, CancellationToken::new()).unwrap()
}

fn post(service: &AuditService, body: &Value) -> Outcome {
    let body = body.to_string();
    let admission = service.admit(CLIENT, Some(body.len())).unwrap();
    service.submit(admission, CLIENT, body.as_bytes()).unwrap()
}

fn batch(names: &[&str]) -> Value {
    json!({"page_id": "page-1", "uid": "claude:abc", "_build": "build-1", "events":
        names.iter().map(|name| json!({"event": name, "ts": "2026-09-12T00:00:00.000Z",
            "data": {"n": 1}, "content": {"text": "never stored"}})).collect::<Vec<_>>()})
}

fn wait_until(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    condition()
}

fn lines(directory: &Path) -> Vec<Value> {
    let mut segments = list_segments(directory);
    segments.sort();
    segments
        .iter()
        .flat_map(|segment| {
            std::fs::read_to_string(&segment.path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn prepare(body: &Value, max_events: usize) -> Result<intake::Prepared, Rejection> {
    intake::prepare(
        body.to_string().as_bytes(),
        CLIENT,
        max_events,
        Limits::default().data_bytes,
        &AtomicU64::new(1),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
    )
}

fn records(prepared: &intake::Prepared) -> Vec<Value> {
    prepared
        .batch
        .as_ref()
        .map(|batch| {
            std::str::from_utf8(&batch.bytes)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn event_names_follow_the_python_pattern() {
    for name in [
        "page.loaded",
        "http.response.parsed",
        "a",
        "x_y:z-1",
        &"n".repeat(160),
    ] {
        assert!(valid_event_name(name), "{name}");
    }
    for name in ["", " page", "page loaded", "页面", "a/b", &"n".repeat(161)] {
        assert!(!valid_event_name(name), "{name:?}");
    }
}

#[test]
fn structural_errors_reject_and_invalid_events_are_skipped() {
    for body in [
        "not json",
        "[]",
        "{}",
        r#"{"events": null}"#,
        r#"{"events": {}}"#,
        r#"{"events": [1]}"#,
        r#"{"events": [], "page_id": 7}"#,
    ] {
        let result = intake::prepare(
            body.as_bytes(),
            CLIENT,
            100,
            8192,
            &AtomicU64::new(1),
            SystemTime::UNIX_EPOCH,
        );
        assert!(
            matches!(result, Err(Rejection::Malformed(_))),
            "{body} should be malformed"
        );
    }
    let long = "p".repeat(129);
    assert!(matches!(
        prepare(&json!({"page_id": long, "events": []}), 100),
        Err(Rejection::Malformed(_))
    ));
    assert!(matches!(
        prepare(&batch(&["a", "b", "c"]), 2),
        Err(Rejection::TooManyEvents)
    ));
    let empty = prepare(&json!({"events": []}), 100).unwrap();
    assert!(empty.batch.is_none());
    assert_eq!(empty.skipped, 0);

    let mixed = json!({"page_id": "page", "uid": "codex:root", "events": [
        {"event": "ok.one", "ts": "2026-09-12T00:00:00Z", "data": {"k": "v"}},
        {"event": "bad name", "data": {}},
        {"event": 5},
        {"ts": "no-name"},
        {"event": "ok.two", "ts": 12345},
        {"event": "ok.three", "data": "not-an-object"},
        {"event": "ok.four", "uid": "grok:own", "severity": "error", "trace_id": "t-1",
            "build": "event-build", "data": null},
        {"event": "ok.five", "uid": "u".repeat(513)},
        {"event": "ok.six", "ts": "t".repeat(65)},
    ]});
    let prepared = prepare(&mixed, 100).unwrap();
    assert_eq!(prepared.skipped, 7);
    let rows = records(&prepared);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["event"], "browser.ok.one");
    assert_eq!(rows[0]["uid"], "codex:root");
    assert_eq!(rows[0]["source"], "codex");
    assert_eq!(rows[0]["page_id"], "page");
    assert_eq!(rows[0]["severity"], "info");
    assert_eq!(rows[0]["client_ts"], "2026-09-12T00:00:00Z");
    assert_eq!(rows[0]["received_at"], "2027-01-15T08:00:00.000Z");
    assert_eq!(rows[0]["client"], "127.0.0.1");
    assert_eq!(rows[0]["data"], json!({"k": "v"}));
    assert_eq!(rows[1]["event"], "browser.ok.four");
    assert_eq!(rows[1]["uid"], "grok:own");
    assert_eq!(rows[1]["source"], "grok");
    assert_eq!(rows[1]["severity"], "error");
    assert_eq!(rows[1]["trace_id"], "t-1");
    assert_eq!(rows[1]["build"], "event-build");
    assert_eq!(rows[1]["client_ts"], Value::Null);
    assert_eq!(rows[1]["data"], json!({}));
    assert!(rows[1]["seq"].as_u64().unwrap() > rows[0]["seq"].as_u64().unwrap());
}

#[test]
fn content_and_unknown_fields_are_never_retained() {
    let secret = "SECRET-MESSAGE-BODY-".repeat(2000);
    let body = json!({"page_id": "p", "events": [{"event": "dom.snapshot",
        "data": {"reason": "render"}, "content": {"composer": secret, "messages": [secret]},
        "unknown": secret}]});
    let prepared = prepare(&body, 100).unwrap();
    let batch = prepared.batch.unwrap();
    assert_eq!(batch.events, 1);
    assert!(batch.bytes.len() < 1024, "{}", batch.bytes.len());
    let text = std::str::from_utf8(&batch.bytes).unwrap();
    assert!(!text.contains("SECRET-MESSAGE-BODY"));
    assert!(!text.contains("content"));
    assert!(!text.contains("unknown"));
}

#[test]
fn sanitizer_redacts_scrubs_and_bounds() {
    let deep = (0..12).fold(json!("leaf"), |inner, _| json!({"next": inner}));
    let big_array: Vec<Value> = (0..300).map(|i| json!(i)).collect();
    let big_object: serde_json::Map<String, Value> =
        (0..140).map(|i| (format!("k{i}"), json!(i))).collect();
    let value = json!({
        "Authorization": "Bearer abc", "api_key": "k", "ACCESS-TOKEN": "t", "token": "t",
        "cwd": "/home/someone/project", "home": "~/x", "drive": "C:\\Users\\x",
        "unc": "\\\\server\\share", "url": "/?sid=claude:abc", "root": "/",
        "relative": "api/messages/claude:abc?start=0", "http": "http://127.0.0.1/app.js",
        "stack": "s".repeat(2000), "nested": deep, "array": big_array, "object": big_object,
        "number": 1.5, "flag": true, "nothing": null
    });
    let clean = sanitize(&value, 0);
    for key in ["Authorization", "api_key", "ACCESS-TOKEN", "token"] {
        assert_eq!(clean[key], "<redacted>", "{key}");
    }
    for key in ["cwd", "home", "drive", "unc"] {
        assert_eq!(clean[key], "<path>", "{key}");
    }
    assert_eq!(clean["url"], "/?sid=claude:abc");
    assert_eq!(clean["root"], "/");
    assert_eq!(clean["relative"], "api/messages/claude:abc?start=0");
    assert_eq!(clean["http"], "http://127.0.0.1/app.js");
    let stack = clean["stack"].as_str().unwrap();
    assert_eq!(stack.chars().count(), 1025);
    assert!(stack.ends_with('…'));
    let mut cursor = &clean["nested"];
    for _ in 0..8 {
        cursor = &cursor["next"];
    }
    assert_eq!(cursor, "<depth-limit>");
    let array = clean["array"].as_array().unwrap();
    assert_eq!(array.len(), 257);
    assert_eq!(array[256], json!({"truncated": true, "items": 300}));
    let object = clean["object"].as_object().unwrap();
    assert_eq!(object.len(), 129);
    assert_eq!(object["<truncated-keys>"], 140);
    assert_eq!(clean["number"], 1.5);
    assert_eq!(clean["flag"], true);
    assert_eq!(clean["nothing"], Value::Null);
    assert!(!looks_like_absolute_path("//double"));
    assert!(!looks_like_absolute_path("/ spaced"));
    assert!(looks_like_absolute_path("/tmp/x"));
    assert!(looks_like_absolute_path("d:/x"));

    let mut oversized = serde_json::Map::new();
    for i in 0..40 {
        oversized.insert(format!("key{i}"), json!("v".repeat(500)));
    }
    let bounded = intake::bounded_data(&Value::Object(oversized), 8192);
    assert_eq!(bounded["truncated"], true);
    assert!(bounded["bytes"].as_u64().unwrap() > 8192);
    assert_eq!(bounded["keys"].as_array().unwrap().len(), 32);
    assert_eq!(bounded["keys"][0], "key0");
}

#[test]
fn segment_names_round_trip_and_reject_foreign_files() {
    assert_eq!(segment_name("2026-09-12", 0), "browser-2026-09-12.jsonl");
    assert_eq!(
        segment_name("2026-09-12", 7),
        "browser-2026-09-12.0007.jsonl"
    );
    assert_eq!(
        parse_segment_name("browser-2026-09-12.jsonl"),
        Some(("2026-09-12".into(), 0))
    );
    assert_eq!(
        parse_segment_name("browser-2026-09-12.0012.jsonl"),
        Some(("2026-09-12".into(), 12))
    );
    for name in [
        "browser-2026-09-12.12.jsonl",
        "browser-2026-9-12.jsonl",
        "browser-2026-09-12.jsonl.bak",
        "other-2026-09-12.jsonl",
        "browser-2026-09-12.abcd.jsonl",
        "notes.txt",
    ] {
        assert_eq!(parse_segment_name(name), None, "{name}");
    }
    let mut names = [
        parse_segment_name("browser-2026-09-12.0002.jsonl").unwrap(),
        parse_segment_name("browser-2026-09-13.jsonl").unwrap(),
        parse_segment_name("browser-2026-09-12.jsonl").unwrap(),
        parse_segment_name("browser-2026-09-12.0010.jsonl").unwrap(),
    ];
    names.sort();
    assert_eq!(
        names
            .iter()
            .map(|(date, index)| format!("{date}/{index}"))
            .collect::<Vec<_>>(),
        [
            "2026-09-12/0",
            "2026-09-12/2",
            "2026-09-12/10",
            "2026-09-13/0"
        ]
    );
}

#[test]
fn writer_rotates_by_size_and_enforces_byte_retention() {
    let temp = tempfile::tempdir().unwrap();
    let directory = private_dir(temp.path(), "audit");
    std::fs::write(directory.join("notes.txt"), b"operator notes").unwrap();
    std::fs::write(directory.join("browser-2020-01-01.jsonl"), vec![b'x'; 900]).unwrap();
    let limits = Limits {
        file_bytes: 1200,
        retained_bytes: 3000,
        ..small_limits()
    };
    let service = service(&directory, limits);
    for round in 0..12u64 {
        let outcome = post(&service, &batch(&[&format!("round.{round}")]));
        assert_eq!(
            outcome,
            Outcome {
                accepted: 1,
                skipped: 0,
                dropped: false
            }
        );
        assert!(wait_until(Duration::from_secs(5), || {
            service.snapshot().written_events == round + 1
        }));
    }
    let snapshot = service.snapshot();
    assert_eq!(snapshot.dropped_events, 0);
    assert_eq!(snapshot.write_errors, 0);
    let mut segments = list_segments(&directory);
    segments.sort();
    assert!(segments.len() >= 2, "{segments:?}");
    assert!(
        segments.iter().all(|segment| segment.size <= 1200),
        "every batch is far below the rotation threshold: {segments:?}"
    );
    let total: u64 = segments.iter().map(|segment| segment.size).sum();
    assert!(total <= 3000, "{total}");
    assert_eq!(snapshot.retained_bytes, total);
    assert!(
        !directory.join("browser-2020-01-01.jsonl").exists(),
        "oldest segment is pruned first"
    );
    assert_eq!(
        std::fs::read(directory.join("notes.txt")).unwrap(),
        b"operator notes"
    );
    let rows = lines(&directory);
    let last = rows.last().unwrap();
    assert_eq!(last["event"], "browser.round.11");
    assert!(rows.iter().all(|row| row.get("content").is_none()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for segment in &segments {
            let mode = std::fs::metadata(&segment.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{}", segment.path.display());
        }
    }
    // Restart appends to the latest segment of the day instead of leaking a new one.
    drop(service);
    let count_before = list_segments(&directory).len();
    let service = AuditService::open(
        directory.clone(),
        Limits {
            file_bytes: 1200,
            retained_bytes: 3000,
            ..small_limits()
        },
        CancellationToken::new(),
    )
    .unwrap();
    post(&service, &batch(&["after.restart"]));
    assert!(wait_until(Duration::from_secs(5), || service
        .snapshot()
        .written_events
        == 1));
    assert!(list_segments(&directory).len() <= count_before + 1);
}

#[test]
fn queue_saturation_drops_with_counters_and_never_blocks() {
    let temp = tempfile::tempdir().unwrap();
    let directory = private_dir(temp.path(), "audit");
    let gate = Arc::new(Gate::default());
    gate.hold();
    let limits = Limits {
        queue_batches: 2,
        gate: Some(gate.clone()),
        ..small_limits()
    };
    let service = service(&directory, limits);
    let mut outcomes = Vec::new();
    let started = Instant::now();
    for round in 0..6 {
        outcomes.push(post(
            &service,
            &batch(&[&format!("held.{round}"), "second"]),
        ));
    }
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    // Two batches fit the channel; the writer may have taken at most one more.
    let dropped: Vec<_> = outcomes.iter().filter(|outcome| outcome.dropped).collect();
    assert!(dropped.len() >= 3 && dropped.len() <= 4, "{outcomes:?}");
    assert!(
        outcomes
            .iter()
            .filter(|outcome| !outcome.dropped)
            .all(|outcome| outcome.accepted == 2 && outcome.skipped == 0),
        "{outcomes:?}"
    );
    // A batch refused at admission was never parsed, so it reports no events;
    // only the rare post-parse race (queue filled meanwhile) reports two.
    assert!(
        dropped
            .iter()
            .all(|outcome| outcome.accepted == 0 || outcome.accepted == 2),
        "{outcomes:?}"
    );
    let snapshot = service.snapshot();
    assert_eq!(snapshot.dropped_batches as usize, dropped.len());
    assert_eq!(
        snapshot.dropped_events as usize,
        dropped
            .iter()
            .map(|outcome| outcome.accepted)
            .sum::<usize>()
    );
    assert_eq!(snapshot.accepted_batches as usize, 6 - dropped.len());
    assert_eq!(snapshot.written_events, 0);
    assert!(snapshot.queued_batches >= 2 || snapshot.queued_bytes > 0);
    // Saturated bytes are refused before the body is read at all.
    let tiny = service.admit(CLIENT, Some(10)).unwrap();
    let saturated_by_items = tiny.dropped();
    drop(tiny);
    gate.release();
    let accepted = snapshot.accepted_events;
    assert!(wait_until(Duration::from_secs(5), || {
        service.snapshot().written_events == accepted
    }));
    let after = service.snapshot();
    assert_eq!(after.queued_batches, 0);
    assert_eq!(after.queued_bytes, 0);
    assert_eq!(after.written_bytes, after.retained_bytes);
    let rows = lines(&directory);
    assert_eq!(rows.len() as u64, accepted);
    assert_eq!(rows[0]["event"], "browser.held.0");
    if saturated_by_items {
        assert!(after.dropped_batches as usize > dropped.len());
    }
}

#[test]
fn byte_budget_is_reserved_from_the_declared_length_before_reading() {
    let temp = tempfile::tempdir().unwrap();
    let directory = private_dir(temp.path(), "audit");
    let gate = Arc::new(Gate::default());
    gate.hold();
    let limits = Limits {
        queue_bytes: 4096,
        gate: Some(gate.clone()),
        ..small_limits()
    };
    let service = service(&directory, limits);
    let bound = service.limits().batch_bytes();
    assert!(bound > 4096);
    // Unknown length is estimated at the batch bound and refused outright.
    assert!(service.admit(CLIENT, None).unwrap().dropped());
    assert!(service.admit(CLIENT, Some(5000)).unwrap().dropped());
    assert_eq!(service.snapshot().dropped_batches, 2);
    let body = batch(&["small"]).to_string();
    let admission = service.admit(CLIENT, Some(body.len())).unwrap();
    assert!(!admission.dropped());
    assert_eq!(service.snapshot().queued_bytes as usize, body.len());
    // Abandoning the admission (for example a 400) releases the reservation.
    drop(admission);
    assert_eq!(service.snapshot().queued_bytes, 0);
    let admission = service.admit(CLIENT, Some(body.len())).unwrap();
    let outcome = service.submit(admission, CLIENT, body.as_bytes()).unwrap();
    assert!(!outcome.dropped);
    // The reservation shrinks from the declared length to the prepared snapshot.
    let queued = service.snapshot().queued_bytes;
    let expected = prepare(&batch(&["small"]), 100)
        .unwrap()
        .batch
        .unwrap()
        .bytes
        .len();
    assert_eq!(queued as usize, expected);
    assert_ne!(queued as usize, body.len());
    // A malformed body after admission counts as rejected and holds nothing.
    let admission = service.admit(CLIENT, Some(3)).unwrap();
    assert_eq!(
        service.submit(admission, CLIENT, b"{{{").unwrap_err(),
        Rejection::Malformed("需要包含 events 数组的 JSON 对象")
    );
    assert_eq!(service.snapshot().rejected_requests, 1);
    assert_eq!(service.snapshot().queued_bytes, queued);
    gate.release();
}

#[tokio::test]
async fn shutdown_drains_within_the_deadline_and_closes_admission() {
    let temp = tempfile::tempdir().unwrap();
    let directory = private_dir(temp.path(), "audit");
    let gate = Arc::new(Gate::default());
    gate.hold();
    let service = service(
        &directory,
        Limits {
            gate: Some(gate.clone()),
            ..small_limits()
        },
    );
    for round in 0..5 {
        assert!(!post(&service, &batch(&[&format!("flush.{round}")])).dropped);
    }
    assert_eq!(service.snapshot().written_events, 0);
    gate.release();
    let started = Instant::now();
    assert!(service.shutdown().await);
    assert!(started.elapsed() < Duration::from_secs(3));
    let snapshot = service.snapshot();
    assert_eq!(snapshot.written_events, 5);
    assert_eq!(snapshot.dropped_events, 0);
    assert_eq!(lines(&directory).len(), 5);
    assert_eq!(service.admit(CLIENT, Some(1)).unwrap_err(), Refusal::Closed);
    assert!(service.shutdown().await, "repeatable");
}

#[tokio::test]
async fn cancellation_token_alone_stops_the_writer() {
    let temp = tempfile::tempdir().unwrap();
    let directory = private_dir(temp.path(), "audit");
    let token = CancellationToken::new();
    let service = AuditService::open(directory.clone(), small_limits(), token.clone()).unwrap();
    assert!(!post(&service, &batch(&["before.cancel"])).dropped);
    token.cancel();
    assert!(service.shutdown().await);
    assert_eq!(service.snapshot().written_events, 1);
    assert_eq!(service.admit(CLIENT, Some(1)).unwrap_err(), Refusal::Closed);
}

#[test]
fn limiter_refills_per_client_and_bounds_its_map() {
    let mut limiter = Limiter::new(2, 10.0);
    let start = Instant::now();
    assert_eq!(limiter.take(CLIENT, start), None);
    assert_eq!(limiter.take(CLIENT, start), None);
    let wait = limiter.take(CLIENT, start).unwrap();
    assert!(
        wait > Duration::ZERO && wait <= Duration::from_millis(100),
        "{wait:?}"
    );
    let other = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));
    assert_eq!(limiter.take(other, start), None);
    assert_eq!(
        limiter.take(CLIENT, start + Duration::from_millis(100)),
        None
    );
    assert!(
        limiter
            .take(CLIENT, start + Duration::from_millis(100))
            .is_some()
    );
    // Refill never exceeds the burst capacity.
    let later = start + Duration::from_secs(60);
    assert_eq!(limiter.take(CLIENT, later), None);
    assert_eq!(limiter.take(CLIENT, later), None);
    assert!(limiter.take(CLIENT, later).is_some());
    for octet in 0..70u8 {
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, octet));
        assert_eq!(
            limiter.take(ip, later + Duration::from_millis(u64::from(octet))),
            None
        );
    }
    // The stalest bucket was evicted, so the exhausted client starts fresh.
    assert_eq!(
        limiter.take(CLIENT, later + Duration::from_millis(70)),
        None
    );
}

#[test]
fn service_refuses_rate_limited_and_busy_clients_before_reading() {
    let temp = tempfile::tempdir().unwrap();
    let directory = private_dir(temp.path(), "audit");
    let service = service(
        &directory,
        Limits {
            burst: 3,
            rate_per_second: 0.001,
            in_flight: 1,
            ..small_limits()
        },
    );
    let first = service.admit(CLIENT, Some(1)).unwrap();
    // The token bucket is charged first, then the single parse slot is busy.
    assert_eq!(service.admit(CLIENT, Some(1)).unwrap_err(), Refusal::Busy);
    drop(first);
    let _second = service.admit(CLIENT, Some(1)).unwrap();
    match service.admit(CLIENT, Some(1)).unwrap_err() {
        Refusal::RateLimited { retry_after } => assert!(retry_after >= Duration::from_secs(1)),
        other => panic!("{other:?}"),
    }
    assert_eq!(service.snapshot().rate_limited_requests, 1);
}

#[test]
fn directory_validation_requires_private_absolute_directories() {
    let temp = tempfile::tempdir().unwrap();
    let directory = private_dir(temp.path(), "audit");
    assert_eq!(
        super::validate_directory(&directory).unwrap(),
        directory.canonicalize().unwrap()
    );
    for invalid in [
        std::path::PathBuf::new(),
        std::path::PathBuf::from("relative"),
        temp.path().join("missing"),
        temp.path().join("audit/../audit"),
    ] {
        assert_eq!(
            super::validate_directory(&invalid).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput,
            "{}",
            invalid.display()
        );
    }
    let file = temp.path().join("file");
    std::fs::write(&file, b"").unwrap();
    assert!(super::validate_directory(&file).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert_eq!(
            super::validate_directory(&directory).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let alias = temp.path().join("alias");
        symlink(&directory, &alias).unwrap();
        assert_eq!(
            super::validate_directory(&alias).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
    assert!(
        AuditService::open(
            "relative".into(),
            Limits::default(),
            CancellationToken::new()
        )
        .is_err()
    );
}
