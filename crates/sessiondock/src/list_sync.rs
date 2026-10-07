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

struct Snapshot {
    scope: String,
    version: String,
    rows: BTreeMap<String, Rows>,
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
}

impl Store {
    pub fn reply(&self, scope: &str, known: &str, mut data: Value) -> Value {
        let mut cache = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let previous = cache.iter().rev().find(|entry| entry.scope == scope);
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
            let Some(list) = Rows::new(&data[name], field, previous.and_then(|s| s.rows.get(name)))
            else {
                return data; // Older nodes may omit optional arrays or row identities.
            };
            rows.insert(name.to_owned(), list);
        }
        let same = previous.is_some_and(|s| s.rows == rows);
        let version = if same {
            previous.unwrap().version.clone()
        } else {
            let mut bytes = [0_u8; 16];
            if getrandom::fill(&mut bytes).is_err() {
                return data;
            }
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        };
        if let Some(base) = cache
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
        }
        data["list_version"] = json!(version);
        if !same {
            cache.push_back(Snapshot {
                scope: scope.to_owned(),
                version,
                rows,
            });
            while cache.len() > REVISIONS {
                cache.pop_front();
            }
        }
        data
    }
}

/// Reconstruct a complete response, also used by hub→node reads. Apply against
/// the baseline captured when the request began, never a concurrent newer one.
pub fn expand(mut data: Value, baseline: Option<&Value>) -> Option<Value> {
    let Some(delta) = data.get("list_delta") else {
        return Some(data);
    };
    let baseline = baseline?;
    if delta["base"] != baseline["list_version"] {
        return None;
    }
    let mut arrays = BTreeMap::new();
    for (name, patch) in delta["collections"].as_object()? {
        arrays.insert(name.clone(), expand_rows(&baseline[name], patch)?);
    }
    let object = data.as_object_mut()?;
    object.remove("list_delta");
    object.remove("list_unchanged");
    object.extend(arrays);
    Some(data)
}

fn expand_rows(baseline: &Value, patch: &Value) -> Option<Value> {
    let field = patch["key"].as_str()?;
    let mut rows: Vec<Value> = baseline.as_array()?.clone();
    let old: BTreeMap<_, _> = rows
        .iter()
        .map(|row| Some((key(row, field)?, row.clone())))
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
    rows.retain(|row| {
        key(row, field).is_some_and(|id| !removed.contains(&id) && !replaced.contains(&id))
    });
    for item in upsert {
        let index = usize::try_from(item["index"].as_u64()?).ok()?;
        if index > rows.len() {
            return None;
        }
        let row = if let Some(id) = item["id"].as_str() {
            let mut row = old.get(id)?.clone();
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
        let data = serde_json::from_slice::<Value>(&input).ok()?;
        let data = store.reply(&scope, &known, data);
        Some(serde_json::to_vec(&data).expect("JSON serializes"))
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
