//! Summaries of SessionDock's own Kiro cache; native files are never parsed.
use super::{
    AgentMeta, Input, RowSummary, SidecarBytes, committed_end, cursor_head, iso_seconds,
    native_identity, norm_ts,
};
use serde_json::Value;
pub(super) fn summarize(input: &Input<'_>) -> RowSummary {
    let (summary, stamp) = match &input.sidecar {
        Some(SidecarBytes::Bytes { bytes, stamp }) => {
            (serde_json::from_slice::<Value>(bytes).ok(), Some(*stamp))
        }
        _ => (None, None),
    };
    let valid = summary
        .as_ref()
        .is_some_and(|value| value["format"] == crate::sessions::kiro::FORMAT);
    let info = summary.unwrap_or(Value::Null);
    let session = &info["session"];
    let sid = session["id"].as_str().unwrap_or_default().to_owned();
    let fallback = iso_seconds(stamp.map_or(0, |stamp| stamp.mtime_ns));
    let created = norm_ts(&session["created"]).unwrap_or_else(|| fallback.clone());
    let updated = norm_ts(&session["updated"]).unwrap_or_else(|| created.clone());
    let (native_id, declared_ids) = native_identity(session.get("id").into_iter());
    let committed = input.data.as_ref().and_then(committed_end);
    RowSummary {
        forked_from_id: info["native"]["forked_from_id"].as_str().map(str::to_owned),
        sid,
        title: session["title"]
            .as_str()
            .unwrap_or("新建 Kiro 会话")
            .to_owned(),
        cwd: session["cwd"].as_str().unwrap_or_default().to_owned(),
        created,
        updated,
        size: input.data.as_ref().map_or(0, |data| data.stamp.size)
            + stamp.map_or(0, |stamp| stamp.size),
        model: session["model"].clone(),
        branch: Value::Null,
        codex: None,
        agent: info["agent"]["id"].as_str().map(|id| AgentMeta {
            id: id.to_owned(),
            parent_sid: info["agent"]["parent_sid"].as_str().map(str::to_owned),
            title: info["agent"]["title"].as_str().unwrap_or("").to_owned(),
            kind: info["agent"]["type"]
                .as_str()
                .unwrap_or("subagent")
                .to_owned(),
            open_turn: matches!(
                info["agent"]["status"].as_str(),
                Some("pending" | "in_progress")
            ),
            turn_at: None,
        }),
        grok: None,
        continued_in_sid: None,
        native_id,
        declared_ids,
        unsupported: (!valid).then(|| "Kiro 私有投影格式不可读取".into()),
        warnings: Vec::new(),
        committed,
        cursor_head: committed
            .and_then(|end| input.data.as_ref().and_then(|data| cursor_head(data, end))),
        background: 0,
        turn: None,
    }
}
