//! Factory Droid native transcript row summary.
use serde_json::Value;

use super::{
    AgentMeta, CLAUDE_HEAD_LINES, Input, Records, RowSummary, clip, committed_end, cursor_head,
    iso_seconds, native_identity,
};

pub(super) fn summarize(input: &Input<'_>) -> RowSummary {
    let stem = input
        .path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(data) = &input.data else {
        let mut row = RowSummary::blank(
            stem.clone(),
            clip(
                &format!("(无标题) {}", stem.chars().take(8).collect::<String>()),
                110,
            ),
            "(未知)".to_owned(),
            iso_seconds(0),
        );
        row.unsupported = Some("会话文件暂时不可读取".to_owned());
        return row;
    };
    let records = Records::parse(data, CLAUDE_HEAD_LINES);
    let hard_error = None;
    let all = records
        .all()
        .map(|r| (r.value.clone(), 0))
        .collect::<Vec<_>>();
    let meta = crate::sessions::providers::droid::metadata(
        input.path,
        &all,
        &iso_seconds(data.stamp.mtime_ns),
    );
    let sid = meta["sid"].as_str().unwrap_or(&stem).to_owned();
    let cwd = meta["cwd"].as_str().unwrap_or("(未知)").to_owned();
    let title = meta["title"]
        .as_str()
        .unwrap_or("新建 Droid 会话")
        .to_owned();
    let model = meta["model"].clone();
    let created = meta["created"].as_str().unwrap_or("").to_owned();
    let updated = meta["updated"].as_str().unwrap_or("").to_owned();
    let (native_id, declared_ids) = native_identity(
        records
            .all()
            .filter(|r| r.value["type"] == "session_start")
            .filter_map(|r| r.value.get("id").or_else(|| r.value.get("sessionId"))),
    );
    let warnings = Vec::new();
    let committed = committed_end(data);
    let head = records
        .all()
        .find(|record| record.value["type"] == "session_start")
        .map(|record| &record.value);
    // Native calling metadata identifies a delegated child. `parent` instead
    // identifies a fork, which remains an independently listed conversation.
    let agent = head
        .and_then(|head| head["callingSessionId"].as_str())
        .filter(|parent| !parent.is_empty())
        .map(|parent| AgentMeta {
            id: sid.clone(),
            parent_sid: Some(parent.to_owned()),
            title: title.clone(),
            kind: "subagent".to_owned(),
            // A child relationship is not evidence that its process is active.
            open_turn: false,
            turn_at: None,
        });
    let forked_from_id = head
        .and_then(|head| head["parent"].as_str())
        .filter(|parent| !parent.is_empty())
        .map(str::to_owned);
    RowSummary {
        forked_from_id,
        sid,
        title: clip(&title, 110),
        cwd,
        created,
        updated,
        size: data.stamp.size,
        model,
        branch: Value::Null,
        codex: None,
        agent,
        grok: None,
        continued_in_sid: None,
        native_id,
        declared_ids,
        unsupported: hard_error,
        warnings,
        committed,
        cursor_head: committed.and_then(|end| cursor_head(data, end)),
        background: 0,
        turn: None,
    }
}
