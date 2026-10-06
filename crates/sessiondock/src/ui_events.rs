//! Shared, subscriber-owned UI invalidations. No conversation bodies cross
//! this stream; detailed reads remain demand-driven.
use axum::response::{
    IntoResponse, Response, Sse,
    sse::{Event, KeepAlive},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    convert::Infallible,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::broadcast;

pub struct Snapshot {
    sessions: Value,
    live: Value,
    term: Value,
    cursors: BTreeMap<(String, String), Value>,
    /// Hub only: each node's draft epoch (`/api/live` `draft_epochs`).
    drafts: Value,
}

// Observation timestamps and diagnostics do not represent a UI change.
fn stable(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for key in [
                "sig",
                "built_at",
                "observed_at",
                "cache",
                "scan",
                "last_seen",
                "checked_at",
                "strikes",
                "failed_since",
                "offline_since",
                "draft_epoch",
            ] {
                object.remove(key);
            }
            for value in object.values_mut() {
                stable(value);
            }
        }
        Value::Array(items) => {
            for value in items {
                stable(value);
            }
        }
        _ => {}
    }
}
fn row_metadata(row: &mut Value) {
    if let Some(object) = row.as_object_mut() {
        for key in [
            "cursor",
            "size",
            "mtime",
            "updated",
            "updated_at",
            "last_active",
            "last_active_at",
            "message_count",
        ] {
            object.remove(key);
        }
        if let Some(Value::Array(items)) = object.get_mut("agent_items") {
            for item in items.iter_mut() {
                row_metadata(item);
            }
            items.sort_by_key(|item| item["id"].as_str().unwrap_or_default().to_owned());
        }
    }
}
impl Snapshot {
    pub fn new(mut sessions: Value, mut live: Value, mut term: Value) -> Self {
        let drafts = live
            .as_object_mut()
            .and_then(|live| live.remove("draft_epochs"))
            .unwrap_or(Value::Null);
        let mut cursors = BTreeMap::new();
        if let Some(rows) = sessions["sessions"].as_array_mut() {
            for row in rows.iter_mut() {
                let uid = row["uid"].as_str().unwrap_or_default().to_owned();
                if row["cursor"].is_object() {
                    cursors.insert(
                        (uid.clone(), String::new()),
                        json!({"uid":uid,"agent":null,"cursor":row["cursor"]}),
                    );
                }
                for item in row["agent_items"].as_array().into_iter().flatten() {
                    if item["cursor"].is_object() {
                        let agent = item["id"].as_str().unwrap_or_default().to_owned();
                        cursors.insert(
                            (uid.clone(), agent.clone()),
                            json!({"uid":uid,"agent":agent,"cursor":item["cursor"]}),
                        );
                    }
                }
                row_metadata(row);
            }
            rows.sort_by_key(|row| row["uid"].as_str().unwrap_or_default().to_owned());
        }
        for field in ["uids", "tmux_uids"] {
            if let Some(items) = live[field].as_array_mut() {
                items.sort_by_key(Value::to_string);
            }
        }
        for field in ["sessions", "pending"] {
            if let Some(items) = term[field].as_array_mut() {
                items.sort_by_key(|row| row["name"].as_str().unwrap_or_default().to_owned());
            }
        }
        stable(&mut sessions);
        stable(&mut live);
        stable(&mut term);
        Self {
            sessions,
            live,
            term,
            cursors,
            drafts,
        }
    }
    pub fn change(&self, previous: &Self) -> Option<Value> {
        let cursors: Vec<_> = self
            .cursors
            .iter()
            .filter(|(key, value)| previous.cursors.get(*key) != Some(*value))
            .map(|(_, value)| value.clone())
            .collect();
        let sessions = self.sessions != previous.sessions;
        let live = self.live != previous.live;
        let term = self.term != previous.term;
        // A node reporting a new draft epoch, or newly reachable, may hold a
        // draft another page changed; a node that went away carries none.
        let drafts = self.drafts.as_object().is_some_and(|now| {
            now.iter()
                .any(|(node, epoch)| previous.drafts.get(node) != Some(epoch))
        });
        (sessions || live || term || drafts || !cursors.is_empty()).then(|| {
            let mut change = json!({"sessions":sessions,"live":live,"term":term,"cursors":cursors});
            if drafts {
                change["drafts"] = json!(true);
            }
            change
        })
    }
}

#[derive(Default)]
pub struct EventBus {
    sender: Mutex<Option<broadcast::Sender<Value>>>,
    /// Draft changes so far; `/api/live` reports it so the Hub's observer
    /// can relay a node's draft change to Hub pages.
    draft_epoch: AtomicU64,
}
impl EventBus {
    /// Invalidate directory reads after a completed metadata write. Snapshot
    /// sampling alone can miss a change and its reversal between observations.
    pub fn publish_sessions(&self) {
        let sender_slot = self.sender.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sender) = sender_slot.as_ref() {
            let _ = sender.send(json!({"sessions":true}));
        }
    }

    /// A draft revision changed: open composers read their draft at once
    /// instead of waiting for the next input CHECK.
    pub fn publish_drafts(&self) {
        self.draft_epoch.fetch_add(1, Ordering::Relaxed);
        let sender_slot = self.sender.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sender) = sender_slot.as_ref() {
            let _ = sender.send(json!({"drafts":true}));
        }
    }

    pub fn draft_epoch(&self) -> u64 {
        self.draft_epoch.load(Ordering::Relaxed)
    }

    pub fn subscribe<F, Fut>(self: &Arc<Self>, observe: F) -> broadcast::Receiver<Value>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<Snapshot>> + Send + 'static,
    {
        let mut sender_slot = self.sender.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sender) = sender_slot.as_ref() {
            return sender.subscribe();
        }
        let (sender, receiver) = broadcast::channel(32);
        *sender_slot = Some(sender.clone());
        let bus = self.clone();
        tokio::spawn(async move {
            let mut previous = None;
            loop {
                // Check and remove under the subscribe lock so a new subscriber
                // cannot attach between the last-receiver check and removal.
                {
                    let mut sender_slot = bus.sender.lock().unwrap_or_else(|e| e.into_inner());
                    if sender.receiver_count() == 0 {
                        *sender_slot = None;
                        break;
                    }
                }
                if let Some(snapshot) = observe().await {
                    if let Some(old) = &previous {
                        if let Some(change) = snapshot.change(old) {
                            let _ = sender.send(change);
                        }
                    } else {
                        // Reconcile after the first baseline is captured too:
                        // a browser's initial HTTP reads may have preceded it.
                        let _ = sender.send(json!({"initial":true}));
                    }
                    previous = Some(snapshot);
                } else {
                    // Keep the connection, but release the browser to its
                    // compatibility polls until observation recovers.
                    previous = None;
                    let _ = sender.send(json!({"retry":true}));
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        receiver
    }
}
pub fn stream(
    mut receiver: broadcast::Receiver<Value>,
    shutdown: tokio_util::sync::CancellationToken,
) -> Response {
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(Event::default().event("change").data("{\"initial\":true}"));
        loop {
            let received = tokio::select! {
                _ = shutdown.cancelled() => break,
                received = receiver.recv() => received,
            };
            let value = match received {
                Ok(value) => value,
                Err(broadcast::error::RecvError::Lagged(_)) => json!({"initial":true}),
                Err(broadcast::error::RecvError::Closed) => break,
            };
            yield Ok(Event::default().event("change").data(value.to_string()));
        }
    };
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
        .into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
        .headers_mut()
        .insert("cache-control", "no-cache".parse().unwrap());
    response
}
