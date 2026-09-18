#![cfg(unix)]

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const SCRIPT: &str = "stty -echo; printf 'READY_MARKER\\n'; while IFS= read -r line; do case \"$line\" in EXIT) printf 'FINAL_MARKER\\n'; exit 7 ;; TITLE) printf '\\033]2;hello\\007' ;; ALT) printf '\\033[?1049h\\033[HALTSCREEN' ;; MAIN) printf '\\033[?1049l' ;; *) printf '<%s>\\n' \"$line\" ;; esac; done";
const POLL: Duration = Duration::from_millis(20);
const DEADLINE: Duration = Duration::from_secs(10);
const READ_SLICE: Duration = Duration::from_millis(200);
const FRAME_DATA: u8 = 1;
const FRAME_EXIT: u8 = 3;

// Fixed free shell fixture, private explicit host directory, no CLI home access.
struct Host {
    child: Child,
    directory: PathBuf,
}

impl Host {
    fn start() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ptyhost-grid-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_ptyhost"))
            .arg("--dir")
            .arg(&directory)
            .args([
                "run",
                "--name",
                "fixture",
                "--cols",
                "80",
                "--rows",
                "24",
                "--history",
                "100",
                "--",
                "/bin/sh",
                "-c",
                SCRIPT,
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
        let mut buffer = Vec::new();
        read_json_line(&mut stream, &mut buffer, Instant::now() + DEADLINE)
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

    fn send_text(&self, text: &str) {
        assert_eq!(self.request(json!({"op":"send","text":text}))["ok"], true);
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

struct Attach {
    stream: UnixStream,
    buffer: Vec<u8>,
    line_buf: Vec<u8>,
    lines: VecDeque<String>,
    data: VecDeque<Vec<u8>>,
    exit: Option<Value>,
}

impl Attach {
    fn open(host: &Host, request: Value) -> Self {
        let mut stream = host.connect();
        stream.set_read_timeout(Some(READ_SLICE)).unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut buffer = Vec::new();
        let ack = read_json_line(&mut stream, &mut buffer, Instant::now() + DEADLINE);
        assert_eq!(ack["ok"], true, "attach reply: {ack}");
        Self {
            stream,
            buffer,
            line_buf: Vec::new(),
            lines: VecDeque::new(),
            data: VecDeque::new(),
            exit: None,
        }
    }

    fn grid(host: &Host) -> Self {
        Self::open(
            host,
            json!({"op":"attach","cols":80,"rows":24,"replay":false,"mode":"grid"}),
        )
    }

    fn bytes(host: &Host) -> Self {
        Self::open(
            host,
            json!({"op":"attach","cols":80,"rows":24,"replay":false}),
        )
    }

    fn next_message(&mut self) -> Value {
        self.next_message_until(Instant::now() + DEADLINE)
    }

    fn next_message_until(&mut self, deadline: Instant) -> Value {
        loop {
            if let Some(line) = self.lines.pop_front() {
                return serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("grid line is not JSON ({e}): {line}"));
            }
            self.pump(deadline, "grid message");
        }
    }

    fn next_exit(&mut self) -> Value {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(exit) = self.exit.take() {
                return exit;
            }
            self.pump(deadline, "exit frame");
        }
    }

    fn next_data_until(&mut self, deadline: Instant) -> Vec<u8> {
        loop {
            if let Some(bytes) = self.data.pop_front() {
                return bytes;
            }
            self.pump(deadline, "byte data");
        }
    }

    fn pump(&mut self, deadline: Instant, what: &str) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what} after {} ms",
            DEADLINE.as_millis()
        );
        let mut chunk = [0u8; 65536];
        match self.stream.read(&mut chunk) {
            Ok(0) => panic!("connection closed while waiting for {what}"),
            Ok(n) => {
                self.buffer.extend_from_slice(&chunk[..n]);
                self.parse_frames();
            }
            Err(error) if is_timeout(&error) => {}
            Err(error) => panic!("read failed while waiting for {what}: {error}"),
        }
    }

    fn parse_frames(&mut self) {
        loop {
            if self.buffer.len() < 5 {
                return;
            }
            let kind = self.buffer[0];
            let len = u32::from_be_bytes(self.buffer[1..5].try_into().unwrap()) as usize;
            assert!(len < 8 << 20, "frame too large: {len}");
            if self.buffer.len() < 5 + len {
                return;
            }
            let payload = self.buffer[5..5 + len].to_vec();
            self.buffer.drain(..5 + len);
            match kind {
                FRAME_DATA => {
                    self.data.push_back(payload.clone());
                    self.line_buf.extend_from_slice(&payload);
                    while let Some(at) = self.line_buf.iter().position(|&b| b == b'\n') {
                        let mut line: Vec<u8> = self.line_buf.drain(..=at).collect();
                        line.pop();
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        self.lines
                            .push_back(String::from_utf8(line).expect("grid payload is utf-8"));
                    }
                }
                FRAME_EXIT => {
                    self.exit = Some(serde_json::from_slice(&payload).unwrap());
                }
                other => panic!("unexpected frame kind {other}"),
            }
        }
    }
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
    )
}

