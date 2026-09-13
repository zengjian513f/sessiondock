//! Batch 41 (bug-report): server-side structured events into the same JSONL
//! segments the browser intake writes, and the time-window query a diagnostic
//! bundle needs (Python `EventStore.record` / `EventStore.query`).
//!
//! A server event goes through the same queue as a browser batch: `try_send`,
//! never a wait for disk, counted as dropped when the queue is full. Only
//! structured metadata is stored; there is no
//! `content` blob, which is the documented difference from Python's SQLite
//! store. The query reads the segment files of the window's dates line by
//! line and returns matching rows in file order (ascending `seq`).

use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    sync::atomic::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use super::{AuditService, Batch, intake, writer};

/// Python `query(limit=100_000)` for the bug-report window.
pub const MAX_QUERY_ROWS: usize = 100_000;

/// One server-side event. Strings are clipped like Python's audit row fields;
/// `data` is sanitized like browser `data`.
pub struct ServerEvent<'a> {
    pub event: &'a str,
    pub category: &'a str,
    pub severity: &'a str,
    pub uid: &'a str,
    pub trace_id: &'a str,
    pub page_id: &'a str,
    pub build: &'a str,
    pub data: Value,
}

impl AuditService {
    /// Queue one server-side event without blocking. Returns whether the
    /// writer accepted it; a refused or dropped event is counted, never retried.
    pub fn record(&self, event: ServerEvent<'_>) -> bool {
        let Some(record) = server_record(&event, SystemTime::now()) else {
            return false;
        };
        let mut record = record;
        record["seq"] = json!(self.shared.sequence.fetch_add(1, Ordering::Relaxed));
        let mut bytes = serde_json::to_vec(&record).expect("Value serializes");
        bytes.push(b'\n');
        let length = bytes.len();
        let counters = &self.shared.counters;
        let dropped = || {
            counters.dropped_batches.fetch_add(1, Ordering::Relaxed);
            counters.dropped_events.fetch_add(1, Ordering::Relaxed);
            false
        };
        if self.shutdown.is_cancelled()
            || self.shared.stop.load(Ordering::Acquire)
            || self.shared.queued_batches.load(Ordering::Acquire)
                >= self.shared.limits.queue_batches
        {
            return dropped();
        }
        self.shared.queued_batches.fetch_add(1, Ordering::AcqRel);
        self.shared.reserve(length);
        match self.tx.try_send(Batch {
            bytes: bytes.into_boxed_slice(),
            events: 1,
        }) {
            Ok(()) => {
                counters.accepted_batches.fetch_add(1, Ordering::Relaxed);
                counters.accepted_events.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(_) => {
                self.shared.queued_batches.fetch_sub(1, Ordering::AcqRel);
                self.shared.release(length);
                dropped()
            }
        }
    }
}

fn bounded(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Same millisecond RFC 3339 text the intake stamps `received_at` with, so
/// window bounds compare as strings.
pub fn rfc3339(now: SystemTime) -> String {
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    chrono::DateTime::from_timestamp(
        (nanos / 1_000_000_000) as i64,
        (nanos % 1_000_000_000) as u32,
    )
    .unwrap_or_default()
    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The stored shape of a server event: the browser record's fields with
/// `client: "server"`, no client timestamp and the given category.
pub fn server_record(event: &ServerEvent<'_>, now: SystemTime) -> Option<Value> {
    let uid = bounded(event.uid, intake::UID_MAX);
    Some(json!({
        "seq": 0,
        "received_at": rfc3339(now),
        "event": if event.event.is_empty() { "unknown".to_owned() } else { bounded(event.event, intake::EVENT_NAME_MAX) },
        "category": bounded(event.category, 80),
        "severity": if event.severity.is_empty() { "info".to_owned() } else { bounded(event.severity, intake::SEVERITY_MAX) },
        "client_ts": Value::Null,
        "page_id": bounded(event.page_id, intake::ID_MAX),
        "uid": uid,
        "source": uid.split_once(':').map_or("", |(source, _)| source),
        "trace_id": bounded(event.trace_id, intake::ID_MAX),
        "request_id": "",
        "connection_id": "",
        "build": bounded(event.build, intake::ID_MAX),
        "client": "server",
        "data": intake::sanitize(&event.data, 0),
    }))
}

/// Python `bug_report.create`'s relevance test: any identity in common with
/// the report, or the row belongs to the report itself.
#[derive(Clone, Debug, Default)]
pub struct QueryFilter {
    pub uid: String,
    pub page_id: String,
    pub trace_id: String,
    pub report_id: String,
}

impl QueryFilter {
    pub fn matches(&self, row: &Value) -> bool {
        (!self.uid.is_empty() && row["uid"] == self.uid.as_str())
            || (!self.page_id.is_empty() && row["page_id"] == self.page_id.as_str())
            || (!self.trace_id.is_empty() && row["trace_id"] == self.trace_id.as_str())
            || (!self.report_id.is_empty() && row["data"]["report_id"] == self.report_id.as_str())
    }
}

/// Rows whose `received_at` lies in `[since, until]` and match `filter`,
/// in file order, at most `limit`. Segments outside the window's dates are
/// not opened; a malformed line is skipped. Blocking I/O: run
/// on a blocking executor.
pub fn query(
    directory: &Path,
    since: SystemTime,
    until: SystemTime,
    filter: &QueryFilter,
    limit: usize,
) -> Vec<Value> {
    let limit = limit.clamp(1, MAX_QUERY_ROWS);
    let (since_text, until_text) = (rfc3339(since), rfc3339(until));
    // The active segment is named by its UTC date; a window that started the
    // previous day also needs that day's segments.
    let first_date = intake::utc_date(
        since
            .checked_sub(std::time::Duration::from_secs(24 * 3600))
            .unwrap_or(since),
    );
    let last_date = intake::utc_date(until);
    let mut segments: Vec<_> = writer::list_segments(directory)
        .into_iter()
        .filter(|segment| segment.date >= first_date && segment.date <= last_date)
        .collect();
    segments.sort_by(|left, right| (&left.date, left.index).cmp(&(&right.date, right.index)));
    let mut rows = Vec::new();
    for segment in segments {
        let Ok(file) = File::open(&segment.path) else {
            continue;
        };
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(row) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            let Some(received) = row["received_at"].as_str() else {
                continue;
            };
            if received < since_text.as_str() || received > until_text.as_str() {
                continue;
            }
            if !filter.matches(&row) {
                continue;
            }
            rows.push(row);
            if rows.len() >= limit {
                return rows;
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn event(name: &str, uid: &str, data: Value) -> Value {
        server_record(
            &ServerEvent {
                event: name,
                category: "bug-report",
                severity: "",
                uid,
                trace_id: "",
                page_id: "page-1",
                build: "b",
                data,
            },
            SystemTime::now(),
        )
        .unwrap()
    }

    #[test]
    fn server_records_keep_the_browser_shape_without_content() {
        let row = event(
            "bug_report.created",
            "codex:one",
            json!({"report_id":"BUG-1","password":"x"}),
        );
        assert_eq!(row["event"], "bug_report.created");
        assert_eq!(row["client"], "server");
        assert_eq!(row["source"], "codex");
        assert_eq!(row["severity"], "info");
        assert_eq!(row["data"]["password"], "<redacted>");
        assert!(row.get("content").is_none());
        assert!(
            server_record(
                &ServerEvent {
                    event: "browser.fake",
                    category: "",
                    severity: "",
                    uid: "",
                    trace_id: "",
                    page_id: "",
                    build: "",
                    data: Value::Null
                },
                SystemTime::now()
            )
            .is_some()
        );
    }

    #[test]
    fn query_filters_by_identity_window_and_limit() {
        let temp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let date = intake::utc_date(now);
        // Browser rows as the intake writes them: the prefix is added by intake.
        let browser = |name: &str, uid: &str| {
            let mut row = event("x", uid, json!({}));
            row["event"] = json!(format!("browser.{name}"));
            row
        };
        let mut rows = vec![
            browser("dom.snapshot", "codex:one"),
            browser("unrelated", "claude:other"),
            event("bug_report.created", "", json!({"report_id":"BUG-1"})),
        ];
        rows[1]["page_id"] = json!("page-2");
        let mut old = browser("old", "codex:one");
        old["received_at"] = json!(rfc3339(now - Duration::from_secs(3600)));
        rows.push(old);
        let mut text = String::new();
        for row in &rows {
            text.push_str(&row.to_string());
            text.push('\n');
        }
        text.push_str("not json\n");
        std::fs::write(temp.path().join(writer::segment_name(&date, 0)), text).unwrap();
        std::fs::write(temp.path().join("notes.txt"), "ignored").unwrap();
        let filter = QueryFilter {
            uid: "codex:one".into(),
            page_id: "page-1".into(),
            trace_id: String::new(),
            report_id: "BUG-1".into(),
        };
        let found = query(
            temp.path(),
            now - Duration::from_secs(900),
            now + Duration::from_secs(5),
            &filter,
            100,
        );
        let names: Vec<&str> = found
            .iter()
            .map(|row| row["event"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["browser.dom.snapshot", "bug_report.created"]);
        assert_eq!(
            query(
                temp.path(),
                now - Duration::from_secs(900),
                now + Duration::from_secs(5),
                &filter,
                1
            )
            .len(),
            1
        );
        let none = QueryFilter::default();
        assert!(query(temp.path(), now, now, &none, 10).is_empty());
    }
}
