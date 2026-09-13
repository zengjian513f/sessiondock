//! Hub aggregation (`hub.py` `HubHandler.selected/aggregate/search_aggregate/
//! bulk_delete/bulk_fork_visibility/purge_all`, 597–880): the five read
//! routes merged across the selected nodes, the NDJSON search stream, and the
//! three writes that fan out per machine. Every node answer already carries
//! the wire namespace (the registry applies `namespace::public_payload`);
//! this module only merges, sorts, signs and reports.
//!
//! Nothing here binds a listener: H4 turns `AggregateError` into a 400
//! `{"error": …}` and `search_stream` into a `application/x-ndjson` body.

#[cfg(test)]
mod tests;

use std::{
    cmp::Reverse,
    fmt,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::{Stream, StreamExt};
use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, mpsc};

use super::{
    client::Client,
    namespace::{self, NamespaceError, truthy},
    registry::{Fetched, Node, Registry, SearchEvent},
};

/// Nodes queried at once (`ThreadPoolExecutor(max_workers=min(16, n))`).
pub const FAN_OUT: usize = 16;
/// A search stream emits `{"type":"heartbeat"}` after this long without a
/// node event so browsers and proxies keep the connection open.
pub const HEARTBEAT: Duration = Duration::from_secs(1);
/// Query keys the browser addresses to the hub, never forwarded to nodes.
const HUB_KEYS: [&str; 3] = ["nodes", "sig", "progress"];
const REQUEST_FAILED: &str = "机器请求失败，请核对结果";
pub const TRASH_DIR: &str = "所选机器的本地回收站";

/// Browser query pairs in wire order (`parse_qs(keep_blank_values=True)`).
pub type Params = [(String, String)];

/// Rejected input (Python `ValueError` → 400 `{"error": text}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AggregateError(pub String);

impl AggregateError {
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AggregateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AggregateError {}

impl From<NamespaceError> for AggregateError {
    fn from(error: NamespaceError) -> Self {
        Self(error.message().to_string())
    }
}

fn invalid(message: &str) -> AggregateError {
    AggregateError(message.to_string())
}

/// First value of `key` (`query.get(key, [""])[0]`).
pub fn first<'a>(query: &'a Params, key: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// `progress=1` asks for the NDJSON search stream instead of one JSON body.
pub fn progress_requested(query: &Params) -> bool {
    first(query, "progress") == Some("1")
}

/// `HubHandler.selected`: the enabled nodes, or those named by `?nodes=a,b`
/// (registry order; an empty list selects nothing). Unknown or disabled ids
/// are rejected.
pub fn selected(registry: &Registry, query: &Params) -> Result<Vec<Node>, AggregateError> {
    let nodes = registry.all();
    let Some(ids) = first(query, "nodes") else {
        return Ok(nodes);
    };
    let wanted: Vec<&str> = ids.split(',').filter(|id| !id.is_empty()).collect();
    if wanted
        .iter()
        .any(|id| !nodes.iter().any(|node| node.id == *id))
    {
        return Err(invalid("筛选包含未注册的机器"));
    }
    Ok(nodes
        .into_iter()
        .filter(|node| wanted.contains(&node.id.as_str()))
        .collect())
}

/// The query forwarded to nodes: hub keys dropped, values grouped per key in
/// first-appearance order like `urlencode(parse_qs(...), doseq=True)`.
pub fn upstream(query: &Params) -> Vec<(String, String)> {
    let mut grouped: IndexMap<&str, Vec<&str>> = IndexMap::new();
    for (key, value) in query {
        if !HUB_KEYS.contains(&key.as_str()) {
            grouped
                .entry(key.as_str())
                .or_default()
                .push(value.as_str());
        }
    }
    grouped
        .into_iter()
        .flat_map(|(key, values)| {
            values
                .into_iter()
                .map(move |value| (key.to_string(), value.to_string()))
        })
        .collect()
}

/// One node's answer to an aggregate query.
#[derive(Clone, Debug, PartialEq)]
struct Answer {
    node: Node,
    data: Value,
    failure: Option<Value>,
}

impl Answer {
    fn ok(&self) -> bool {
        self.failure.is_none()
    }