fn read_more(stream: &mut UnixStream, buffer: &mut Vec<u8>, deadline: Instant) {
    let mut chunk = [0u8; 8192];
    loop {
        assert!(Instant::now() < deadline, "timed out reading JSON line");
        match stream.read(&mut chunk) {
            Ok(0) => panic!("connection closed while reading JSON line"),
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                return;
            }
            Err(error) if is_timeout(&error) => {}
            Err(error) => panic!("read failed while reading JSON line: {error}"),
        }
    }
}

fn read_json_line(stream: &mut UnixStream, buffer: &mut Vec<u8>, deadline: Instant) -> Value {
    loop {
        if let Some(at) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=at).collect();
            return serde_json::from_slice(&line[..line.len() - 1]).unwrap();
        }
        read_more(stream, buffer, deadline);
    }
}

fn first_span_text(row: &Value) -> &str {
    row["s"][0][0].as_str().unwrap_or("")
}

fn row_text(row: &Value) -> String {
    row["s"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|span| span[0].as_str())
        .collect()
}

fn diff_rows_contain(msg: &Value, needle: &str) -> bool {
    msg["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|entry| row_text(&entry[1]).contains(needle))
}

fn wait_message(attach: &mut Attach, deadline: Instant, pred: impl Fn(&Value) -> bool) -> Value {
    loop {
        let msg = attach.next_message_until(deadline);
        if pred(&msg) {
            return msg;
        }
    }
}

#[test]
fn grid_attach_gets_a_snapshot_then_row_diffs() {
    let host = Host::start();
    let mut grid = Attach::grid(&host);
    let snap = grid.next_message();
    assert_eq!(snap["t"], "snapshot");
    assert_eq!(snap["reset"], true);
    assert_eq!(snap["cols"], 80);
    assert_eq!(snap["rows"], 24);
    assert_eq!(snap["grid"].as_array().unwrap().len(), 24);
    assert_eq!(snap["grid"][0]["s"][0][0], "READY_MARKER");
    assert_eq!(snap["modes"]["alt"], false);
    assert_eq!(snap["history_total"], 0);
    assert_eq!(snap["seq"], 1);

    host.send_text("hello\r");
    let diff = grid.next_message();
    assert_eq!(diff["t"], "diff");
    assert_eq!(diff["seq"], 2);
    let rows = diff["rows"].as_array().expect("diff.rows");
    let entry = rows
        .iter()
        .find(|entry| entry[0] == 1)
        .expect("diff row y == 1");
    assert_eq!(first_span_text(&entry[1]), "<hello>");
    assert_eq!(diff["cursor"]["y"], 2);
    assert!(
        diff.get("scrolled").is_none(),
        "unexpected scrolled: {diff}"
    );
}

#[test]
fn scrolled_rows_and_history_arrive_in_order() {
    let host = Host::start();
    let mut grid = Attach::grid(&host);
    assert_eq!(grid.next_message()["t"], "snapshot");

    for n in 1..=30 {
        host.send_text(&format!("l{n}\r"));
    }

    let mut scrolled = Vec::new();
    let deadline = Instant::now() + DEADLINE;
    while scrolled.len() < 7 {
        let msg = grid.next_message_until(deadline);
        if msg["t"] != "diff" {
            continue;
        }
        if let Some(rows) = msg["scrolled"].as_array() {
            scrolled.extend(rows.iter().cloned());
        }
    }

    assert!(
        first_span_text(&scrolled[0]) == "READY_MARKER",
        "first scrolled row: {}",
        scrolled[0]
    );
    for (index, row) in scrolled.iter().enumerate().skip(1) {
        let expected = format!("<l{index}>");
        assert_eq!(first_span_text(row), expected, "scrolled[{index}] = {row}");
    }

    let mut second = Attach::grid(&host);
    let snap = second.next_message();
    assert_eq!(snap["t"], "snapshot");
    assert_eq!(snap["reset"], true);
    let history = snap["history"].as_array().expect("history");
    let history_total = snap["history_total"].as_u64().unwrap();
    assert_eq!(history.len() as u64, history_total);
    assert!(
        history_total >= 7,
        "history_total {history_total} below 7: {snap}"
    );
    assert_eq!(history[0]["s"][0][0], "READY_MARKER");
}

#[test]
fn resize_sends_a_viewport_snapshot_without_history() {
    let host = Host::start();
    let mut grid = Attach::grid(&host);
    assert_eq!(grid.next_message()["t"], "snapshot");
    assert_eq!(
        host.request(json!({"op":"resize","cols":100,"rows":30}))["ok"],
        true
    );
    let snap = wait_message(&mut grid, Instant::now() + DEADLINE, |msg| {
        msg["t"] == "snapshot"
    });
    assert_eq!(snap["reset"], false);
    assert_eq!(snap["cols"], 100);
    assert_eq!(snap["rows"], 30);
    assert_eq!(snap["history"].as_array().unwrap().len(), 0);
}

#[test]
fn alt_screen_title_and_exit() {
    let host = Host::start();
    let mut grid = Attach::grid(&host);
    assert_eq!(grid.next_message()["t"], "snapshot");

    host.send_text("ALT\r");
    let alt_deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_alt = false;
    let mut saw_text = false;
    while !saw_alt || !saw_text {
        let msg = grid.next_message_until(alt_deadline);
        if msg["t"] != "diff" {
            continue;
        }
        if msg["modes"]["alt"] == true {
            saw_alt = true;
        }
        if diff_rows_contain(&msg, "ALTSCREEN") {
            saw_text = true;
        }
    }

    host.send_text("MAIN\r");
    wait_message(&mut grid, Instant::now() + DEADLINE, |msg| {
        msg["t"] == "diff" && msg["modes"]["alt"] == false
    });

    host.send_text("TITLE\r");
    wait_message(&mut grid, Instant::now() + DEADLINE, |msg| {
        msg["t"] == "diff" && msg["title"] == "hello"
    });

    host.send_text("EXIT\r");
    let exit = grid.next_exit();
    assert_eq!(exit["code"], 7, "exit frame: {exit}");
}

#[test]
fn byte_and_grid_clients_coexist() {
    let host = Host::start();
    let mut bytes = Attach::bytes(&host);
    let mut grid = Attach::grid(&host);
    let snap = grid.next_message();
    assert_eq!(snap["t"], "snapshot");
    serde_json::from_str::<Value>(&snap.to_string()).unwrap();

    host.send_text("both\r");

    let deadline = Instant::now() + DEADLINE;
    let mut raw = Vec::new();
    while !raw.windows(6).any(|window| window == b"<both>") {
        let frame = bytes.next_data_until(deadline);
        assert!(
            !frame.starts_with(br#"{"t":"#),
            "byte client received grid JSON: {:?}",
            String::from_utf8_lossy(&frame)
        );
        raw.extend_from_slice(&frame);
    }
    assert!(
        !raw.starts_with(br#"{"t":"#),
        "byte client stream started with JSON: {:?}",
        String::from_utf8_lossy(&raw)
    );

    let msg = wait_message(&mut grid, Instant::now() + DEADLINE, |msg| {
        diff_rows_contain(msg, "<both>")
    });
    assert_eq!(msg["t"], "diff");
}
