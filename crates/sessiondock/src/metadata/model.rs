//! Pure, versioned metadata transformations. No process or native-file access.

use std::collections::{BTreeMap, BTreeSet};

use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, Visitor},
};
use serde_json::{Value, json};

use super::{MAX_RECORDS, MetadataError};

pub const SCHEMA_VERSION: u32 = 1;
const UID_LIMIT: usize = 256;
const FIELD_LIMIT: usize = 2048;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Document {
    pub schema_version: u32,
    pub revision: u64,
    #[serde(deserialize_with = "unique_rows")]
    pub sessions: BTreeMap<String, Row>,
}

fn unique_rows<'de, D: de::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Row>, D::Error> {
    struct Rows;
    impl<'de> Visitor<'de> for Rows {
        type Value = BTreeMap<String, Row>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("unique session UID map")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut rows = BTreeMap::new();
            while let Some((uid, row)) = map.next_entry::<String, Row>()? {
                if rows.insert(uid, row).is_some() {
                    return Err(de::Error::custom("duplicate metadata UID"));
                }
                if rows.len() > MAX_RECORDS {
                    return Err(de::Error::custom("metadata record limit"));
                }
            }
            Ok(rows)
        }
    }
    deserializer.deserialize_map(Rows)
}

fn no(value: &bool) -> bool {
    !value
}
fn zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Row {
    #[serde(skip_serializing_if = "no")]
    starred: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    starred_at: Option<f64>,
    #[serde(skip_serializing_if = "no")]
    fork_parent_visible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped: Option<ActivityStop>,
    #[serde(skip_serializing_if = "zero")]
    activity_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    rewind_pending: Option<PendingRewind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeline: Option<TimelinePin>,
    #[serde(skip_serializing_if = "zero")]
    timeline_revision: u64,
    /// Files published by the write service and recorded for this session.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<Attachment>,
    /// The session that started this one, written once from the process tree
    /// while both were alive (Python `spawned_by`); never rewritten.
    #[serde(skip_serializing_if = "Option::is_none")]
    spawned_by: Option<SpawnedBy>,
}

/// Python `session_meta.record_spawn_parents` payload: the spawner's source and
/// native session id. The spawner row may be gone; this is not a UID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnedBy {
    pub source: String,
    pub sid: String,
}

pub const MAX_ATTACHMENTS: usize = 256;

/// Final path of an uploaded file, recorded once per path per session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub path: String,
    pub name: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub at: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopState {
    Idle,
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityStop {
    pub at: f64,
    pub reason: String,
    pub state: StopState,
    pub inferred: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingRewind {
    pub from_tip: String,
    pub stale_end: u64,
    pub started_at: f64,
}

/// A persisted display pin: the read model shows the Claude tree as if `tip`
/// were the current leaf. It never writes native files or signals the CLI;
/// native records appended after `stale_end` retire it in the read model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelinePin {
    pub tip: String,
    pub stale_end: u64,
    /// The node the operator rewound to before (its parent is `tip`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct MetadataSnapshot {
    pub(super) document: Document,
}

pub(super) fn validate_uid(uid: &str) -> Result<(), MetadataError> {
    if uid.is_empty()
        || uid.len() > UID_LIMIT
        || !uid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b":._-".contains(&byte))
    {
        return Err(MetadataError::new(
            400,
            "invalid_metadata_uid",
            "元数据 UID 必须为 1 至 256 字节的字母、数字或 :._-",
        ));
    }
    Ok(())
}

fn field(value: &str, limit: usize) -> Result<(), MetadataError> {
    if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err(MetadataError::new(
            400,
            "invalid_metadata_field",
            "元数据字段为空、过长或包含控制字符",
        ));
    }
    Ok(())
}

fn epoch(value: f64) -> Result<(), MetadataError> {
    if !value.is_finite() || !(0.0..=253_402_300_799.0).contains(&value) {
        return Err(MetadataError::new(
            400,
            "invalid_metadata_time",
            "元数据时间戳无效",
        ));
    }
    Ok(())
}

