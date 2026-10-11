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
