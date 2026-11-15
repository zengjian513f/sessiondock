//! Opt-in list transport. Cached revisions only save bytes: eviction, restart,
//! or an unknown revision always returns an ordinary complete snapshot.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{Method, header},
    middleware::Next,
    response::Response,
};
use serde_json::{Value, json};

pub const HEADER: &str = "x-sessiondock-list";
const REVISIONS: usize = 32;

#[derive(Clone, PartialEq)]
struct Rows {
    field: &'static str,
    order: Vec<String>,
    values: BTreeMap<String, Arc<Value>>,
}

fn key(row: &Value, field: &str) -> Option<String> {
    Some(format!(
        "{}\n{}",
        row["node_id"].as_str().unwrap_or(""),
        row[field].as_str()?
    ))
}

impl Rows {
    fn new(value: &Value, field: &'static str, old: Option<&Rows>) -> Option<Self> {
        let mut rows = Self {
            field,
            order: Vec::new(),
            values: BTreeMap::new(),
        };
        for row in value.as_array()? {
            let id = key(row, field)?;
            let shared = old
                .and_then(|old| old.values.get(&id))
                .filter(|v| v.as_ref() == row)
                .cloned()
                .unwrap_or_else(|| Arc::new(row.clone()));
            if rows.values.insert(id.clone(), shared).is_some() {
                return None;
            }
            rows.order.push(id);
        }
        Some(rows)
    }

    fn delta(&self, old: &Self) -> Value {
        let changed: BTreeSet<_> = self
            .values
            .iter()
            .filter(|(id, row)| old.values.get(*id) != Some(row))
            .map(|(id, _)| id)
            .collect();
        let removed: Vec<_> = old
            .order
            .iter()
            .filter(|id| !self.values.contains_key(*id))
            .collect();
        let retained_old: Vec<_> = old
            .order
            .iter()
            .filter(|id| self.values.contains_key(*id) && !changed.contains(id))
            .collect();
        let retained_new: Vec<_> = self
            .order
            .iter()
            .filter(|id| !changed.contains(id))
            .collect();
        let upsert: Vec<_> = self
            .order
            .iter()
            .enumerate()
            .filter(|(_, id)| changed.contains(id))
            .map(|(index, id)| match old.values.get(id) {
                Some(before) => row_patch(index, id, before, &self.values[id]),
                None => json!({"index":index,"row":self.values[id].as_ref()}),
            })
            .collect();
        let mut delta = json!({"key":self.field,"remove":removed,"upsert":upsert});
        // Usually updated rows move to the front. No O(list length) order vector
        // crosses the wire unless unchanged rows themselves were reordered.
        if retained_old != retained_new {
            delta["order"] = json!(self.order);
        }
        delta
    }
}

fn row_patch(index: usize, id: &str, before: &Value, after: &Value) -> Value {
    let (Some(before), Some(after)) = (before.as_object(), after.as_object()) else {
        return json!({"index":index,"row":after});
    };
    let mut set = serde_json::Map::new();
    let unset: Vec<_> = before
        .keys()
        .filter(|key| !after.contains_key(*key))
        .collect();
    let mut patch = json!({"index":index,"id":id,"unset":unset});
    for (field, value) in after {
        if before.get(field) == Some(value) {
            continue;
        }
        if field == "agent_items"
            && let Some(old) = before.get(field).and_then(|v| Rows::new(v, "id", None))
            && let Some(new) = Rows::new(value, "id", None)
        {
            patch["agents"] = new.delta(&old);
        } else {
            set.insert(field.clone(), value.clone());
        }
    }
    patch["set"] = Value::Object(set);
    patch
}

struct CachedWire {
    raw: axum::body::Bytes,
    envelope: serde_json::Map<String, Value>,
    complete: axum::body::Bytes,
    unchanged: axum::body::Bytes,
}

impl CachedWire {
    fn build(
        raw: axum::body::Bytes,
        data: &Value,
        rows: &BTreeMap<String, Rows>,
        version: &str,
    ) -> Option<Self> {
        let object = data.as_object()?;
        let mut envelope = object.clone();
        for name in rows.keys() {
            envelope.remove(name);
        }
        let mut full = object.clone();
        full.insert("list_version".to_owned(), json!(version));
        let complete = axum::body::Bytes::from(
            serde_json::to_vec(&Value::Object(full)).expect("JSON serializes"),
        );
        let empty_delta: BTreeMap<_, _> = rows
            .iter()
            .map(|(name, list)| {
                (
                    name.clone(),
                    json!({"key": list.field, "remove": [], "upsert": []}),
                )
            })
            .collect();
        let mut unch = envelope.clone();
        unch.insert(
            "list_delta".to_owned(),
            json!({"base": version, "collections": empty_delta}),
        );
        unch.insert("list_unchanged".to_owned(), json!(true));
        unch.insert("list_version".to_owned(), json!(version));
        let unchanged = axum::body::Bytes::from(
            serde_json::to_vec(&Value::Object(unch)).expect("JSON serializes"),
        );
        Some(Self {
            raw,
            envelope,
            complete,
            unchanged,
        })
    }