fn increment(value: u64) -> Result<u64, MetadataError> {
    value.checked_add(1).ok_or_else(|| {
        MetadataError::new(409, "metadata_revision_exhausted", "元数据版本号已达到上限")
    })
}

fn validate_attachment(attachment: &Attachment) -> Result<(), MetadataError> {
    field(&attachment.path, 4096)?;
    field(&attachment.name, 255)?;
    epoch(attachment.at)?;
    if let Some(agent) = &attachment.agent {
        field(agent, UID_LIMIT)?
    }
    if attachment
        .sha256
        .as_deref()
        .is_some_and(|digest| digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(MetadataError::new(
            400,
            "invalid_metadata_field",
            "附件 sha256 须为 64 位十六进制",
        ));
    }
    Ok(())
}

fn validate_spawned_by(parent: &SpawnedBy) -> Result<(), MetadataError> {
    field(&parent.source, 32)?;
    field(&parent.sid, UID_LIMIT)?;
    if parent.source.chars().any(char::is_whitespace) || parent.sid.chars().any(char::is_whitespace)
    {
        return Err(MetadataError::new(
            400,
            "invalid_metadata_field",
            "发起者 source/sid 不能含空白",
        ));
    }
    Ok(())
}

impl MetadataSnapshot {
    pub fn empty() -> Self {
        Self {
            document: Document {
                schema_version: SCHEMA_VERSION,
                revision: 0,
                sessions: BTreeMap::new(),
            },
        }
    }

    pub fn revision(&self) -> u64 {
        self.document.revision
    }
    pub fn schema_version(&self) -> u32 {
        self.document.schema_version
    }
    pub fn len(&self) -> usize {
        self.document.sessions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.document.sessions.is_empty()
    }

    /// Missing/cleared fields are absent, as with Python session_meta.snapshot.
    pub fn row(&self, uid: &str) -> Value {
        self.document.sessions.get(uid).map_or_else(
            || json!({}),
            |row| serde_json::to_value(row).expect("validated metadata serializes"),
        )
    }

    pub(super) fn validate(&self) -> Result<(), MetadataError> {
        if self.document.schema_version != SCHEMA_VERSION {
            return Err(MetadataError::new(
                501,
                "metadata_schema_unsupported",
                "开发元数据 schema_version 尚不受支持；原文件未被覆盖",
            ));
        }
        if self.len() > MAX_RECORDS {
            return Err(MetadataError::new(
                413,
                "metadata_record_limit",
                "元数据超过 10000 条记录限制",
            ));
        }
        for (uid, row) in &self.document.sessions {
            validate_uid(uid)?;
            if row.starred != row.starred_at.is_some() {
                return Err(MetadataError::new(
                    400,
                    "invalid_metadata_star",
                    "收藏状态与时间戳不一致",
                ));
            }
            if let Some(at) = row.starred_at {
                epoch(at)?
            }
            if let Some(stop) = &row.stopped {
                epoch(stop.at)?;
                field(&stop.reason, FIELD_LIMIT)?
            }
            if let Some(pending) = &row.rewind_pending {
                field(&pending.from_tip, UID_LIMIT)?;
                epoch(pending.started_at)?
            }
            if let Some(timeline) = &row.timeline {
                field(&timeline.tip, UID_LIMIT)?;
                if let Some(target) = &timeline.target {
                    field(target, UID_LIMIT)?
                }
                if let Some(at) = timeline.pinned_at {
                    epoch(at)?
                }
            }
            if row.attachments.len() > MAX_ATTACHMENTS {
                return Err(MetadataError::new(
                    413,
                    "metadata_attachment_limit",
                    "单个会话最多登记 256 个附件",
                ));
            }
            for attachment in &row.attachments {
                validate_attachment(attachment)?
            }
            if let Some(parent) = &row.spawned_by {
                validate_spawned_by(parent)?
            }
        }
        Ok(())
    }

