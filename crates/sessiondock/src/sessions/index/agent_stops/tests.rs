//! Python `ClaudeAgentItemTests` mechanics at the scan level: notice
//! de-duplication by text and first-seen time, foreground results,
//! `async_launched`, incremental reads of complete lines only, rescans on
//! shrink and inode change, the record budget.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::super::Stamp;
use super::*;

fn notice_text(agent: &str, status: &str) -> String {
    format!(
        "<task-notification>\n<task-id>{agent}</task-id>\n<status>{status}</status>\n<summary>Agent finished</summary>\n</task-notification>"
    )
}

/// Python `notice(ts, agent_id, status, shape="attachment")`.
fn attachment_notice(ts: &str, agent: &str, status: &str) -> Value {
    json!({"type": "attachment", "timestamp": ts,
        "attachment": {"type": "queued_command", "commandMode": "task-notification",
                       "prompt": notice_text(agent, status), "timestamp": ts}})
}

/// Python `notice(..., shape="user")`.
fn user_notice(ts: &str, agent: &str, status: &str) -> Value {
    json!({"type": "user", "timestamp": ts,
        "message": {"role": "user", "content": notice_text(agent, status)}})
}

fn queue_operation(ts: &str, operation: &str, text: &str) -> Value {
    json!({"type": "queue-operation", "operation": operation, "timestamp": ts, "content": text})
}

/// Python `agent_result(ts, agent_id, status, tool_use_id)`.
fn agent_result(ts: &str, agent: &str, status: &str) -> Value {
    json!({"type": "user", "timestamp": ts,
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_x",
             "content": [{"type": "text", "text": if status == "async_launched" {
                 "Async agent launched successfully" } else { "报告" }}]}]},
        "toolUseResult": {"status": status, "agentId": agent}})
}

