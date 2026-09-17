#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ptyhost_record::format::FrameIter;
use ptyhost_record::reader::{Event, list_segments, replay};
use ptyhost_record::{Frame, SEGMENT_HEADER_LEN};
use serde_json::{Value, json};

const SCRIPT: &str = "stty -echo; printf 'READY_MARKER\\n'; while IFS= read -r line; do case \"$line\" in EXIT) printf 'FINAL_MARKER\\n'; exit 7 ;; *) printf '<%s>\\n' \"$line\" ;; esac; done";
const META: &str = r#"{"source":"codex","sid":"synthetic-sid","instance_id":"synthetic-instance"}"#;
const POLL: Duration = Duration::from_millis(20);
const DEADLINE: Duration = Duration::from_secs(10);

// Fixed free shell fixture, private explicit host directory, no CLI home access.
struct Host {
    child: Child,
    directory: PathBuf,
}

impl Host {
    fn start(extra: &[&str]) -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ptyhost-record-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_ptyhost"))
            .arg("--dir")
            .arg(&directory)
            .arg("run")
            .args(extra)
            .args([
                "--name", "fixture", "--cols", "80", "--rows", "24", "--meta", META, "--",
                "/bin/sh", "-c", SCRIPT,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut host = Self { child, directory };
        let deadline = Instant::now() + DEADLINE;
        while !host.directory.join("fixture.json").exists() {
            assert!(
                host.child.try_wait().unwrap().is_none(),
                "host exited during setup"
            );
            assert!(
                Instant::now() < deadline,
                "host did not create its private record"
            );
            thread::sleep(POLL);
        }
        host.wait_capture("READY_MARKER");
        host
    }

    fn connect(&self) -> UnixStream {
        let stream = UnixStream::connect(self.directory.join("fixture.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
    }

    fn request(&self, value: Value) -> Value {
        let mut stream = self.connect();
        writeln!(stream, "{value}").unwrap();
        read_json(&mut BufReader::new(stream))
    }

    fn wait_capture(&self, needle: &str) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let capture = self.request(json!({"op":"capture", "styled":false}));
            let text = capture["text"].as_str().unwrap().replace('\n', "");
            if text.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "capture did not contain {needle:?}"
            );
            thread::sleep(POLL);
        }
    }