    fn change(
        &self,
        update: impl FnOnce(&mut BTreeMap<String, Row>) -> Result<(), MetadataError>,
    ) -> Result<Self, MetadataError> {
        let mut next = self.clone();
        update(&mut next.document.sessions)?;
        next.document
            .sessions
            .retain(|_, row| row != &Row::default());
        if next.document.sessions != self.document.sessions {
            next.document.revision = increment(self.revision())?
        }
        next.validate()?;
        Ok(next)
    }

    pub fn with_starred(&self, uid: &str, starred: bool, at: f64) -> Result<Self, MetadataError> {
        validate_uid(uid)?;
        epoch(at)?;
        self.change(|rows| {
            let row = rows.entry(uid.to_owned()).or_default();
            row.starred = starred;
            row.starred_at = if starred {
                Some(row.starred_at.unwrap_or(at))
            } else {
                None
            };
            Ok(())
        })
    }

    /// Idempotent per path: recording the same published path twice keeps one
    /// entry and the revision unchanged.
    pub fn with_attachment(
        &self,
        uid: &str,
        attachment: Attachment,
    ) -> Result<Self, MetadataError> {
        validate_uid(uid)?;
        validate_attachment(&attachment)?;
        self.change(|rows| {
            let row = rows.entry(uid.to_owned()).or_default();
            if row
                .attachments
                .iter()
                .any(|existing| existing.path == attachment.path)
            {
                return Ok(());
            }
            if row.attachments.len() >= MAX_ATTACHMENTS {
                return Err(MetadataError::new(
                    413,
                    "metadata_attachment_limit",
                    "单个会话最多登记 256 个附件",
                ));
            }
            row.attachments.push(attachment);
            Ok(())
        })
    }

    pub fn attachments(&self, uid: &str) -> &[Attachment] {
        self.document
            .sessions
            .get(uid)
            .map_or(&[], |row| row.attachments.as_slice())
    }

    pub fn spawned_by(&self, uid: &str) -> Option<&SpawnedBy> {
        self.document.sessions.get(uid)?.spawned_by.as_ref()
    }

    /// Python `spawned_uids`: sessions whose spawner is already recorded.
    pub fn spawned_uids(&self) -> BTreeSet<String> {
        self.document
            .sessions
            .iter()
            .filter(|(_, row)| row.spawned_by.is_some())
            .map(|(uid, _)| uid.clone())
            .collect()
    }

    /// Python `record_spawn_parents`: a session is spawned once; the first
    /// observed relation is kept for good and a later, different clue is
    /// ignored. Entries with an empty uid, source or sid are skipped like
    /// Python; a malformed one (whitespace, control characters, over budget)
    /// fails the whole transaction so nothing half-recorded is published.
    pub fn with_spawn_parents(&self, found: &[(String, SpawnedBy)]) -> Result<Self, MetadataError> {
        self.change(|rows| {
            for (uid, parent) in found {
                let uid = uid.trim();
                let parent = SpawnedBy {
                    source: parent.source.trim().to_owned(),
                    sid: parent.sid.trim().to_owned(),
                };
                if uid.is_empty() || parent.source.is_empty() || parent.sid.is_empty() {
                    continue;
                }
                validate_uid(uid)?;
                validate_spawned_by(&parent)?;
                let row = rows.entry(uid.to_owned()).or_default();
                if row.spawned_by.is_none() {
                    row.spawned_by = Some(parent);
                }
            }
            Ok(())
        })
    }

    pub fn with_fork_visibility(
        &self,
        uids: &[String],
        visible: bool,
    ) -> Result<Self, MetadataError> {
        if uids.len() > 1000 {
            return Err(MetadataError::new(
                413,
                "metadata_batch_limit",
                "单次偏好更新最多 1000 个会话",
            ));
        }
        for uid in uids {
            validate_uid(uid)?
        }
        self.change(|rows| {
            for uid in uids {
                rows.entry(uid.clone()).or_default().fork_parent_visible = visible
            }
            Ok(())
        })
    }