fn line(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn feed(values: &[Value]) -> StopScan {
    let mut scan = StopScan::default();
    for value in values {
        collect(&line(value), &mut scan);
    }
    scan
}

fn stops(scan: &StopScan) -> Vec<(&str, &str)> {
    scan.stops()
        .iter()
        .map(|(agent, at)| (agent.as_str(), at.as_str()))
        .collect()
}

#[test]
fn notices_and_foreground_results_are_stops_but_async_launch_is_not() {
    let scan = feed(&[
        json!({"type": "user", "timestamp": "2026-09-12T00:00:00.000Z",
            "message": {"role": "user", "content": "主任务"}}),
        attachment_notice("2026-09-12T00:12:00.100Z", "done", "completed"),
        user_notice("2026-09-12T00:22:00.100Z", "streamed", "completed"),
        attachment_notice("2026-09-12T00:31:00.100Z", "resumed", "failed"),
        attachment_notice("2026-09-12T00:52:00.000Z", "killed", "killed"),
        agent_result("2026-09-12T01:00:00.100Z", "fresh", "async_launched"),
        agent_result("2026-09-12T01:12:00.200Z", "foreground", "completed"),
        // A background Bash task-id never matches a subagent but is recorded.
        attachment_notice("2026-09-12T01:13:00.000Z", "b7f1a2", "completed"),
        // No timestamp: ignored like Python (`if not ts: return`).
        json!({"type": "user", "message": {"role": "user",
            "content": notice_text("untimed", "completed")}}),
        // Not an object / not JSON: ignored.
        json!([1, 2, 3]),
    ]);
    assert_eq!(
        stops(&scan),
        [
            ("b7f1a2", "2026-09-12T01:13:00.000Z"),
            ("done", "2026-09-12T00:12:00.100Z"),
            ("foreground", "2026-09-12T01:12:00.200Z"),
            ("killed", "2026-09-12T00:52:00.000Z"),
            ("resumed", "2026-09-12T00:31:00.100Z"),
            ("streamed", "2026-09-12T00:22:00.100Z"),
        ]
    );
    let mut scan = StopScan::default();
    collect(b"{not json but <task-id>x</task-id>", &mut scan);
    assert!(scan.stops().is_empty());
    // A result without agentId or with a status other than a real stop.
    let scan = feed(&[
        json!({"type": "user", "timestamp": "2026-09-12T01:00:00.000Z",
            "message": {"role": "user", "content": [{"type": "tool_result", "content": "x"}]},
            "toolUseResult": {"status": "completed", "agentId": ""}}),
        json!({"type": "user", "timestamp": "2026-09-12T01:00:00.000Z",
            "message": {"role": "user", "content": [{"type": "tool_result", "content": "x"}]},
            "toolUseResult": "agentId tool_result"}),
        json!({"type": "user", "timestamp": "2026-09-12T01:00:00.000Z",
            "message": {"role": "user", "content": [{"type": "tool_result", "content": "x"}]},
            "toolUseResult": {"agentId": 42}}),
    ]);
    assert_eq!(stops(&scan), [("42", "2026-09-12T01:00:00.000Z")]);
}

#[test]
fn copies_of_one_notice_count_at_their_first_seen_time_only() {
    // Python `test_late_copies_of_a_notice_and_refusal_do_not_flip_a_running_agent`.
    let text = notice_text("worker", "completed");
    let scan = feed(&[
        queue_operation("2026-09-12T00:19:00.650Z", "enqueue", &text),
        queue_operation("2026-09-12T00:21:00.400Z", "remove", &text),
        attachment_notice("2026-09-12T00:19:00.650Z", "worker", "completed"),
        queue_operation("2026-09-12T00:33:00.000Z", "enqueue", &text),
        user_notice("2026-09-12T00:35:00.000Z", "worker", "completed"),
    ]);
    assert_eq!(stops(&scan), [("worker", "2026-09-12T00:19:00.650Z")]);
    assert!(claude_active(
        true,
        "2026-09-12T00:20:59.000Z",
        scan.stops().get("worker").map(String::as_str)
    ));
    // A notice with new text (a second real stop) counts.
    let mut scan = scan;
    collect(
        &line(&attachment_notice(
            "2026-09-12T00:40:00.000Z",
            "worker",
            "failed",
        )),
        &mut scan,
    );
    assert_eq!(stops(&scan), [("worker", "2026-09-12T00:40:00.000Z")]);
    assert!(!claude_active(
        true,
        "2026-09-12T00:20:59.000Z",
        scan.stops().get("worker").map(String::as_str)
    ));
}

#[test]
fn an_earlier_copy_written_later_lowers_the_stop_to_the_notices_maximum() {
    // The attachment carries the event time but lands after the dequeue
    // record that carries a later time: the agent's stop is recomputed
    // from every notice text's first-seen time.
    let text = notice_text("worker", "completed");
    let other = notice_text("worker", "failed");
    let scan = feed(&[
        queue_operation("2026-09-12T00:30:00.000Z", "remove", &text),
        queue_operation("2026-09-12T00:20:00.000Z", "enqueue", &other),
        attachment_notice("2026-09-12T00:10:00.000Z", "worker", "completed"),
    ]);
    assert_eq!(stops(&scan), [("worker", "2026-09-12T00:20:00.000Z")]);
    // Python recomputes from notices alone: a foreground result set earlier
    // is replaced by the notices' maximum, exactly like `_collect_agent_stops`.
    let scan = feed(&[
        agent_result("2026-09-12T00:50:00.000Z", "worker", "completed"),
        queue_operation("2026-09-12T00:30:00.000Z", "remove", &text),
        attachment_notice("2026-09-12T00:10:00.000Z", "worker", "completed"),
    ]);
    assert_eq!(stops(&scan), [("worker", "2026-09-12T00:10:00.000Z")]);
}

#[test]
fn one_line_can_name_several_agents_and_only_short_task_ids_match() {
    let both = format!(
        "{}\n{}",
        notice_text("alpha", "completed"),
        notice_text("beta", "killed")
    );
    let long = "x".repeat(65);
    let scan = feed(&[
        queue_operation("2026-09-12T00:01:00.000Z", "enqueue", &both),
        queue_operation(
            "2026-09-12T00:02:00.000Z",
            "enqueue",
            &notice_text(&long, "completed"),
        ),
        queue_operation(
            "2026-09-12T00:03:00.000Z",
            "enqueue",
            "<task-notification><task-id>bad id</task-id></task-notification>",
        ),
    ]);
    assert_eq!(
        stops(&scan),
        [
            ("alpha", "2026-09-12T00:01:00.000Z"),
            ("beta", "2026-09-12T00:01:00.000Z")
        ]
    );
}

#[test]
fn ts_after_and_claude_active_follow_python() {
    assert!(ts_after(
        "2026-09-12T00:20:00.000Z",
        "2026-09-12T00:19:00.650Z"
    ));
    assert!(!ts_after(
        "2026-09-12T00:19:00.650Z",
        "2026-09-12T00:19:00.650Z"
    ));
    assert!(!ts_after("2026-09-12T00:20:00.000Z", "garbage"));
    assert!(!ts_after("", "2026-09-12T00:20:00.000Z"));
    assert!(claude_active(true, "2026-09-12T00:20:00.000Z", None));
    assert!(!claude_active(false, "2026-09-12T00:20:00.000Z", None));
    assert!(claude_active(
        true,
        "2026-09-12T00:20:00.000Z",
        Some("2026-09-12T00:19:00.000Z")
    ));
    assert!(!claude_active(
        true,
        "2026-09-12T00:20:00.000Z",
        Some("2026-09-12T00:20:00.000Z")
    ));
}

struct Owner {
    _temp: TempDir,
    root: PathBuf,
    data: PathBuf,
}

impl Owner {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("proj").join("owner.jsonl");
        fs::create_dir_all(data.parent().unwrap()).unwrap();
        Self {
            _temp: temp,
            root,
            data,
        }
    }

    fn write(&self, bytes: &[u8]) {
        fs::write(&self.data, bytes).unwrap();
    }

    fn append(&self, bytes: &[u8]) {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&self.data)
            .unwrap();
        file.write_all(bytes).unwrap();
    }

    fn stamp(&self) -> Stamp {
        let meta = cap_std::fs::Dir::open_ambient_dir(&self.root, cap_std::ambient_authority())
            .unwrap()
            .metadata(self.data.strip_prefix(&self.root).unwrap())
            .unwrap();
        Stamp::of(&meta)
    }

    fn update(&self, scan: &mut StopScan) {
        update(scan, &self.root, &self.data, self.stamp());
    }
}

