//! Read-only session list, messages, grant pages, input history, and SSE watch.
//! Page queries use opaque cursors. The publisher owns file checks and each
//! subscriber renders its own checkpoint. `debug_run`
//! selects the list view (`SessionStore::list_view`).
//! Main-session packets carry the live `prompt` (`bridge::live`: Claude
//! question-card file, Codex approval screen),
//! and the watch loop emits `prompt_only` packets when only that changes.
//! This layer does not interpret native history or move live cursors.
use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{StatusCode, header},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    bridge::{CodexProbe, LivePrompts, PromptScope},
    error::ApiError,
    files::FileService,
    media::MediaStore,
    sessions::{MessageBody, MessageQuery, PageStore, SessionError, SessionStore},
    state::{AppState, JsonBytes},
};

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ListQuery {
    force: String,
    sig: String,
    debug_run: String,
}

/// Responses above this many bytes schedule a coalesced `malloc_trim` once
/// they have left the process (docs/read-model.md "常驻内存预算").
const LARGE_RESPONSE: usize = 4 * 1024 * 1024;

fn query_error(error: QueryRejection) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_query", error.body_text())
}

pub async fn list(
    State(state): State<AppState>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<JsonBytes, ApiError> {
    let Query(query) = query.map_err(query_error)?;
    // The first 64 characters; an id the registry does
    // not know (or a malformed one) is an empty view, never an error.
    let debug_run: String = query.debug_run.chars().take(64).collect();
    state
        .reader
        .run(move |store| {
            let value = store.list_view_unless(query.force == "1", &debug_run, &query.sig)?;
            Ok(JsonBytes::new(&value))
        })
        .await
}

pub async fn messages(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    query: Result<Query<MessageQuery>, QueryRejection>,
) -> Result<JsonBytes, ApiError> {
    let Query(query) = query.map_err(query_error)?;
    let media = state.media.clone();
    let files = state.files.clone();
    let pages = state.history_pages.clone();
    let prompts = state.prompts.clone();
    let (body, scope, mut prompt) = state
        .reader
        .run(move |store| {
            let snapshot = store.snapshot(&uid, &query.agent)?;
            let scope = query.agent.is_empty().then(|| PromptScope::of(&snapshot));
            // A hot read splices retained bytes and leaves no projection
            // temporaries behind (a cold open's parse trims its own in
            // `Views::file`); what a large response leaves is its own
            // buffer, freed after the send, so the trim runs later and off
            // this path instead of costing every big read ~10 ms.
            let body = snapshot.messages_body(&query, &media, files.as_deref(), &pages)?;
            let prompt = claude_prompt_field(&prompts, scope.as_ref(), &snapshot, &body);
            if body.size() > LARGE_RESPONSE {
                crate::sessions::memory::release_soon();
            }
            Ok((body, scope, prompt))
        })
        .await?;
    // `result["prompt"]` is set for every main view:
    // null unless a Claude card or a Codex approval is live.
    if let Some(scope) = scope {
        codex_prompt_field(
            &state,
            scope.as_ref(),
            &mut prompt,
            &mut CodexProbe::default(),
        )
        .await;
    }
    Ok(JsonBytes(body.finish(prompt.as_ref())))
}

/// The Claude half of the `prompt` field, on the blocking reader (it reads
/// and may clear the card file). Runs only for a main view (`scope` given):
/// `Some(prompt)` is the field to append, `None` omits it (agent views).
fn claude_prompt_field(
    prompts: &LivePrompts,
    scope: Option<&Option<PromptScope>>,
    snapshot: &Arc<crate::sessions::ViewSnapshot>,
    body: &MessageBody,
) -> Option<Value> {
    match scope {
        Some(Some(PromptScope::Claude { sid })) => {
            Some(prompts.claude_prompt_unless(sid, |tool_id| body.answers(snapshot, tool_id)))
        }
        Some(_) => Some(Value::Null),
        None => None,
    }
}

/// The Codex half of the `prompt` field: a screen capture of the managed
/// instance, on the async runtime after the blocking read.
async fn codex_prompt_field(
    state: &AppState,
    scope: Option<&PromptScope>,
    prompt: &mut Option<Value>,
    probe: &mut CodexProbe,
) {
    if let Some(PromptScope::Codex { uid }) = scope {
        *prompt = Some(state.prompts.codex_prompt(state, uid, probe).await);
    }
}

/// Opaque grant lookups for history pages and per-message media pages. Both
/// ignore unrelated query parameters.
#[derive(Deserialize)]
pub struct PageQuery {
    cursor: String,
    #[serde(default)]
    agent: String,
    /// `debug_run`: the page's view selector, appended to every `/api/`
    /// URL by the frontend; accepted and ignored here.
    #[serde(default)]
    #[allow(dead_code)]
    debug_run: String,
}
struct PageBody {
    bytes: Vec<u8>,
    _permit: Arc<tokio::sync::OwnedSemaphorePermit>,
}
impl AsRef<[u8]> for PageBody {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
/// Shared read resources for one grant-based page read on the blocking reader.
struct PageResources {
    pages: Arc<PageStore>,
    media: Arc<MediaStore>,
    files: Option<Arc<FileService>>,
}
/// Common admission, response budget and headers for grant-based page reads.
/// One of the response permits (`Pools::responses`, queued within the bounded
/// wait) is held until the response body is released.
async fn page_response<F>(
    state: AppState,
    query: Result<Query<PageQuery>, QueryRejection>,
    work: F,
) -> Result<Response, ApiError>
where
    F: FnOnce(&SessionStore, &PageResources, &PageQuery) -> Result<Vec<u8>, SessionError>
        + Send
        + 'static,
{
    let Query(query) = query.map_err(query_error)?;
    let resources = PageResources {
        pages: state.history_pages.clone(),
        media: state.media.clone(),
        files: state.files.clone(),
    };
    let permit =
        Arc::new(crate::state::admit(&state.history_page_http, "history_page_busy").await?);
    let worker_permit = permit.clone();
    let bytes = state
        .reader
        .run(move |store| {
            let _permit = worker_permit;
            Ok(JsonBytes(work(store, &resources, &query)?))
        })
        .await?;
    let length = bytes.0.len();
    if length > LARGE_RESPONSE {
        crate::sessions::memory::release_soon();
    }
    let mut response = Body::from(Bytes::from_owner(PageBody {
        bytes: bytes.0,
        _permit: permit,
    }))
    .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    headers.insert(header::CONTENT_LENGTH, length.into());
    headers.insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("x-sessiondock-decoded-length", length.into());
    Ok(response)
}
pub async fn history_page(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    page_response(state, query, move |store, resources, query| {
        let grant = resources.pages.lookup(&query.cursor, &uid, &query.agent)?;
        store.snapshot(&uid, &query.agent)?.history_page_body(
            grant,
            &query.cursor,
            &resources.media,
            resources.files.as_deref(),
            &resources.pages,
        )
    })
    .await
}
/// Continuation of one message's typed images beyond the inline limit.
pub async fn media_page(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    page_response(state, query, move |store, resources, query| {
        let grant = resources
            .pages
            .lookup_media(&query.cursor, &uid, &query.agent)?;
        let value = store.snapshot(&uid, &query.agent)?.media_page(
            grant,
            &query.cursor,
            &resources.media,
            resources.files.as_deref(),
            &resources.pages,
        )?;
        Ok(JsonBytes::new(&value).0)
    })
    .await
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct ViewQuery {
    uid: String,
    agent: String,
    start: u64,
    head: String,
    anchor: String,
}

impl ViewQuery {
    fn cursor(&self) -> MessageQuery {
        MessageQuery {
            agent: self.agent.clone(),
            start: self.start,
            head: self.head.clone(),
            anchor: self.anchor.clone(),
            window: "1".into(),
            ..Default::default()
        }
    }
}

pub async fn input_history(
    State(state): State<AppState>,
    query: Result<Query<ViewQuery>, QueryRejection>,
) -> Result<JsonBytes, ApiError> {
    let Query(query) = query.map_err(query_error)?;
    let cursor = MessageQuery {
        agent: query.agent,
        ..Default::default()
    };
    state
        .reader
        .run(move |store| {
            let value = store.messages(&query.uid, &cursor)?;
            let history: Vec<Value> = value["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| {
                    matches!(m["role"].as_str(), Some("user" | "command"))
                        && m["counted"] != false
                        && !m["text"].as_str().unwrap_or("").trim().is_empty()
                })
                .map(|m| json!({"text": m["text"], "ts": m["ts"]}))
                .collect();
            Ok(JsonBytes::new(
                &json!({"history": history, "end": value["end"],
            "version": value["version"], "anchor": value["anchor"]}),
            ))
        })
        .await
}

struct Packet {
    body: MessageBody,
    /// The `prompt` field to append (`None` on agent views).
    prompt: Option<Value>,
    query: MessageQuery,
}

impl Packet {
    /// The SSE data (the document with its `prompt` appended) and the
    /// cursor the next packet continues from.
    fn data(self) -> (String, MessageQuery) {
        let data = String::from_utf8(self.body.finish(self.prompt.as_ref()))
            .expect("serde_json output and its spliced copies are UTF-8");
        if data.len() > LARGE_RESPONSE {
            crate::sessions::memory::release_soon();
        }
        (data, self.query)
    }
}

fn packet(body: MessageBody, prompt: Option<Value>, requested: MessageQuery) -> Packet {
    let query = MessageQuery {
        start: body.end(),
        head: body.head().into(),
        anchor: body.anchor().into(),
        agent: requested.agent,
        window: "1".into(),
        ..Default::default()
    };
    Packet {
        body,
        prompt,
        query,
    }
}

/// Emit `{"prompt_only": true, "prompt": ...}` as a `prompt` packet.
fn prompt_only(prompt: Value) -> String {
    json!({"prompt_only": true, "prompt": prompt}).to_string()
}

/// Poll spacing for the Claude file stamp / Codex screen (0.4 s).
const PROMPT_POLL: Duration = Duration::from_millis(500);

pub async fn watch(
    State(state): State<AppState>,
    query: Result<Query<ViewQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query.map_err(query_error)?;
    let cursor = query.cursor();
    let mut subscription = state.observations.subscribe(query.uid, query.agent).await?;
    let snapshot = subscription.current()?;
    // `claude_sid` / `codex_session` only for a main session; the
    // scope comes from this view's validated records.
    let scope = cursor
        .agent
        .is_empty()
        .then(|| PromptScope::of(&snapshot))
        .flatten();
    let media = state.media.clone();
    let files = state.files.clone();
    let pages = state.history_pages.clone();
    let prompts = state.prompts.clone();
    let first_scope = scope.clone();
    let mut probe = CodexProbe::default();
    let mut first = state
        .reader
        .run(move |_| {
            let body = snapshot.messages_body(&cursor, &media, files.as_deref(), &pages)?;
            let prompt = if first_scope.is_some() {
                claude_prompt_field(&prompts, Some(&first_scope), &snapshot, &body)
            } else {
                None
            };
            Ok(packet(body, prompt, cursor))
        })
        .await?;
    codex_prompt_field(&state, scope.as_ref(), &mut first.prompt, &mut probe).await;
    let stream = async_stream::stream! {
        // `prompt_revision` / `codex_prompt`: what the last packet carried.
        let mut claude_revision = match &scope {
            Some(PromptScope::Claude { sid }) => state.prompts.claude_revision(sid),
            _ => None,
        };
        let mut codex_prompt = first.prompt.clone().unwrap_or(Value::Null);
        let mut poll = tokio::time::interval(PROMPT_POLL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // A byte checkpoint does not encode the browser's metadata version.
        // Always send one aligned batch so a reconnect catches a preference or
        // menu change that occurred between its HTTP snapshot and subscribe.
        let (first_data, mut cursor) = first.data();
        yield Ok::<Event, Infallible>(Event::default().data(first_data));
        loop {
            let changed = tokio::select! {
                _ = state.shutdown.cancelled() => break,
                changed = subscription.changed() => changed,
                _ = poll.tick(), if scope.is_some() => {
                    // A changed card file stamp or approval screen
                    // without new records is a `prompt_only` packet.
                    match &scope {
                        Some(PromptScope::Claude { sid }) => {
                            let live = state.prompts.clone();
                            let wanted = sid.clone();
                            let current = tokio::task::spawn_blocking(move || {
                                (live.claude_revision(&wanted), live.claude_prompt_raw(&wanted))
                            }).await.unwrap_or((None, Value::Null));
                            if current.0 != claude_revision {
                                claude_revision = current.0;
                                yield Ok(Event::default().data(prompt_only(current.1)));
                            }
                        }
                        Some(PromptScope::Codex { uid }) => {
                            let current = state.prompts.codex_prompt(&state, uid, &mut probe).await;
                            if current != codex_prompt {
                                codex_prompt = current.clone();
                                yield Ok(Event::default().data(prompt_only(current)));
                            }
                        }
                        None => {}
                    }
                    continue;
                }
            };
            let request_cursor = cursor.clone();
            let snapshot = changed.and_then(|_| subscription.current());
            let media = state.media.clone();
            let files = state.files.clone();
            let pages = state.history_pages.clone();
            let prompts = state.prompts.clone();
            let packet_scope = scope.clone();
            let result = match snapshot {
                Ok(snapshot) => state.reader.run_wait(&state.shutdown, move |_| {
                    let body = snapshot.messages_body(&request_cursor, &media, files.as_deref(), &pages)?;
                    let prompt = if packet_scope.is_some() {
                        claude_prompt_field(&prompts, Some(&packet_scope), &snapshot, &body)
                    } else {
                        None
                    };
                    Ok(packet(body, prompt, request_cursor))
                }).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(mut next) => {
                    codex_prompt_field(&state, scope.as_ref(), &mut next.prompt, &mut probe).await;
                    match &scope {
                        // `_claude_prompt` may have deleted the file: resync the stamp.
                        Some(PromptScope::Claude { sid }) => claude_revision = state.prompts.claude_revision(sid),
                        Some(PromptScope::Codex { .. }) => codex_prompt = next.prompt.clone().unwrap_or(Value::Null),
                        None => {}
                    }
                    // The publisher already compares the complete revision,
                    // including view metadata. A changed agent menu/preference
                    // can have exactly the same byte checkpoint and no messages.
                    let (data, query) = next.data();
                    yield Ok(Event::default().data(data));
                    cursor = query;
                }
                Err(error) => {
                    yield Ok(Event::default().event("migration-error")
                        .data(json!({"error": error.message, "code": error.code,
                            "status": error.status.as_u16()}).to_string()));
                    break;
                }
            }
        }
    };
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    Ok(response)
}