    pub fn with_activity_stop(&self, uid: &str, stop: ActivityStop) -> Result<Self, MetadataError> {
        validate_uid(uid)?;
        self.change(|rows| {
            let row = rows.entry(uid.to_owned()).or_default();
            row.stopped = Some(stop);
            row.activity_revision = increment(row.activity_revision)?;
            Ok(())
        })
    }

    pub fn without_inferred_activity_stop(&self, uid: &str) -> Result<Self, MetadataError> {
        validate_uid(uid)?;
        self.change(|rows| {
            if let Some(row) = rows.get_mut(uid)
                && row
                    .stopped
                    .as_ref()
                    .is_some_and(|stop| stop.inferred || stop.reason == "终端已结束或中断")
            {
                row.stopped = None;
                row.activity_revision = increment(row.activity_revision)?;
            }
            Ok(())
        })
    }

    pub fn with_pending_rewind(
        &self,
        uid: &str,
        pending: PendingRewind,
    ) -> Result<Self, MetadataError> {
        validate_uid(uid)?;
        self.change(|rows| {
            rows.entry(uid.to_owned()).or_default().rewind_pending = Some(pending);
            Ok(())
        })
    }

    /// Call only after native/terminal confirmation. Pending selection itself
    /// never changes the confirmed timeline and never authorizes native writes.
    pub fn with_confirmed_rewind(&self, uid: &str, tip: &str) -> Result<Self, MetadataError> {
        validate_uid(uid)?;
        field(tip, UID_LIMIT)?;
        self.change(|rows| {
            let row = rows
                .get_mut(uid)
                .ok_or_else(|| MetadataError::new(409, "rewind_not_pending", "没有待确认的回滚"))?;
            let pending = row
                .rewind_pending
                .take()
                .ok_or_else(|| MetadataError::new(409, "rewind_not_pending", "没有待确认的回滚"))?;
            row.timeline = Some(TimelinePin {
                tip: tip.to_owned(),
                stale_end: pending.stale_end,
                target: None,
                pinned_at: None,
            });
            row.timeline_revision = increment(row.timeline_revision)?;
            Ok(())
        })
    }

    pub fn without_pending_rewind(&self, uid: &str) -> Result<Self, MetadataError> {
        validate_uid(uid)?;
        self.change(|rows| {
            if let Some(row) = rows.get_mut(uid) {
                row.rewind_pending = None
            }
            Ok(())
        })
    }

    /// Replace the display pin. Re-pinning the identical tip/target/boundary
    /// is a no-op that keeps the original `pinned_at` and revision. This is a
    /// read-model preference only: no native write, no CLI signal.
    pub fn with_timeline_pin(&self, uid: &str, pin: TimelinePin) -> Result<Self, MetadataError> {
        validate_uid(uid)?;
        field(&pin.tip, UID_LIMIT)?;
        if let Some(target) = &pin.target {
            field(target, UID_LIMIT)?
        }
        if let Some(at) = pin.pinned_at {
            epoch(at)?
        }
        self.change(|rows| {
            let row = rows.entry(uid.to_owned()).or_default();
            if row.timeline.as_ref().is_some_and(|current| {
                current.tip == pin.tip
                    && current.stale_end == pin.stale_end
                    && current.target == pin.target
            }) {
                return Ok(());
            }
            row.timeline = Some(pin);
            row.timeline_revision = increment(row.timeline_revision)?;
            Ok(())
        })
    }

    pub fn without_timeline_pin(&self, uid: &str) -> Result<Self, MetadataError> {
        validate_uid(uid)?;
        self.change(|rows| {
            if let Some(row) = rows.get_mut(uid)
                && row.timeline.take().is_some()
            {
                row.timeline_revision = increment(row.timeline_revision)?;
            }
            Ok(())
        })
    }