fn lines(values: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend(line(value));
        bytes.push(b'\n');
    }
    bytes
}

#[test]
fn update_reads_only_new_complete_lines_and_rescans_on_shrink() {
    // Python `test_stop_notices_are_read_incrementally_and_only_from_complete_lines`.
    let owner = Owner::new();
    let head = lines(&[
        json!({"type": "user", "timestamp": "2026-09-12T00:00:00.000Z",
        "message": {"role": "user", "content": "主任务"}}),
    ]);
    owner.write(&head);
    let mut scan = StopScan::default();
    owner.update(&mut scan);
    assert!(scan.stops().is_empty());
    assert_eq!(scan.scanned(), head.len() as u64);
    assert_eq!(scan.stamp(), Some(owner.stamp()));

    let notice = line(&attachment_notice(
        "2026-09-12T01:05:00.100Z",
        "worker",
        "completed",
    ));
    let (first, second) = notice.split_at(notice.len() / 2);
    owner.append(first);
    owner.update(&mut scan);
    assert!(scan.stops().is_empty(), "a half line is not a stop");
    assert_eq!(
        scan.scanned(),
        head.len() as u64,
        "nor is it consumed: the offset stays at the last LF"
    );
    owner.append(second);
    owner.append(b"\n");
    owner.update(&mut scan);
    assert_eq!(stops(&scan), [("worker", "2026-09-12T01:05:00.100Z")]);
    assert_eq!(scan.scanned(), (head.len() + notice.len() + 1) as u64);

    // Rewritten shorter: rescanned from the start, the old notice is gone.
    owner.write(&head);
    owner.update(&mut scan);
    assert!(scan.stops().is_empty());
    let mut rewritten = head.clone();
    rewritten.extend(line(&attachment_notice(
        "2026-09-12T01:21:00.000Z",
        "worker",
        "killed",
    )));
    rewritten.push(b'\n');
    owner.write(&rewritten);
    owner.update(&mut scan);
    assert_eq!(stops(&scan), [("worker", "2026-09-12T01:21:00.000Z")]);

    // Same stamp: nothing is read (the offset and stops are as before).
    let before = scan.scanned();
    owner.update(&mut scan);
    assert_eq!(scan.scanned(), before);
}