    fn matches(&self, bytes: &axum::body::Bytes) -> bool {
        self.raw.len() == bytes.len()
            && (std::ptr::eq(self.raw.as_ptr(), bytes.as_ptr()) || &self.raw == bytes)
    }
}

struct Snapshot {
    scope: String,
    version: String,
    rows: BTreeMap<String, Rows>,
    wire: Option<CachedWire>,
}

pub struct Store {
    snapshots: Mutex<VecDeque<Snapshot>>,
    jobs: Arc<tokio::sync::Semaphore>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            snapshots: Mutex::new(VecDeque::new()),
            jobs: Arc::new(tokio::sync::Semaphore::new(
                std::thread::available_parallelism()
                    .map_or(4, usize::from)
                    .clamp(2, 8),
            )),
        }
    }
}

type RemoteSnapshots = VecDeque<(String, Arc<Value>)>;

#[derive(Clone, Default)]
pub struct RemoteCache(Arc<Mutex<RemoteSnapshots>>);

impl RemoteCache {
    pub fn get(&self, scope: &str) -> Option<Arc<Value>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(key, _)| key == scope)
            .map(|(_, data)| data.clone())
    }

    pub fn put(&self, scope: String, data: Arc<Value>) {
        if !data["list_version"].is_string() {
            return;
        }
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|(key, _)| key != &scope);
        entries.push_back((scope, data));
        while entries.len() > REVISIONS {
            entries.pop_front();
        }
    }

    /// Mark `scope` recently used while it still holds `data`; a concurrent
    /// read that already stored a newer revision is left in place.
    pub fn refresh(&self, scope: &str, data: &Arc<Value>) {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = entries
            .iter()
            .position(|(key, current)| key == scope && Arc::ptr_eq(current, data))
            && let Some(entry) = entries.remove(index)
        {
            entries.push_back(entry);
        }
    }
}

impl Store {
    fn fast_reply(
        &self,
        scope: &str,
        known: &str,
        bytes: &axum::body::Bytes,
    ) -> Option<axum::body::Bytes> {
        let cache = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let previous = cache.iter().rev().find(|entry| entry.scope == scope)?;
        let wire = previous.wire.as_ref().filter(|w| w.matches(bytes))?;
        if known == previous.version {
            return Some(wire.unchanged.clone());
        }
        if !cache.iter().any(|s| s.scope == scope && s.version == known) {
            return Some(wire.complete.clone());
        }
        None
    }

