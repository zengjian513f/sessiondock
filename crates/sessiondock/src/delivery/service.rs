//! Bounded asynchronous access to an existing isolated delivery ledger.
//! Only display/diagnostic reads are exposed; opening performs engine recovery.

use std::{path::PathBuf, sync::Arc};

use serde::Serialize;
use tokio::sync::{Semaphore, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use super::{
    claude,
    engine::{self, DeliveryEngine, Provider},
};

// Also bound concurrent blocking opens process-wide, including opens whose
// awaiting caller disappears. Production performs only a couple of opens at
// startup; the bound keeps parallel opens (e.g. across many tests, or a
// delivery + lifecycle open) from all blocking at once.
static OPEN_WORKERS: Semaphore = Semaphore::const_new(8);

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Internal queue size. A full queue waits instead of rejecting input.
    pub capacity: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { capacity: 8 }
    }
}
impl Limits {
    fn validate(self) -> Result<Self, Error> {
        if self.capacity == 0 {
            Err(Error::InvalidLimits)
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidLimits,
    Closed,
    WorkerFailed,
    Encoding,
    Engine(engine::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "delivery read service: {self:?}")
    }
}
impl std::error::Error for Error {}

/// Already encoded within the blocking worker and response byte budget.
/// No Debug implementation: outbox JSON intentionally contains submitted text.
pub struct EncodedJson {
    bytes: Vec<u8>,
}
impl EncodedJson {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

enum Query {
    CodexOutbox {
        uid: String,
        agent_id: Option<String>,
    },
    ClaudeOutbox(claude::Scope),
    Receipts {
        provider: Provider,
        offset: usize,
        limit: usize,
    },
    Logs {
        after_sequence: u64,
        limit: usize,
    },
    /// Executor hook (batch 31): one trusted closure with exclusive engine
    /// access on the same single blocking worker, under the same admission
    /// and shutdown rules as a read. It replies through its own channel; the
    /// empty JSON reply only carries the permit until the caller drops it.
    Exec(ExecJob),
}
type ExecJob = Box<dyn FnOnce(&mut DeliveryEngine) + Send + 'static>;
struct Request {
    query: Query,
    reply: oneshot::Sender<Result<EncodedJson, Error>>,
}

/// Share with Arc, not with a coordinator-owned clone. The coordinator never
/// owns this handle or its sender, so last-handle Drop can stop the worker.
pub struct DeliveryService {
    tx: mpsc::Sender<Request>,
    stop: CancellationToken,
    done: watch::Receiver<Option<Result<(), Error>>>,
}
impl DeliveryService {
    /// Opens an existing ledger, restoring both provider epochs before ready.
    /// Missing data never initializes an empty ledger. All disk work is off the
    /// reactor. Cancellation does not abort a started blocking open.
    pub async fn open(
        directory: PathBuf,
        limits: Limits,
        shutdown: CancellationToken,
    ) -> Result<Self, Error> {
        Self::open_inner(directory, limits, shutdown, Hooks::default()).await
    }

    async fn open_inner(
        directory: PathBuf,
        limits: Limits,
        shutdown: CancellationToken,
        hooks: Hooks,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        if shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        let permit = OPEN_WORKERS.acquire().await.map_err(|_| Error::Closed)?;
        let stop = shutdown.child_token();
        let opening_stop = stop.clone();
        let engine = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if opening_stop.is_cancelled() {
                return Err(Error::Closed);
            }
            let engine = DeliveryEngine::open(&directory).map_err(Error::Engine)?;
            if opening_stop.is_cancelled() {
                return Err(Error::Closed);
            }
            Ok(engine)
        })
        .await
        .map_err(|_| Error::WorkerFailed)??;
        // No await between obtaining the engine and transferring it to its owner.
        let (tx, rx) = mpsc::channel(limits.capacity);
        let (done_tx, done) = watch::channel(None);
        tokio::spawn(coordinate(engine, rx, stop.clone(), done_tx, hooks));
        Ok(Self { tx, stop, done })
    }