    fn wait_exit(&mut self) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            assert!(Instant::now() < deadline, "host did not exit");
            thread::sleep(POLL);
        }
    }

    fn send_exit(&mut self) {
        assert_eq!(
            self.request(json!({"op":"send","text":"EXIT\r"}))["ok"],
            true
        );
        self.wait_exit();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            if let Ok(mut stream) = UnixStream::connect(self.directory.join("fixture.sock")) {
                let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                let _ = stream.write_all(b"{\"op\":\"kill\",\"force\":true}\n");
            }
            let deadline = Instant::now() + Duration::from_secs(6);
            while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if matches!(self.child.try_wait(), Ok(None)) {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn read_json(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn record_dirs(host: &Host) -> Vec<PathBuf> {
    let records = host.directory.join("records");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&records)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn assert_contains_in_order(haystack: &[u8], needles: &[&[u8]]) {
    let mut pos = 0usize;
    for needle in needles {
        let found = find_subslice(&haystack[pos..], needle).unwrap_or_else(|| {
            panic!(
                "missing {:?} after offset {pos}",
                String::from_utf8_lossy(needle)
            )
        });
        pos += found + needle.len();
    }
}

#[test]
fn records_output_resize_and_exit_in_order() {
    let mut host = Host::start(&[]);
    let host_pid = u64::from(host.child.id());

    assert_eq!(
        host.request(json!({"op":"send","text":"hello\r"}))["ok"],
        true
    );
    host.wait_capture("<hello>");
    assert_eq!(
        host.request(json!({"op":"resize","cols":100,"rows":30}))["ok"],
        true
    );
    assert_eq!(
        host.request(json!({"op":"send","text":"world\r"}))["ok"],
        true
    );
    host.wait_capture("<world>");
    host.send_exit();

    let dirs = record_dirs(&host);
    assert_eq!(dirs.len(), 1, "expected exactly one record directory");
    let record_dir = &dirs[0];

    let meta: Value =
        serde_json::from_str(&std::fs::read_to_string(record_dir.join("meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["name"], "fixture");
    assert_eq!(meta["cols"], 80);
    assert_eq!(meta["rows"], 24);
    assert_eq!(meta["host_pid"], host_pid);
    assert_eq!(meta["exit"]["code"], 7);
    assert!(meta["ended_ms"].as_u64().unwrap() > 0);

    let (checkpoint, read) = replay(record_dir, 64 << 20).unwrap().expect("checkpoint");
    assert_eq!(checkpoint.cols, 80);
    assert_eq!(checkpoint.rows, 24);

    let mut output = Vec::new();
    let mut hello_at = None;
    let mut world_at = None;
    let mut resize_at = None;
    for (index, stamped) in read.events.iter().enumerate() {
        match &stamped.event {
            Event::Output(bytes) => {
                if hello_at.is_none() && find_subslice(bytes, b"<hello>").is_some() {
                    hello_at = Some(index);
                }
                if world_at.is_none() && find_subslice(bytes, b"<world>").is_some() {
                    world_at = Some(index);
                }
                output.extend_from_slice(bytes);
            }
            Event::Resize {
                cols: 100,
                rows: 30,
            } => {
                resize_at = Some(index);
            }
            _ => {}
        }
    }
    assert_contains_in_order(
        &output,
        &[b"READY_MARKER", b"<hello>", b"<world>", b"FINAL_MARKER"],
    );
    let hello_at = hello_at.expect("output containing <hello>");
    let resize_at = resize_at.expect("resize 100x30");
    let world_at = world_at.expect("output containing <world>");
    assert!(
        hello_at < resize_at && resize_at < world_at,
        "resize at {resize_at} not between hello {hello_at} and world {world_at}"
    );

    match read.events.last().map(|stamped| &stamped.event) {
        Some(Event::Exit(payload)) => {
            let exit: Value = serde_json::from_str(payload).unwrap();
            assert_eq!(exit["code"], 7);
        }
        other => panic!("last event was {other:?}, expected Exit"),
    }
    assert!(read.at_end);
    assert!(!read.gap);
}

#[test]
fn info_reports_the_record_directory() {
    let host = Host::start(&[]);
    let reply = host.request(json!({"op":"info"}));
    let record = &reply["info"]["record"];
    let dir = record["dir"].as_str().expect("record.dir string");
    assert!(
        Path::new(dir).is_dir(),
        "record.dir is not a directory: {dir}"
    );
    assert!(record["segments"].as_u64().unwrap() >= 1);
    assert_eq!(record["active"], true);
}

#[test]
fn no_record_flag_writes_nothing() {
    let mut host = Host::start(&["--no-record"]);
    assert!(
        !host.directory.join("records").exists(),
        "records directory must not exist with --no-record"
    );
    let reply = host.request(json!({"op":"info"}));
    assert!(
        reply["info"].get("record").is_none(),
        "info must not contain a record key: {reply}"
    );
    host.send_exit();
}

#[test]
fn small_segments_rotate_with_a_leading_checkpoint() {
    let mut host = Host::start(&[
        "--record-segment-bytes",
        "4096",
        "--record-total-bytes",
        "12288",
    ]);
    let line = format!("{}\r", "x".repeat(100));
    let echo = format!("<{}>", "x".repeat(100));
    for _ in 0..40 {
        assert_eq!(host.request(json!({"op":"send","text":line}))["ok"], true);
    }
    let deadline = Instant::now() + DEADLINE;
    loop {
        let capture = host.request(json!({
            "op": "capture",
            "styled": false,
            "kind": "scrollback",
            "lines": 10_000
        }));
        let text = capture["text"].as_str().unwrap().replace('\n', "");
        if text.matches(echo.as_str()).count() >= 40 {
            break;
        }
        assert!(Instant::now() < deadline, "did not capture 40 echoed lines");
        thread::sleep(POLL);
    }

    let dirs = record_dirs(&host);
    assert_eq!(dirs.len(), 1);
    let segments = list_segments(&dirs[0]).unwrap();
    assert!(
        segments.len() >= 2 && segments.len() <= 3,
        "expected 2..=3 segments, got {}",
        segments.len()
    );
    for segment in &segments {
        let bytes = std::fs::read(&segment.path).unwrap();
        assert!(bytes.len() >= SEGMENT_HEADER_LEN);
        let first = FrameIter::new(&bytes[SEGMENT_HEADER_LEN..])
            .next()
            .unwrap_or_else(|| panic!("segment {} has no first frame", segment.index));
        assert!(
            matches!(first.frame, Frame::Checkpoint { .. }),
            "segment {} first frame was {:?}",
            segment.index,
            first.frame
        );
    }
    host.send_exit();
}