    fn rows(&self, key: &str) -> Vec<Value> {
        self.data
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn flag(&self, key: &str) -> bool {
        self.data.get(key).is_some_and(truthy)
    }
}

/// Query every selected node at once, answers in selection order.
async fn gather(
    registry: &Registry,
    client: &Client,
    nodes: Vec<Node>,
    path: &str,
    query: &Params,
) -> Vec<Answer> {
    futures_util::stream::iter(nodes)
        .map(|node| async move {
            let Fetched { data, failure } = registry.fetch(client, &node, path, query, None).await;
            Answer {
                node,
                data,
                failure,
            }
        })
        .buffered(FAN_OUT)
        .collect()
        .await
}

/// `{"errors", "partial", "nodes"}` — the start of every aggregate body.
fn envelope(registry: &Registry, answers: &[Answer]) -> Map<String, Value> {
    let errors: Vec<Value> = answers
        .iter()
        .filter_map(|answer| answer.failure.clone())
        .collect();
    let partial = !errors.is_empty();
    let mut result = Map::new();
    result.insert("errors".into(), Value::Array(errors));
    result.insert("partial".into(), Value::Bool(partial));
    result.insert("nodes".into(), Value::Array(registry.public()));
    result
}

/// Rows across nodes in the hub's order: newest `updated` first, ties by
/// uid descending (`sort(key=(updated, uid), reverse=True)`).
fn merged_rows(answers: &[Answer], key: &str) -> Vec<Value> {
    let mut rows: Vec<Value> = answers.iter().flat_map(|answer| answer.rows(key)).collect();
    let text = |row: &Value, key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    rows.sort_by_cached_key(|row| Reverse((text(row, "updated"), text(row, "uid"))));
    rows
}

/// `sessions`/`results` plus `truncated`, `truncated_nodes`, `total_pool`.
fn merge_rows(result: &mut Map<String, Value>, answers: &[Answer], key: &str) {
    result.insert(key.into(), Value::Array(merged_rows(answers, key)));
    result.insert(
        "truncated".into(),
        Value::Bool(answers.iter().any(|answer| answer.flag("truncated"))),
    );
    result.insert(
        "truncated_nodes".into(),
        Value::Array(
            answers
                .iter()
                .filter(|answer| answer.flag("truncated"))
                .map(|answer| Value::String(answer.node.id.clone()))
                .collect(),
        ),
    );
    result.insert(
        "total_pool".into(),
        sum(answers.iter().map(|answer| answer.data.get("total_pool"))),
    );
}

/// `GET /api/sessions`: merged rows signed with `sig`; when the browser's
/// `?sig=` still matches, only `{unchanged, sig, nodes, errors}` goes back.
pub async fn sessions(
    registry: &Registry,
    client: &Client,
    query: &Params,
) -> Result<Value, AggregateError> {
    let nodes = selected(registry, query)?;
    let answers = gather(registry, client, nodes, "/api/sessions", &upstream(query)).await;
    Ok(sessions_body(
        registry,
        &answers,
        first(query, "sig").unwrap_or(""),
    ))
}

fn sessions_body(registry: &Registry, answers: &[Answer], known: &str) -> Value {
    let mut result = envelope(registry, answers);
    merge_rows(&mut result, answers, "sessions");
    let sig = signature(&result);
    if known == sig {
        return json!({
            "unchanged": true, "sig": sig,
            "nodes": result["nodes"], "errors": result["errors"],
        });
    }
    result.insert("sig".into(), Value::String(sig));
    result.insert("built_at".into(), json!(now()));
    Value::Object(result)
}

/// `GET /api/search` without `progress=1`: one JSON body like `sessions`
/// (unsigned).
pub async fn search(
    registry: &Registry,
    client: &Client,
    query: &Params,
) -> Result<Value, AggregateError> {
    let nodes = selected(registry, query)?;
    let answers = gather(registry, client, nodes, "/api/search", &upstream(query)).await;
    Ok(Value::Object(search_body(registry, &answers)))
}

fn search_body(registry: &Registry, answers: &[Answer]) -> Map<String, Value> {
    let mut result = envelope(registry, answers);
    merge_rows(&mut result, answers, "results");
    result
}

/// `GET /api/live`: uids and start times of the nodes that answered.
pub async fn live(
    registry: &Registry,
    client: &Client,
    query: &Params,
) -> Result<Value, AggregateError> {
    let nodes = selected(registry, query)?;
    let answers = gather(registry, client, nodes, "/api/live", &upstream(query)).await;
    Ok(live_body(registry, &answers))
}

fn live_body(registry: &Registry, answers: &[Answer]) -> Value {
    let mut result = envelope(registry, answers);
    for key in ["uids", "tmux_uids"] {
        let rows: Vec<Value> = answers
            .iter()
            .filter(|answer| answer.ok())
            .flat_map(|answer| answer.rows(key))
            .collect();
        result.insert(key.into(), Value::Array(rows));
    }
    let mut started = Map::new();
    for answer in answers.iter().filter(|answer| answer.ok()) {
        if let Some(entries) = answer.data.get("started_at").and_then(Value::as_object) {
            for (uid, value) in entries {
                started.insert(uid.clone(), value.clone());
            }
        }
    }
    result.insert("started_at".into(), Value::Object(started));
    Value::Object(result)
}

/// `GET /api/term/list`: rows of every node (stale ones included) plus a
/// per-machine `capabilities` map; `sources` is the union of the reachable
/// nodes' sources.
pub async fn term_list(
    registry: &Registry,
    client: &Client,
    query: &Params,
) -> Result<Value, AggregateError> {
    let nodes = selected(registry, query)?;
    let answers = gather(registry, client, nodes, "/api/term/list", &upstream(query)).await;
    Ok(term_list_body(registry, &answers))
}

fn term_list_body(registry: &Registry, answers: &[Answer]) -> Value {
    let mut result = envelope(registry, answers);
    result.insert(
        "enabled".into(),
        Value::Bool(answers.iter().any(|answer| answer.flag("enabled"))),
    );
    result.insert("home".into(), Value::String(String::new()));
    result.insert("sources".into(), json!({}));
    for key in ["sessions", "pending"] {
        let rows: Vec<Value> = answers.iter().flat_map(|answer| answer.rows(key)).collect();
        result.insert(key.into(), Value::Array(rows));
    }
    let mut sources = Map::new();
    let mut capabilities = Map::new();
    for answer in answers {
        let data = &answer.data;
        let get = |key: &str, default: Value| data.get(key).cloned().unwrap_or(default);
        let reason = match &answer.failure {
            Some(failure) => failure["error"].clone(),
            None => get("unavailable_reason", json!("")),
        };
        capabilities.insert(
            answer.node.id.clone(),
            json!({
                "enabled": answer.flag("enabled") && answer.ok(),
                "unavailable_reason": reason,
                "sources": get("sources", json!({})),
                "home": get("home", json!("")),
                // 终端后端是每台机器各自的设置，网页按机器分别展示和切换。
                "backend": get("backend", json!("")),
                "backends": if answer.ok() { get("backends", json!([])) } else { json!([]) },
            }),
        );
        if answer.ok()
            && let Some(entries) = data.get("sources").and_then(Value::as_object)
        {
            for (source, available) in entries {
                // Python `sources.get(source, False) or available`.
                if !sources.get(source).is_some_and(truthy) {
                    sources.insert(source.clone(), available.clone());
                }
            }
        }
    }
    result.insert("sources".into(), Value::Object(sources));
    result.insert("capabilities".into(), Value::Object(capabilities));
    Value::Object(result)
}

/// `GET /api/trash`: items of every node with their sizes summed.
pub async fn trash(
    registry: &Registry,
    client: &Client,
    query: &Params,
) -> Result<Value, AggregateError> {
    let nodes = selected(registry, query)?;
    let answers = gather(registry, client, nodes, "/api/trash", &upstream(query)).await;
    Ok(trash_body(registry, &answers))
}

fn trash_body(registry: &Registry, answers: &[Answer]) -> Value {
    let mut result = envelope(registry, answers);
    let items: Vec<Value> = answers
        .iter()
        .flat_map(|answer| answer.rows("items"))
        .collect();
    result.insert("items".into(), Value::Array(items));
    result.insert(
        "size".into(),
        sum(answers.iter().map(|answer| answer.data.get("size"))),
    );
    result.insert("dir".into(), Value::String(TRASH_DIR.into()));
    Value::Object(result)
}

// ---- search stream ------------------------------------------------------------

/// What a node scan reports into the shared queue.
enum Event {
    Progress { done: u64, total: u64 },
    Matches(Vec<Value>),
    Result(Fetched),
}

/// One row of the `progress` event's `nodes`.
struct Scan {
    node: Node,
    done: Value,
    total: Value,
    state: &'static str,
}

impl Scan {
    fn row(&self) -> Value {
        json!({
            "id": self.node.id, "name": self.node.name,
            "done": self.done, "total": self.total, "state": self.state,
        })
    }
}

fn progress_line(scans: &IndexMap<String, Scan>) -> String {
    let rows: Vec<Value> = scans.values().map(Scan::row).collect();
    line(&json!({
        "type": "progress",
        "done": sum(scans.values().map(|scan| Some(&scan.done))),
        "total": sum(scans.values().map(|scan| truthy(&scan.total).then_some(&scan.total))),
        "total_known": scans.values().all(|scan| !scan.total.is_null()),
        "nodes": rows,
    }))
}

/// One NDJSON line (`json.dumps(event, ensure_ascii=False) + "\n"`).
fn line(event: &Value) -> String {
    let mut text = serde_json::to_string(event).expect("JSON value serializes");
    text.push('\n');
    text
}

/// Aborts the node scans when the browser goes away mid-stream.
struct AbortOnDrop(Vec<tokio::task::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// `GET /api/search?progress=1`: the NDJSON stream — an initial `progress`
/// line, then `progress` (per node scan step), `matches`, `heartbeat`
/// (after `HEARTBEAT` without events) and one final `result` line whose
/// `data` is the JSON `search` body. Selection errors surface before the
/// stream exists so the caller can still answer 400.
pub fn search_stream(
    registry: Arc<Registry>,
    client: Arc<Client>,
    query: &Params,
) -> Result<impl Stream<Item = String> + Send + 'static, AggregateError> {
    let nodes = selected(&registry, query)?;
    let upstream: Arc<Vec<(String, String)>> = Arc::new(upstream(query));
    Ok(async_stream::stream! {
        let (tx, mut rx) = mpsc::channel::<(String, Event)>(64);
        let pool = Arc::new(Semaphore::new(FAN_OUT));
        let mut scans: IndexMap<String, Scan> = IndexMap::new();
        let mut tasks = Vec::new();
        for node in &nodes {
            let offline = registry.offline(&node.id);
            scans.insert(node.id.clone(), Scan {
                node: node.clone(),
                done: json!(0),
                total: if offline { json!(0) } else { Value::Null },
                state: if offline { "offline" } else { "preparing" },
            });
            tasks.push(tokio::spawn(scan(
                registry.clone(), client.clone(), node.clone(), upstream.clone(),
                pool.clone(), tx.clone(),
            )));
        }
        drop(tx);
        let _guard = AbortOnDrop(tasks);
        yield progress_line(&scans);
        let mut answers: Vec<Answer> = Vec::new();
        while answers.len() < nodes.len() {
            let (nid, event) = match tokio::time::timeout(HEARTBEAT, rx.recv()).await {
                Err(_) => {
                    // Keep the browser/proxy alive while a node parses a large file.
                    yield line(&json!({"type": "heartbeat"}));
                    continue;
                }
                Ok(None) => break,
                Ok(Some(event)) => event,
            };
            let Some(scan) = scans.get_mut(&nid) else { continue };
            match event {
                Event::Progress { done, total } => {
                    scan.done = json!(done);
                    scan.total = json!(total);
                    scan.state = "scanning";
                    yield progress_line(&scans);
                }
                Event::Matches(rows) => {
                    yield line(&json!({"type": "matches", "results": rows}));
                }
                Event::Result(Fetched { data, failure }) => {
                    let node = scan.node.clone();
                    if failure.is_some() {
                        scan.state = if registry.offline(&nid) { "offline" } else { "error" };
                        if scan.state == "offline" && scan.total.is_null() {
                            scan.total = json!(0);
                        }
                    } else {
                        let total = if scan.total.is_null() {
                            data.get("total_pool").cloned().unwrap_or(json!(0))
                        } else {
                            scan.total.clone()
                        };
                        let truncated = data.get("truncated").is_some_and(truthy);
                        scan.done = match data.get("scanned") {
                            Some(scanned) => scanned.clone(),
                            None if truncated => scan.done.clone(),
                            None => total.clone(),
                        };
                        scan.total = total;
                        scan.state = if truncated { "limited" } else { "done" };
                    }
                    yield progress_line(&scans);
                    if let Some(rows) = data.get("results").filter(|rows| truthy(rows)) {
                        yield line(&json!({"type": "matches", "results": rows}));
                    }
                    answers.push(Answer { node, data, failure });
                }
            }
        }
        yield line(&json!({"type": "result", "data": search_body(&registry, &answers)}));
    })
}

/// One node's scan: forward its progress/matches, then its final answer,
/// strictly in that order.
async fn scan(
    registry: Arc<Registry>,
    client: Arc<Client>,
    node: Node,
    upstream: Arc<Vec<(String, String)>>,
    pool: Arc<Semaphore>,
    tx: mpsc::Sender<(String, Event)>,
) {
    let Ok(_permit) = pool.acquire().await else {
        return;
    };
    let (events, mut received) = mpsc::channel::<SearchEvent>(64);
    let forward = {
        let tx = tx.clone();
        let nid = node.id.clone();
        tokio::spawn(async move {
            while let Some(event) = received.recv().await {
                let event = match event {
                    SearchEvent::Progress { done, total } => Event::Progress { done, total },
                    SearchEvent::Matches(rows) => Event::Matches(rows),
                };
                if tx.send((nid.clone(), event)).await.is_err() {
                    break;
                }
            }
        })
    };
    let fetched = registry
        .fetch(&client, &node, "/api/search", &upstream, Some(&events))
        .await;
    drop(events);
    let _ = forward.await;
    let _ = tx.send((node.id, Event::Result(fetched))).await;
}

// ---- writes split per machine ---------------------------------------------------

/// Scoped uids grouped by machine in first-appearance order, local uids kept.
fn group_uids(
    body: &Value,
    empty: &'static str,
) -> Result<IndexMap<String, Vec<String>>, AggregateError> {
    let mut groups: IndexMap<String, Vec<String>> = IndexMap::new();
    if let Some(uids) = body.get("uids").and_then(Value::as_array) {
        for uid in uids {
            let uid = uid
                .as_str()
                .ok_or_else(|| invalid("missing session source"))?;
            let (nid, local) = namespace::split(uid, true)?;
            groups.entry(nid).or_default().push(local);
        }
    }
    if groups.is_empty() {
        return Err(invalid(empty));
    }
    Ok(groups)
}

/// One machine's share of a bulk write: `POST path body`, 200 only, payload
/// scoped; otherwise `None`.
async fn write(
    registry: &Registry,
    client: &Client,
    node: &Node,
    path: &str,
    body: Value,
) -> Option<Value> {
    match registry
        .request(client, node, path, "POST", Some(&body), client.timeout)
        .await
    {
        Ok((200, data)) => Some(namespace::public_payload(data, node, path)),
        _ => None,
    }
}

fn extend(result: &mut Map<String, Value>, key: &str, data: &Value) {
    if let (Some(target), Some(rows)) = (
        result.get_mut(key).and_then(Value::as_array_mut),
        data.get(key).and_then(Value::as_array),
    ) {
        target.extend(rows.iter().cloned());
    }
}

fn failed(result: &mut Map<String, Value>, nid: &str, uids: &[String]) {
    if let Some(errors) = result.get_mut("errors").and_then(Value::as_array_mut) {
        for uid in uids {
            let scoped = namespace::qualify(nid, uid, true).unwrap_or_else(|_| uid.clone());
            errors.push(json!({"uid": scoped, "error": REQUEST_FAILED}));
        }
    }
}

/// `POST /api/sessions/delete` `{uids}`: one request per machine, its
/// `deleted`/`errors` merged; a machine that fails or is unknown reports
/// every one of its uids as an error.
pub async fn delete(
    registry: &Registry,
    client: &Client,
    body: &Value,
) -> Result<Value, AggregateError> {
    let groups = group_uids(body, "没有选中任何会话")?;
    let mut result = Map::from_iter([
        ("ok".to_string(), Value::Bool(true)),
        ("deleted".to_string(), json!([])),
        ("errors".to_string(), json!([])),
    ]);
    for (nid, uids) in &groups {
        let answer = match registry.get(nid) {
            Some(node) => {
                write(
                    registry,
                    client,
                    &node,
                    "/api/sessions/delete",
                    json!({"uids": uids}),
                )
                .await
            }
            None => None,
        };
        match answer {
            Some(data) => {
                extend(&mut result, "deleted", &data);
                extend(&mut result, "errors", &data);
            }
            None => failed(&mut result, nid, uids),
        }
    }
    Ok(Value::Object(result))
}

/// `POST /api/sessions/fork-visibility` `{uids, visible}` split per machine.
pub async fn fork_visibility(
    registry: &Registry,
    client: &Client,
    body: &Value,
) -> Result<Value, AggregateError> {
    let Some(visible) = body.get("visible").and_then(Value::as_bool) else {
        return Err(invalid("需要布尔值 visible"));
    };
    let groups = group_uids(body, "没有选中任何父会话")?;
    let mut result = Map::from_iter([
        ("ok".to_string(), Value::Bool(true)),
        ("updated".to_string(), json!([])),
        ("errors".to_string(), json!([])),
    ]);
    for (nid, uids) in &groups {
        let answer = match registry.get(nid) {
            Some(node) => {
                write(
                    registry,
                    client,
                    &node,
                    "/api/sessions/fork-visibility",
                    json!({"uids": uids, "visible": visible}),
                )
                .await
            }
            None => None,
        };
        match answer {
            Some(data) => {
                extend(&mut result, "updated", &data);
                extend(&mut result, "errors", &data);
            }
            None => failed(&mut result, nid, uids),
        }
    }
    Ok(Value::Object(result))
}

/// `POST /api/trash/purge` `{all:true}` on every selected machine, totals
/// summed and each machine's errors prefixed with its name.
pub async fn purge_all(
    registry: &Registry,
    client: &Client,
    query: &Params,
) -> Result<Value, AggregateError> {
    let nodes = selected(registry, query)?;
    let mut removed = Vec::new();
    let mut freed = Vec::new();
    let mut errors = Vec::new();
    for node in &nodes {
        match write(
            registry,
            client,
            node,
            "/api/trash/purge",
            json!({"all": true}),
        )
        .await
        {
            Some(data) => {
                removed.push(data.get("removed").cloned().unwrap_or(json!(0)));
                freed.push(data.get("freed").cloned().unwrap_or(json!(0)));
                if let Some(rows) = data.get("errors").and_then(Value::as_array) {
                    for error in rows {
                        let text = match error {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        };
                        errors.push(Value::String(format!("{}: {text}", node.name)));
                    }
                }
            }
            None => errors.push(Value::String(format!(
                "{}: 请求失败，请核对结果",
                node.name
            ))),
        }
    }
    Ok(json!({
        "ok": true,
        "removed": sum(removed.iter().map(Some)),
        "freed": sum(freed.iter().map(Some)),
        "errors": errors,
    }))
}

// ---- helpers ------------------------------------------------------------------

/// The list signature: sha256 of the sorted-key JSON of the body with the
/// node rows reduced to `{id, name, online}`, first 24 hex digits. Only ever
/// compared with a value this hub produced, so it need not match Python's
/// bytes.
fn signature(result: &Map<String, Value>) -> String {
    let mut stable = result.clone();
    let nodes: Vec<Value> = result
        .get("nodes")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let pick = |key: &str| row.get(key).cloned().unwrap_or(Value::Null);
                    json!({"id": pick("id"), "name": pick("name"), "online": pick("online")})
                })
                .collect()
        })
        .unwrap_or_default();
    stable.insert("nodes".into(), Value::Array(nodes));
    let mut text = String::new();
    sorted_json(&Value::Object(stable), &mut text);
    let digest = Sha256::digest(text.as_bytes());
    let mut hex = String::with_capacity(24);
    for byte in &digest[..12] {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// `json.dumps(value, sort_keys=True)`-shaped text: object keys sorted at
/// every level, compact separators.
fn sorted_json(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("string serializes"));
                out.push(':');
                sorted_json(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                sorted_json(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).expect("scalar serializes")),
    }
}

/// Python `sum(...)` over JSON numbers: an integer while every term is one,
/// a float otherwise; missing or non-numeric terms count as 0.
fn sum<'a>(values: impl Iterator<Item = Option<&'a Value>>) -> Value {
    let mut integer: i64 = 0;
    let mut float: f64 = 0.0;
    let mut is_float = false;
    for value in values.flatten() {
        if let Some(number) = value.as_i64() {
            integer = integer.saturating_add(number);
            float += number as f64;
        } else if let Some(number) = value.as_f64() {
            is_float = true;
            float += number;
        }
    }
    if is_float {
        json!(float)
    } else {
        json!(integer)
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}