    pub fn timeline(&self, uid: &str) -> Option<&TimelinePin> {
        self.document.sessions.get(uid)?.timeline.as_ref()
    }
    pub fn timeline_revision(&self, uid: &str) -> u64 {
        self.document
            .sessions
            .get(uid)
            .map_or(0, |row| row.timeline_revision)
    }
    pub fn pending_rewind(&self, uid: &str) -> Option<&PendingRewind> {
        self.document.sessions.get(uid)?.rewind_pending.as_ref()
    }

    pub fn resolve_activity(&self, uid: &str, activity: &Value) -> Value {
        if !matches!(activity["state"].as_str(), Some("working" | "waiting")) {
            return activity.clone();
        }
        let Some(stop) = self
            .document
            .sessions
            .get(uid)
            .and_then(|row| row.stopped.as_ref())
        else {
            return activity.clone();
        };
        let at = activity["ts"].as_f64().or_else(|| {
            activity["ts"]
                .as_str()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.timestamp_millis() as f64 / 1000.0)
        });
        if at.is_some_and(|at| at > stop.at) {
            return activity.clone();
        }
        let state = match stop.state {
            StopState::Idle => "idle",
            StopState::Aborted => "aborted",
        };
        let timestamp = chrono::DateTime::from_timestamp_millis((stop.at * 1000.0) as i64)
            .expect("validated epoch")
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        json!({"role":"status", "state":state, "text":state, "ts":timestamp, "reason":stop.reason})
    }

    pub fn enrich_one(&self, row: &mut Value, topology: &[Value]) {
        self.decorate(row, &fork_parent_uids(topology));
    }

    pub fn enrich(&self, rows: &mut [Value]) {
        let parents = fork_parent_uids(rows);
        for row in rows {
            self.decorate(row, &parents)
        }
    }

    fn decorate(&self, row: &mut Value, parents: &BTreeSet<String>) {
        let uid = row["uid"].as_str().unwrap_or("").to_owned();
        let Some(object) = row.as_object_mut() else {
            return;
        };
        for key in [
            "starred",
            "starred_at",
            "fork_parent",
            "fork_parent_visible",
            "spawned_by",
        ] {
            object.remove(key);
        }
        let saved = self.document.sessions.get(&uid);
        if let Some(saved) = saved.filter(|row| row.starred) {
            object.insert("starred".into(), json!(true));
            object.insert("starred_at".into(), json!(saved.starred_at));
        }
        if let Some(parent) = saved.and_then(|row| row.spawned_by.as_ref()) {
            object.insert(
                "spawned_by".into(),
                json!({"source": parent.source, "sid": parent.sid}),
            );
        }
        if parents.contains(&uid) {
            object.insert("fork_parent".into(), json!(true));
            object.insert(
                "fork_parent_visible".into(),
                json!(saved.is_some_and(|row| row.fork_parent_visible)),
            );
        }
    }
}

/// Source and optional node identity both constrain SID relationships. Invalid
/// unsupported rows cannot assert a reliable fork edge that hides another row.
pub fn fork_parent_uids(rows: &[Value]) -> BTreeSet<String> {
    let edges: BTreeSet<_> = rows
        .iter()
        .filter(|row| row["supported"] != false && row["agent_id"].is_null())
        .filter_map(|row| {
            row["forked_from_id"]
                .as_str()
                .filter(|sid| !sid.is_empty())
                .map(|sid| {
                    (
                        row["source"].as_str().unwrap_or(""),
                        row["node_id"].as_str().unwrap_or(""),
                        sid,
                    )
                })
        })
        .collect();
    rows.iter()
        .filter(|row| {
            edges.contains(&(
                row["source"].as_str().unwrap_or(""),
                row["node_id"].as_str().unwrap_or(""),
                row["sid"].as_str().unwrap_or(""),
            ))
        })
        .filter_map(|row| {
            row["uid"]
                .as_str()
                .filter(|uid| !uid.is_empty())
                .map(str::to_owned)
        })
        .collect()
}