    pub fn reply_bytes(
        &self,
        scope: &str,
        known: &str,
        input: axum::body::Bytes,
    ) -> Option<axum::body::Bytes> {
        {
            let cache = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(previous) = cache.iter().rev().find(|entry| entry.scope == scope)
                && let Some(wire) = previous.wire.as_ref().filter(|w| w.matches(&input))
            {
                if known == previous.version {
                    return Some(wire.unchanged.clone());
                }
                let Some(base) = cache
                    .iter()
                    .find(|s| s.scope == scope && s.version == known)
                else {
                    return Some(wire.complete.clone());
                };
                let delta: BTreeMap<_, _> = previous
                    .rows
                    .iter()
                    .map(|(name, list)| (name.clone(), list.delta(&base.rows[name])))
                    .collect();
                let unchanged = base.rows == previous.rows;
                let mut out = wire.envelope.clone();
                out.insert(
                    "list_delta".to_owned(),
                    json!({"base": known, "collections": delta}),
                );
                out.insert("list_unchanged".to_owned(), json!(unchanged));
                out.insert("list_version".to_owned(), json!(&previous.version));
                return Some(axum::body::Bytes::from(
                    serde_json::to_vec(&Value::Object(out)).expect("JSON serializes"),
                ));
            }
        }
        let mut data = serde_json::from_slice::<Value>(&input).ok()?;
        let mut cache = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let prev_idx = cache.iter().rposition(|entry| entry.scope == scope);
        let terminal = scope
            .split('?')
            .next()
            .is_some_and(|path| path.ends_with("/term/list"));
        let fields = if terminal {
            vec![("sessions", "name"), ("pending", "record_id")]
        } else {
            vec![("sessions", "uid")]
        };
        let mut rows = BTreeMap::new();
        for (name, field) in fields {
            let prev_rows = prev_idx.and_then(|i| cache[i].rows.get(name));
            let Some(list) = Rows::new(&data[name], field, prev_rows) else {
                // Older nodes may omit optional arrays or row identities.
                return Some(axum::body::Bytes::from(
                    serde_json::to_vec(&data).expect("JSON serializes"),
                ));
            };
            rows.insert(name.to_owned(), list);
        }
        let same = prev_idx.is_some_and(|i| cache[i].rows == rows);
        let version = if same {
            cache[prev_idx.unwrap()].version.clone()
        } else {
            let mut bytes = [0_u8; 16];
            if getrandom::fill(&mut bytes).is_err() {
                return Some(axum::body::Bytes::from(
                    serde_json::to_vec(&data).expect("JSON serializes"),
                ));
            }
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        };
        let new_wire = CachedWire::build(input, &data, &rows, &version);
        let out = if same
            && known == version
            && let Some(wire) = &new_wire
        {
            wire.unchanged.clone()
        } else if let Some(base) = cache
            .iter()
            .find(|s| s.scope == scope && s.version == known)
        {
            let delta: BTreeMap<_, _> = rows
                .iter()
                .map(|(name, list)| (name.clone(), list.delta(&base.rows[name])))
                .collect();
            let unchanged = base.rows == rows;
            for name in rows.keys() {
                data.as_object_mut().unwrap().remove(name);
            }
            data["list_delta"] = json!({"base":known,"collections":delta});
            data["list_unchanged"] = json!(unchanged);
            data["list_version"] = json!(&version);
            axum::body::Bytes::from(serde_json::to_vec(&data).expect("JSON serializes"))
        } else if let Some(wire) = &new_wire {
            wire.complete.clone()
        } else {
            data["list_version"] = json!(&version);
            axum::body::Bytes::from(serde_json::to_vec(&data).expect("JSON serializes"))
        };
        if same {
            cache[prev_idx.unwrap()].wire = new_wire;
        } else {
            for entry in cache.iter_mut() {
                if entry.scope == scope {
                    entry.wire = None;
                }
            }
            cache.push_back(Snapshot {
                scope: scope.to_owned(),
                version,
                rows,
                wire: new_wire,
            });
            while cache.len() > REVISIONS {
                cache.pop_front();
            }
        }
        Some(out)
    }
}

/// A reconstructed hub→node list response.
pub enum Expanded {
    /// The complete response.
    Complete(Value),
    /// The node reported `list_unchanged` and every envelope field equals the
    /// baseline: the complete response is the baseline itself, so callers can
    /// keep sharing it and everything derived from it.
    Baseline,
}

/// Reconstruct a complete response, also used by hub→node reads. Apply against
/// the baseline captured when the request began, never a concurrent newer one.
pub fn expand(mut data: Value, baseline: Option<&Value>) -> Option<Expanded> {
    let Some(delta) = data.get("list_delta") else {
        return Some(Expanded::Complete(data));
    };
    let baseline = baseline?;
    if delta["base"] != baseline["list_version"] {
        return None;
    }
    if is_baseline(&data, baseline)? {
        return Some(Expanded::Baseline);
    }
    let mut arrays = BTreeMap::new();
    for (name, patch) in delta["collections"].as_object()? {
        arrays.insert(name.clone(), expand_rows(&baseline[name], patch)?);
    }
    let object = data.as_object_mut()?;
    object.remove("list_delta");
    object.remove("list_unchanged");
    object.extend(arrays);
    Some(Expanded::Complete(data))
}

/// Whether the complete response equals `baseline` without rebuilding it.
/// `None` exactly where `expand_rows` would reject the patch; `Some(false)`
/// leaves the decision to the ordinary expansion.
fn is_baseline(data: &Value, baseline: &Value) -> Option<bool> {
    if data["list_unchanged"] != true {
        return Some(false);
    }
    let (Some(data), Some(base)) = (data.as_object(), baseline.as_object()) else {
        return Some(false);
    };
    let collections = data["list_delta"]["collections"].as_object()?;
    for (name, patch) in collections {
        let field = patch["key"].as_str()?;
        let removed = patch["remove"].as_array()?;
        let upsert = patch["upsert"].as_array()?;
        if !removed.is_empty() || !upsert.is_empty() || patch.get("order").is_some() {
            return Some(false);
        }
        // An empty patch reproduces the baseline rows when each has a key.
        let rows = baseline[name].as_array()?;
        if !rows.iter().all(|row| row[field].is_string()) {
            return None;
        }
    }
    // The envelope fields of this read must equal the baseline as well.
    let envelope = |key: &String| key != "list_delta" && key != "list_unchanged";
    let same = data
        .iter()
        .filter(|(key, _)| envelope(key) && !collections.contains_key(*key))
        .all(|(key, value)| base.get(key) == Some(value))
        && base
            .keys()
            .all(|key| collections.contains_key(key) || (envelope(key) && data.contains_key(key)));
    Some(same)
}