#[test]
fn update_restarts_on_a_new_inode_and_keeps_the_scan_when_the_file_is_gone() {
    let owner = Owner::new();
    let one = lines(&[attachment_notice(
        "2026-09-12T01:00:00.000Z",
        "one",
        "completed",
    )]);
    owner.write(&one);
    let mut scan = StopScan::default();
    owner.update(&mut scan);
    assert_eq!(stops(&scan), [("one", "2026-09-12T01:00:00.000Z")]);

    // Replace the file (new inode, longer content): a fresh scan.
    let replacement = owner.root.join("proj").join("replacement.jsonl");
    let two = lines(&[
        json!({"type": "user", "timestamp": "2026-09-12T00:00:00.000Z",
            "message": {"role": "user", "content": "主任务"}}),
        attachment_notice("2026-09-12T02:00:00.000Z", "two", "completed"),
    ]);
    fs::write(&replacement, &two).unwrap();
    fs::rename(&replacement, &owner.data).unwrap();
    let stamp = owner.stamp();
    owner.update(&mut scan);
    assert_eq!(stops(&scan), [("two", "2026-09-12T02:00:00.000Z")]);
    assert_eq!(scan.stamp(), Some(stamp));

    // The index's stamp names an inode that is no longer there: unchanged.
    fs::remove_file(&owner.data).unwrap();
    update(
        &mut scan,
        &owner.root,
        &owner.data,
        Stamp {
            size: stamp.size + 100,
            ..stamp
        },
    );
    assert_eq!(stops(&scan), [("two", "2026-09-12T02:00:00.000Z")]);
    assert_eq!(scan.stamp(), Some(stamp));
    // A path outside the root never opens and leaves the scan untouched.
    let mut outside = StopScan::default();
    update(
        &mut outside,
        &owner.root,
        Path::new("/etc/passwd"),
        Stamp {
            dev: 1,
            ino: 1,
            size: 10,
            mtime_ns: 1,
        },
    );
    assert_eq!(outside.stamp(), None);
}

#[test]
fn a_line_over_the_record_budget_is_skipped_whole_and_the_scan_continues() {
    let owner = Owner::new();
    let mut bytes = lines(&[attachment_notice(
        "2026-09-12T01:00:00.000Z",
        "one",
        "completed",
    )]);
    let huge_start = bytes.len();
    bytes.extend(b"{\"type\":\"user\",\"timestamp\":\"2026-09-12T01:30:00.000Z\",\"content\":\"");
    bytes.extend(std::iter::repeat_n(b'x', budgets::RECORD_BYTES + 3 * CHUNK));
    bytes.extend(notice_text("huge", "completed").as_bytes());
    bytes.extend(b"\"}\n");
    bytes.extend(lines(&[attachment_notice(
        "2026-09-12T02:00:00.000Z",
        "two",
        "completed",
    )]));
    owner.write(&bytes);
    let mut scan = StopScan::default();
    owner.update(&mut scan);
    assert_eq!(
        stops(&scan),
        [
            ("one", "2026-09-12T01:00:00.000Z"),
            ("two", "2026-09-12T02:00:00.000Z")
        ]
    );
    assert_eq!(scan.scanned(), bytes.len() as u64);
    assert!(huge_start < bytes.len());
}
