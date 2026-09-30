//! Typed native identity traversal, shared by rollout and projection adapters.
//! Never recurse through arbitrary JSON: prompts, schemas, tool text and external
//! resource IDs are not native identities merely because they look like UUIDs.
use super::TransferError;
use serde_json::Value;

#[derive(Clone, Copy, Debug)]
pub(super) enum Identity {
    Thread,
    Turn,
    Record,
}
type Mapper<'a> = dyn FnMut(Identity, &str) -> Result<String, TransferError> + 'a;

fn field(
    v: &mut Value,
    key: &str,
    kind: Identity,
    map: &mut Mapper<'_>,
) -> Result<(), TransferError> {
    if let Some(id) = v.get_mut(key) {
        scalar(id, kind, map)?;
    }
    Ok(())
}
fn scalar(v: &mut Value, kind: Identity, map: &mut Mapper<'_>) -> Result<(), TransferError> {
    if let Some(id) = v.as_str().filter(|s| !s.is_empty()) {
        *v = Value::String(map(kind, id)?);
    }
    Ok(())
}
fn threads(v: &mut Value, map: &mut Mapper<'_>) -> Result<(), TransferError> {
    for key in [
        "thread_id",
        "sender_thread_id",
        "receiver_thread_id",
        "new_thread_id",
        "agent_thread_id",
        "senderThreadId",
        "receiverThreadId",
        "agentThreadId",
    ] {
        field(v, key, Identity::Thread, map)?;
    }
    for key in ["receiver_thread_ids", "receiverThreadIds"] {
        if let Some(ids) = v.get_mut(key).and_then(Value::as_array_mut) {
            for id in ids {
                scalar(id, Identity::Thread, map)?;
            }
        }
    }
    for key in [
        "receiver_agents",
        "agent_statuses",
        "receiverAgents",
        "agentStatuses",
    ] {
        if let Some(agents) = v.get_mut(key).and_then(Value::as_array_mut) {
            for agent in agents {
                field(agent, "thread_id", Identity::Thread, map)?;
                field(agent, "threadId", Identity::Thread, map)?;
            }
        }
    }
    for key in ["statuses", "agents_states", "agentsStates"] {
        if let Some(states) = v.get_mut(key).and_then(Value::as_object_mut) {
            let mut next = serde_json::Map::new();
            for (id, value) in std::mem::take(states) {
                next.insert(map(Identity::Thread, &id)?, value);
            }
            *states = next;
        }
    }
    Ok(())
}

/// Both native TurnItem (PascalCase) and app-server projection (camelCase).
pub(super) fn item(v: &mut Value, map: &mut Mapper<'_>) -> Result<(), TransferError> {
    for key in ["id", "call_id", "callId", "event_id", "eventId"] {
        field(v, key, Identity::Record, map)?;
    }
    let kind = v["type"].as_str().unwrap_or("");
    if matches!(
        kind,
        "CollabAgentToolCall" | "collabAgentToolCall" | "SubAgentActivity" | "subAgentActivity"
    ) {
        threads(v, map)?;
    }
    Ok(())
}

pub(super) fn realtime(v: &mut Value, map: &mut Mapper<'_>) -> Result<(), TransferError> {
    for key in ["id", "realtime_session_id"] {
        field(v, key, Identity::Record, map)?;
    }
    if v["type"] == "bem_item_promoted" {
        field(v, "turn_id", Identity::Turn, map)?;
        field(v, "item_id", Identity::Record, map)?;
    }
    Ok(())
}

pub(super) fn visit(row: &mut Value, map: &mut Mapper<'_>) -> Result<(), TransferError> {
    let kind = row["type"].as_str().unwrap_or("").to_owned();
    if kind == "realtime_item" {
        return realtime(&mut row["payload"], map);
    }
    if !matches!(
        kind.as_str(),
        "event_msg" | "response_item" | "turn_context"
    ) {
        return Ok(());
    }
    let p = &mut row["payload"];
    for key in ["turn_id", "root_turn_id"] {
        field(p, key, Identity::Turn, map)?;
    }
    if let Some(metadata) = p.get_mut("internal_chat_message_metadata_passthrough") {
        field(metadata, "turn_id", Identity::Turn, map)?;
    }
    for key in ["call_id", "item_id", "event_id"] {
        field(p, key, Identity::Record, map)?;
    }
    if kind == "response_item" {
        field(p, "id", Identity::Record, map)?;
    }
    if kind == "event_msg" {
        // Thread ownership in item_started/completed/settings and collab events.
        let event = p["type"].as_str().unwrap_or("").to_owned();
        if event.starts_with("collab_") || event == "sub_agent_activity" {
            threads(p, map)?;
        } else {
            field(p, "thread_id", Identity::Thread, map)?;
        }
        if event == "thread_goal_updated" {
            field(p, "threadId", Identity::Thread, map)?;
            field(p, "turnId", Identity::Turn, map)?;
            if let Some(goal) = p.get_mut("goal") {
                field(goal, "threadId", Identity::Thread, map)?;
            }
        }
        if matches!(event.as_str(), "item_started" | "item_completed") {
            if let Some(value) = p.get_mut("item") {
                item(value, map)?;
            }
        }
    }
    Ok(())
}
