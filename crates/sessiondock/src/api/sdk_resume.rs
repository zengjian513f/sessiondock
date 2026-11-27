//! Short-lived replay of the exact SSE frames sent to one browser subscription.
use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, header},
    response::Response,
};
use futures_util::StreamExt;
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const HISTORY_BYTES: usize = 1024 * 1024;
const HISTORY_EVENTS: usize = 256;
const GRACE: Duration = Duration::from_secs(45);

#[derive(Default)]
pub struct Registry(Mutex<HashMap<u128, Weak<Feed>>>);

struct Feed {
    scope: Scope,
    state: Mutex<Journal>,
}

#[derive(PartialEq)]
pub struct Scope(pub &'static str, pub String, pub String, pub bool);

struct Journal {
    seq: u64,
    bytes: usize,
    recent: VecDeque<Bytes>,
    client: mpsc::Sender<Bytes>,
    takeover: CancellationToken,
}

impl Registry {
    pub fn resume(&self, scope: &Scope, cursor: Option<&str>) -> Option<Response> {
        let (id, seq) = cursor?.split_once('.')?;
        let id = u128::from_str_radix(id, 16).ok()?;
        let seq = seq.parse::<u64>().ok()?;
        let feed = self.0.lock().unwrap().get(&id)?.upgrade()?;
        if &feed.scope != scope {
            return None;
        }
        let mut journal = feed.state.lock().unwrap();
        let missed = usize::try_from(journal.seq.checked_sub(seq)?).ok()?;
        if seq == 0 || missed > journal.recent.len() {
            return None;
        }
        let mut pending: Vec<_> = journal
            .recent
            .iter()
            .skip(journal.recent.len() - missed)
            .cloned()
            .collect();
        pending.push(Bytes::from_static(b"event: resumed\ndata: {}\n\n"));
        let (tx, rx) = mpsc::channel(2);
        journal.takeover.cancel();
        journal.takeover = CancellationToken::new();
        journal.client = tx;
        Some(response(pending, rx))
    }

    pub fn start(self: &Arc<Self>, scope: Scope, original: Response) -> Response {
        let mut random = [0; 16];
        if getrandom::fill(&mut random).is_err() {
            return original;
        }
        let id = u128::from_le_bytes(random);
        let (tx, rx) = mpsc::channel(2);
        let feed = Arc::new(Feed {
            scope,
            state: Mutex::new(Journal {
                seq: 0,
                bytes: 0,
                recent: VecDeque::new(),
                client: tx,
                takeover: CancellationToken::new(),
            }),
        });
        self.0.lock().unwrap().insert(id, Arc::downgrade(&feed));
        let registry = Arc::clone(self);
        let mut input = original.into_body().into_data_stream();
        tokio::spawn(async move {
            let mut idle_since = None;
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                let data = tokio::select! {
                    _ = tick.tick() => {
                        if feed.state.lock().unwrap().client.is_closed() {
                            if idle_since.get_or_insert_with(tokio::time::Instant::now).elapsed() >= GRACE { break; }
                        } else { idle_since = None; }
                        continue;
                    }
                    chunk = input.next() => match chunk {Some(Ok(data)) => data, _ => break},
                };
                // Axum Sse emits one complete event per body frame. Keep heartbeat
                // traffic out of the journal: it carries no application state.
                let (client, takeover, frame) = {
                    let mut j = feed.state.lock().unwrap();
                    let frame = if data.starts_with(b"event: sdk\n") {
                        j.seq += 1;
                        let mut tagged = format!("id: {id:032x}.{}\n", j.seq).into_bytes();
                        tagged.extend_from_slice(&data);
                        let frame = Bytes::from(tagged);
                        j.bytes += frame.len();
                        j.recent.push_back(frame.clone());
                        while j.bytes > HISTORY_BYTES || j.recent.len() > HISTORY_EVENTS {
                            j.bytes -= j.recent.pop_front().unwrap().len();
                        }
                        frame
                    } else {
                        data
                    };
                    (j.client.clone(), j.takeover.clone(), frame)
                };
                // Backpressure live clients even when a single event exceeds
                // the replay budget. Such an event still reaches the page.
                tokio::select! { _ = client.send(frame) => {}, _ = takeover.cancelled() => {} }
            }
            registry.0.lock().unwrap().remove(&id);
        });
        response(Vec::new(), rx)
    }
}

fn response(pending: Vec<Bytes>, mut rx: mpsc::Receiver<Bytes>) -> Response {
    let body = async_stream::stream! {
        for frame in pending {yield Ok::<_, Infallible>(frame);}
        while let Some(frame) = rx.recv().await {yield Ok(frame);}
    };
    let mut reply = Response::new(Body::from_stream(body));
    reply.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    reply
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    reply
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    reply
}
