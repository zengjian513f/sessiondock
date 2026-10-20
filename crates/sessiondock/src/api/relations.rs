//! Transport for top-level work relationships; never writes native sessions.
use super::metadata;
use crate::{
    error::ApiError,
    metadata::{Predecessor, relations},
    state::{AppState, JsonBytes},
};
use axum::{
    Extension, Json,
    extract::{Query, State, rejection::JsonRejection},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct InfoQuery {
    uid: String,
}

pub async fn info(
    State(state): State<AppState>,
    Query(query): Query<InfoQuery>,
) -> Result<JsonBytes, ApiError> {
    metadata::write(state, move |store, _| {
        relations::info(&store.list(true)?, &query.uid).ok_or_else(missing)
    })
    .await
}

pub async fn catalog(State(state): State<AppState>) -> Result<JsonBytes, ApiError> {
    metadata::write(state, move |store, _| {
        Ok(top_level_catalog(store.list(true)?))
    })
    .await
}
pub fn top_level_catalog(mut value: Value) -> Value {
    if let Some(rows) = value["sessions"].as_array_mut() {
        rows.retain(relations::top_level);
        for row in rows {
            if let Some(object) = row.as_object_mut() {
                object.remove("agent_items");
            }
        }
    }
    value
}
fn missing() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "session_missing", "会话不存在")
}

#[derive(Deserialize)]
pub struct PredecessorRequest {
    uid: String,
    #[serde(default)]
    predecessor_uid: String,
    #[serde(default)]
    remote_predecessor: Option<Predecessor>,
    #[serde(default)]
    remove: bool,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default = "default_status")]
    status: String,
    #[serde(default)]
    artifact: String,
    #[serde(default)]
    evidence: String,
    #[serde(flatten)]
    diagnostics: metadata::Diagnostics,
}
fn default_kind() -> String {
    "continues".into()
}
fn default_status() -> String {
    "confirmed".into()
}

pub async fn save(
    State(state): State<AppState>,
    hub: Option<Extension<super::node_auth::AuthenticatedHub>>,
    body: Result<Json<PredecessorRequest>, JsonRejection>,
) -> Result<JsonBytes, ApiError> {
    let store = metadata::configured(&state)?;
    let Json(body) = body.map_err(metadata::invalid)?;
    body.diagnostics.validate(&state, hub.is_some())?;
    if body.remote_predecessor.is_some() && hub.is_none() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "relations_remote_hub",
            "跨机器关系需要通过 Hub 验证",
        ));
    }
    if !["continues", "branches", "references"].contains(&body.kind.as_str())
        || !["confirmed", "candidate"].contains(&body.status.as_str())
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "relations_kind",
            "关系类型或确认状态无效",
        ));
    }
    metadata::write(state, move |sessions, events| {
        let inventory = sessions.list(true)?;
        let rows = inventory["sessions"].as_array().expect("session rows");
        let owner = rows.iter().find(|r| r["uid"] == body.uid).ok_or_else(missing)?;
        let mut edge = if let Some(edge) = body.remote_predecessor { edge }
        else if let Some(source) = rows.iter().find(|r| r["uid"] == body.predecessor_uid) {
            if !body.remove && !relations::top_level(source) { return Err(top_level_error()); }
            relations::descriptor(source, owner)
        } else if body.remove {
            Predecessor { session_uid: body.predecessor_uid, ..Default::default() }
        } else { return Err(missing()); };
        if !body.remove && !relations::top_level(owner) { return Err(top_level_error()); }
        if edge.node_id.is_none() && edge.session_uid == body.uid {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "relations_self", "不能把当前会话设为自己的前序"));
        }
        edge.kind = body.kind; edge.status = body.status; edge.artifact = body.artifact; edge.evidence = body.evidence;
        let snapshot = metadata::update(&store, events, |store| Ok(store.set_predecessor(&body.uid, edge, body.remove)?))?;
        Ok(json!({"ok": true, "uid": body.uid, "predecessors": snapshot.row(&body.uid)["predecessors"].as_array().cloned().unwrap_or_default(), "metadata_revision": snapshot.revision()}))
    }).await
}
fn top_level_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "relations_top_level",
        "前后序关系仅用于顶级会话",
    )
}