    /// Scope resolution belongs to the trusted caller; no native inventory or
    /// display-name matching occurs here. Codex child scopes remain unsupported.
    pub async fn codex_outbox(
        &self,
        uid: String,
        agent_id: Option<String>,
    ) -> Result<EncodedJson, Error> {
        self.request(Query::CodexOutbox { uid, agent_id }).await
    }
    pub async fn claude_outbox(&self, scope: claude::Scope) -> Result<EncodedJson, Error> {
        self.request(Query::ClaudeOutbox(scope)).await
    }
    pub async fn receipts(
        &self,
        provider: Provider,
        offset: usize,
        limit: usize,
    ) -> Result<EncodedJson, Error> {
        self.request(Query::Receipts {
            provider,
            offset,
            limit,
        })
        .await
    }
    pub async fn logs(&self, after_sequence: u64, limit: usize) -> Result<EncodedJson, Error> {
        self.request(Query::Logs {
            after_sequence,
            limit,
        })
        .await
    }
    /// Trusted in-process executor access (batch 31). The closure runs on the
    /// coordinator's blocking worker with exclusive `&mut DeliveryEngine`; it
    /// is serialized with every read, admitted through the same capacity, and
    /// refused after shutdown. Nothing here performs terminal or native I/O;
    /// the executor claims any returned `DispatchBatch` outside the closure.
    pub async fn with_engine<T, F>(&self, work: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&mut DeliveryEngine) -> T + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: ExecJob = Box::new(move |engine| {
            let _ = tx.send(work(engine));
        });
        let done = self.request(Query::Exec(job)).await?;
        drop(done);
        rx.await.map_err(|_| Error::WorkerFailed)
    }
    async fn request(&self, query: Query) -> Result<EncodedJson, Error> {
        let response = self.admit(query).await?;
        // Dropping this future drops only the response receiver. The request,
        // permit and engine belong to the coordinator/blocking worker already.
        response.await.map_err(|_| Error::WorkerFailed)?
    }
    async fn admit(
        &self,
        query: Query,
    ) -> Result<oneshot::Receiver<Result<EncodedJson, Error>>, Error> {
        if self.stop.is_cancelled() {
            return Err(Error::Closed);
        }
        let (reply, response) = oneshot::channel();
        self.tx
            .send(Request { query, reply })
            .await
            .map_err(|_| Error::Closed)?;
        Ok(response)
    }
    /// Closes admission, rejects reads that have not started, and waits for the
    /// active blocking read and the store's lock release. Safe to call repeatedly.
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.stop.cancel();
        let mut done = self.done.clone();
        loop {
            if let Some(result) = done.borrow_and_update().clone() {
                return result;
            }
            done.changed().await.map_err(|_| Error::WorkerFailed)?;
        }
    }
}
impl Drop for DeliveryService {
    fn drop(&mut self) {
        self.stop.cancel();
        // Drop cannot await; the independent coordinator still joins its active
        // blocking worker before releasing the engine and signaling completion.
    }
}

#[derive(Default)]
struct Hooks {
    #[cfg(all(test, unix))]
    read_pause: Option<Arc<tests::Pause>>,
}
impl Hooks {
    fn before_read(&self) {
        #[cfg(all(test, unix))]
        if let Some(pause) = &self.read_pause {
            pause.block();
        }
    }
}

async fn coordinate(
    engine: DeliveryEngine,
    mut rx: mpsc::Receiver<Request>,
    stop: CancellationToken,
    done: watch::Sender<Option<Result<(), Error>>>,
    hooks: Hooks,
) {
    let mut engine = Some(engine);
    let hooks = Arc::new(hooks);
    let status = loop {
        let request = tokio::select! {
            biased;
            _ = stop.cancelled() => break Ok(()),
            next = rx.recv() => match next { Some(request) => request, None => break Ok(()) },
        };
        let current = engine.take().expect("one joined worker owns the engine");
        let hooks = hooks.clone();
        let work_stop = stop.clone();
        let joined = tokio::task::spawn_blocking(move || {
            let Request { query, reply } = request;
            let mut engine = current;
            let result = if work_stop.is_cancelled() {
                Err(Error::Closed)
            } else {
                hooks.before_read();
                query_json(&mut engine, query)
            };
            (engine, reply, result)
        })
        .await;
        // Intentionally no shutdown select around this await: a started worker
        // retains its engine, lock and permit even when a response is discarded.
        match joined {
            Ok((current, reply, result)) => {
                engine = Some(current);
                let _ = reply.send(result);
            }
            Err(_) => break Err(Error::WorkerFailed),
        }
    };
    rx.close();
    while let Ok(request) = rx.try_recv() {
        let _ = request.reply.send(Err(Error::Closed));
    }
    // Dropping the engine is kept off the reactor.
    let status = if let Some(engine) = engine {
        match tokio::task::spawn_blocking(move || drop(engine)).await {
            Ok(()) => status,
            Err(_) => Err(Error::WorkerFailed),
        }
    } else {
        status
    };
    let _ = done.send(Some(status));
}

fn query_json(engine: &mut DeliveryEngine, query: Query) -> Result<EncodedJson, Error> {
    match query {
        Query::CodexOutbox { uid, agent_id } => encode(
            &engine
                .codex_outbox(&uid, agent_id.as_deref())
                .map_err(Error::Engine)?,
        ),
        Query::ClaudeOutbox(scope) => encode(&engine.claude_outbox(&scope).map_err(Error::Engine)?),
        Query::Receipts {
            provider,
            offset,
            limit,
        } => encode(
            &engine
                .receipts(provider, offset, limit)
                .map_err(Error::Engine)?,
        ),
        Query::Logs {
            after_sequence,
            limit,
        } => encode(&engine.logs(after_sequence, limit).map_err(Error::Engine)?),
        Query::Exec(job) => {
            job(engine);
            Ok(EncodedJson { bytes: Vec::new() })
        }
    }
}

fn encode(value: &impl Serialize) -> Result<EncodedJson, Error> {
    serde_json::to_vec(value)
        .map(|bytes| EncodedJson { bytes })
        .map_err(|_| Error::Encoding)
}

#[cfg(all(test, unix))]
mod tests;