fn expand_rows(baseline: &Value, patch: &Value) -> Option<Value> {
    let field = patch["key"].as_str()?;
    let baseline = baseline.as_array()?;
    let old: BTreeMap<_, &Value> = baseline
        .iter()
        .map(|row| Some((key(row, field)?, row)))
        .collect::<Option<_>>()?;
    let removed: BTreeSet<String> = patch["remove"]
        .as_array()?
        .iter()
        .map(|id| id.as_str().map(str::to_owned))
        .collect::<Option<_>>()?;
    let upsert = patch["upsert"].as_array()?;
    let replaced: BTreeSet<String> = upsert
        .iter()
        .map(|v| {
            v["id"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| key(&v["row"], field))
        })
        .collect::<Option<_>>()?;
    // Only retained rows are copied; replaced rows are cloned from `old` once.
    let mut rows: Vec<Value> = baseline
        .iter()
        .filter(|row| {
            key(row, field).is_some_and(|id| !removed.contains(&id) && !replaced.contains(&id))
        })
        .cloned()
        .collect();
    for item in upsert {
        let index = usize::try_from(item["index"].as_u64()?).ok()?;
        if index > rows.len() {
            return None;
        }
        let row = if let Some(id) = item["id"].as_str() {
            let mut row = (*old.get(id)?).clone();
            let object = row.as_object_mut()?;
            for field in item["unset"].as_array()? {
                object.remove(field.as_str()?);
            }
            object.extend(item["set"].as_object()?.clone());
            if let Some(agents) = item.get("agents") {
                row["agent_items"] = expand_rows(&row["agent_items"], agents)?;
            }
            row
        } else {
            item["row"].clone()
        };
        rows.insert(index, row);
    }
    if let Some(order) = patch.get("order") {
        let mut by_id: BTreeMap<_, _> = rows
            .into_iter()
            .map(|v| Some((key(&v, field)?, v)))
            .collect::<Option<_>>()?;
        rows = order
            .as_array()?
            .iter()
            .map(|id| by_id.remove(id.as_str()?))
            .collect::<Option<_>>()?;
        if !by_id.is_empty() {
            return None;
        }
    }
    Some(Value::Array(rows))
}

pub async fn middleware(State(store): State<Arc<Store>>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let selected = request.method() == Method::GET
        && matches!(
            path,
            "/sessions" | "/term/list" | "/api/sessions" | "/api/term/list"
        );
    let known = selected
        .then(|| {
            request
                .headers()
                .get(HEADER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        })
        .flatten();
    let Some(known) = known else {
        return next.run(request).await;
    };
    let scope = request.uri().to_string();
    let response = next.run(request).await;
    if !response.status().is_success() {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return Response::builder()
                .status(500)
                .body(Body::from(error.to_string()))
                .unwrap();
        }
    };
    if let Some(fast) = store.fast_reply(&scope, &known, &bytes) {
        parts.headers.remove(header::CONTENT_LENGTH);
        parts
            .headers
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        parts.headers.append(header::VARY, HEADER.parse().unwrap());
        return Response::from_parts(parts, Body::from(fast));
    }
    // Delta transport decodes and re-encodes whole lists, even when only one
    // field changed. Keep that CPU work off the HTTP/SSE/WS reactor. A cancelled
    // request retains its permit until the blocking closure actually finishes.
    let permit = store
        .jobs
        .clone()
        .acquire_owned()
        .await
        .expect("list worker pool is never closed");
    let input = bytes.clone();
    let transformed = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        store.reply_bytes(&scope, &known, input)
    })
    .await;
    let bytes = match transformed {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Response::from_parts(parts, Body::from(bytes)),
        Err(_) => {
            return Response::builder()
                .status(500)
                .body(Body::from("列表同步任务失败"))
                .unwrap();
        }
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    parts
        .headers
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    parts.headers.append(header::VARY, HEADER.parse().unwrap());
    Response::from_parts(parts, Body::from(bytes))
}