/// Discovery starts with a top-level session's opening user messages, not its children.
pub async fn hints(
    State(state): State<AppState>,
    Query(query): Query<InfoQuery>,
) -> Result<JsonBytes, ApiError> {
    metadata::write(state, move |store, _| {
        let inventory = store.list(true)?;
        let row = inventory["sessions"].as_array().and_then(|rows| rows.iter().find(|r| r["uid"] == query.uid)).ok_or_else(missing)?;
        if !relations::top_level(row) { return Err(top_level_error()); }
        let snapshot = store.snapshot(&query.uid, "")?;
        let documents = regex::Regex::new(r#"[^\s<>\"'`，。；（）()\[\]]+\.md\b"#).expect("document pattern");
        let identifiers = regex::Regex::new(r"(?i)\b[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\b").expect("UUID pattern");
        let messages: Vec<_> = snapshot.view.events().enumerate().filter(|(_, e)| e.message["role"] == "user")
            .filter(|(_, e)| !["# AGENTS.md", "<environment_context>", "<turn_aborted>", "<permissions instructions>"].iter().any(|prefix| e.message["text"].as_str().unwrap_or("").trim_start().starts_with(prefix)))
            .take(5).map(|(ordinal, event)| {
                let text = event.message["text"].as_str().unwrap_or("");
                json!({"ordinal": ordinal, "id": event.message["id"], "ts": event.message["ts"], "text": text,
                    "documents": documents.find_iter(text).map(|m| m.as_str()).collect::<Vec<_>>(),
                    "session_ids": identifiers.find_iter(text).map(|m| m.as_str()).collect::<Vec<_>>()})
            }).collect();
        Ok(json!({"uid": query.uid, "messages": messages}))
    }).await
}

/// Publish an explicitly requested continuation once its exact native identity
/// exists. The intent lives in the launch receipt, so closing the page or a
/// restart between first SEND and discovery cannot lose the edge.
pub(super) async fn record_continuations(
    state: &AppState,
    records: std::sync::Arc<Vec<crate::lifecycle::model::Record>>,
) -> Result<(), ApiError> {
    if !records
        .iter()
        .any(|record| record.spec().continuation().is_some())
    {
        return Ok(());
    }
    let Some(metadata) = state.metadata.clone() else {
        return Ok(());
    };
    let events = state.ui_events.clone();
    state
        .reader
        .run(move |sessions| {
            let inventory = sessions.list_recent()?;
            let rows = inventory["sessions"].as_array().expect("session rows");
            for record in records.iter() {
                let Some(edge) = record.spec().continuation() else {
                    continue;
                };
                let owner = if let Some(binding) = record
                    .binding()
                    .filter(|b| b.state() == crate::lifecycle::model::BindingState::Confirmed)
                {
                    rows.iter().find(|row| row["uid"] == binding.spec().uid())
                } else if let Some(sid) = record.declared_sid() {
                    let mut matches = rows.iter().filter(|row| {
                        row["sid"] == sid && row["source"] == json!(record.spec().source())
                    });
                    let first = matches.next();
                    if matches.next().is_some() {
                        None
                    } else {
                        first
                    }
                } else {
                    None
                };
                let Some(owner) = owner.filter(|row| relations::top_level(row)) else {
                    continue;
                };
                let uid = owner["uid"].as_str().expect("session uid");
                metadata::update(&metadata, &events, |store| {
                    Ok(store.initialize_continuation(uid, edge.clone())?)
                })
                .map_err(|error| crate::sessions::SessionError {
                    status: error.status.as_u16(),
                    message: error.message,
                })?;
            }
            Ok(())
        })
        .await
}

/// SDK creation already has a stable UID before the first native message.
/// Keep continuation metadata under that identity while inventory catches up.
pub(super) async fn sdk_continuation(
    state: &AppState,
    uid: Option<String>,
    remote: Option<Predecessor>,
    authenticated_hub: bool,
) -> Result<Option<Predecessor>, ApiError> {
    if let Some(edge) = remote {
        if !authenticated_hub {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "relations_remote_hub",
                "跨机器关系需要通过 Hub 验证",
            ));
        }
        return Ok(Some(edge));
    }
    let Some(uid) = uid else {
        return Ok(None);
    };
    state
        .reader
        .run(move |store| {
            let inventory = store.list(true)?;
            let source = inventory["sessions"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["uid"] == uid))
                .ok_or_else(|| crate::sessions::SessionError {
                    status: 404,
                    message: "前序会话不存在".into(),
                })?;
            Ok(Some(relations::descriptor(source, source)))
        })
        .await
}

pub(super) fn record_sdk_continuation(
    state: &AppState,
    uid: &str,
    edge: Option<Predecessor>,
) -> Result<(), ApiError> {
    if let (Some(store), Some(edge)) = (&state.metadata, edge) {
        metadata::update(store, &state.ui_events, |store| {
            Ok(store.initialize_continuation(uid, edge)?)
        })?;
    }
    Ok(())
}
