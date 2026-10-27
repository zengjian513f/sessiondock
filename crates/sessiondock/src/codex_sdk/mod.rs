//! Codex SDK mode: one JSON-RPC connection from this node to the shared Codex
//! app-server daemon (`codex app-server --listen unix://`, the same server a
//! default `codex` TUI starts and reuses). Contract: docs/codex-sdk.md.
//!
//! The daemon owns model context, turns and approvals; this module only
//! multiplexes it for browsers: it resumes a thread once per connection,
//! keeps the in-flight turn and open server requests for late joiners, fans
//! notifications out per thread and returns browser answers to the exact
//! request id it received. Persisted history stays in the native rollout and
//! is read by the ordinary history model. Transport handlers live in
//! `api::codex_sdk`; nothing here parses HTTP.

mod actions;
mod capacity;
mod connection;
mod controls;
mod live;
mod observation;
mod queue;
mod receipts;
mod retention;
mod usage;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;

use connection::Connection;
pub use connection::RpcError;
use live::LiveThread;

const MODEL_CACHE: Duration = Duration::from_secs(300);

/// Where the app-server lives and whether this node may start it.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub socket: PathBuf,
    /// `CODEX_HOME` for `codex app-server daemon start`; `None` never starts.
    pub autostart_home: Option<PathBuf>,
    /// The socket was named by `SESSIONDOCK_CODEX_APP_SERVER`.
    pub explicit: bool,
}

/// How a SessionDock-launched Codex TUI joins the shared daemon (`codex
/// --remote …`), so a terminal and the SDK view drive one live thread
/// instead of two runtimes fighting over the rollout's writer lock.
#[derive(Clone, Debug)]
pub struct TuiRemote {
    /// Value of `--remote`: `unix://` (the default control socket of the
    /// TUI's own `CODEX_HOME`) or `unix://<path>` for an explicit socket.
    pub arg: String,
    pub socket: PathBuf,
    /// `CODEX_HOME` the daemon is started in; `None` never starts one.
    pub home: Option<PathBuf>,
}

impl TuiRemote {
    /// Make the daemon reachable before a launch: connect, else start it once
    /// (`codex app-server daemon start`, idempotent) and wait for the socket.
    /// Blocking and bounded; `false` means launch the TUI the old way rather
    /// than fail the launch. A TUI given `--remote` does not start the daemon
    /// itself and exits with a connection error when none runs.
    pub fn ensure(&self, codex: &std::path::Path) -> bool {
        #[cfg(unix)]
        {
            use std::{os::unix::net::UnixStream, process::Stdio};
            let reachable = || UnixStream::connect(&self.socket).is_ok();
            if reachable() {
                return true;
            }
            let Some(home) = &self.home else {
                return false;
            };
            let Ok(mut child) = std::process::Command::new(codex)
                .args(["app-server", "daemon", "start"])
                .env("CODEX_HOME", home)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                return false;
            };
            let deadline = Instant::now() + Duration::from_secs(30);
            let started = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status.success(),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break false;
                    }
                }
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            while started && !reachable() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            started && reachable()
        }
        #[cfg(not(unix))]
        {
            let _ = codex;
            false
        }
    }
}

impl Endpoint {
    /// `SESSIONDOCK_CODEX_APP_SERVER` wins (`off` disables the mode, any other
    /// value is an explicit socket that is never auto-started); otherwise the
    /// default control socket beside the Codex sessions root.
    pub fn from_config(
        explicit: Option<&str>,
        codex_root: Option<&std::path::Path>,
    ) -> Option<Self> {
        if !cfg!(unix) {
            return None;
        }
        match explicit {
            Some("off") => None,
            Some(value) => Some(Self {
                socket: PathBuf::from(value.strip_prefix("unix://").unwrap_or(value)),
                autostart_home: None,
                explicit: true,
            }),
            None => {
                let home = codex_root?.parent()?.to_path_buf();
                Some(Self {
                    socket: home
                        .join("app-server-control")
                        .join("app-server-control.sock"),
                    autostart_home: Some(home),
                    explicit: false,
                })
            }
        }
    }
}

impl Endpoint {
    /// The TUI's view of this endpoint. A default endpoint applies only when
    /// it is the `CODEX_HOME` the CLI itself resolves (`$CODEX_HOME`, else
    /// `~/.codex`): `--remote unix://` finds its socket there, so a different
    /// configured sessions root (an isolated or test home) must not be joined.
    pub fn tui_remote(&self) -> Option<TuiRemote> {
        if self.explicit {
            return Some(TuiRemote {
                arg: format!("unix://{}", self.socket.display()),
                socket: self.socket.clone(),
                home: None,
            });
        }
        let home = self.autostart_home.as_ref()?;
        let own = std::env::var_os("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))?;
        let same = match (home.canonicalize(), own.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => home == &own,
        };
        same.then(|| TuiRemote {
            arg: "unix://".to_string(),
            socket: self.socket.clone(),
            home: Some(home.clone()),
        })
    }
}

pub struct CodexSdk {
    inner: Arc<Inner>,
}

type CreateReceipt = watch::Receiver<Option<Result<Value, RpcError>>>;

pub(crate) struct Inner {
    endpoint: Endpoint,
    connection: tokio::sync::Mutex<Option<Arc<Connection>>>,
    threads: Mutex<HashMap<String, Arc<LiveThread>>>,
    stopped: Mutex<HashSet<String>>,
    models: Mutex<Option<(Instant, Value)>>,
    creates: Mutex<HashMap<String, CreateReceipt>>,
    receipts: receipts::Receipts,
    retention: retention::Retention,
    observation: tokio::sync::Mutex<observation::Sample>,
    shutdown: CancellationToken,
    queue: queue::Queue,
}

/// What a page receives when it attaches to a thread.
pub struct Attached {
    pub snapshot: Value,
    pub events: broadcast::Receiver<Arc<str>>,
    pub guard: SubscriberGuard,
}

/// Counts attached pages without changing native runtime ownership.
pub struct SubscriberGuard {
    thread: Arc<LiveThread>,
}

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        self.thread.detach();
    }
}

impl CodexSdk {
    /// A live subscription proves this exact thread belongs to our shared
    /// daemon. A terminal joining the same socket adds a client, not a writer.
    pub async fn shares_thread(&self, thread_id: &str, socket: &std::path::Path) -> bool {
        if self.inner.endpoint.socket != socket {
            return false;
        }
        let slot = self.inner.connection.lock().await;
        let Some(connection) = slot.as_ref().filter(|connection| !connection.is_closed()) else {
            return false;
        };
        self.inner
            .threads
            .lock()
            .expect("threads")
            .get(thread_id)
            .is_some_and(|thread| thread.loaded_on() == Some(connection.generation()))
    }

    /// Threads actually loaded on this bridge's current live connection.
    /// Selecting an SDK view or a failed resume is not ownership evidence.
    pub async fn loaded_threads(&self) -> Vec<String> {
        let slot = self.inner.connection.lock().await;
        let Some(connection) = slot.as_ref().filter(|connection| !connection.is_closed()) else {
            return Vec::new();
        };
        self.inner
            .threads
            .lock()
            .expect("threads")
            .iter()
            .filter(|(_, thread)| thread.loaded_on() == Some(connection.generation()))
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn new(
        endpoint: Endpoint,
        shutdown: CancellationToken,
        state_dir: Option<PathBuf>,
    ) -> std::io::Result<Arc<Self>> {
        let queue = queue::Queue::open(state_dir.clone())?;
        let receipts = receipts::Receipts::open(state_dir.clone())?;
        let retention = retention::Retention::open(state_dir, &endpoint.socket)?;
        let sdk = Arc::new(Self {
            inner: Arc::new(Inner {
                endpoint,
                queue,
                connection: tokio::sync::Mutex::new(None),
                threads: Mutex::new(HashMap::new()),
                stopped: Mutex::new(HashSet::new()),
                models: Mutex::new(None),
                creates: Mutex::new(HashMap::new()),
                receipts,
                retention,
                observation: tokio::sync::Mutex::new(observation::Sample::default()),
                shutdown,
            }),
        });
        let inner = sdk.inner.clone();
        tokio::spawn(async move { inner.restore_subscriptions().await });
        let inner = sdk.inner.clone();
        tokio::spawn(async move { inner.run_queue().await });
        let inner = sdk.inner.clone();
        tokio::spawn(async move { inner.run_capacity_retries().await });
        Ok(sdk)
    }

    /// Keep an explicit create attempt alive across HTTP disconnects. Retrying
    /// its request id returns the same result, including uncertain failures.
    pub async fn create(&self, request_id: &str, params: Value) -> Result<Value, RpcError> {
        if let Some(result) = self.inner.receipts.completed(request_id) {
            return Ok(result);
        }
        let mut result = {
            let mut creates = self.inner.creates.lock().expect("creates");
            creates
                .entry(request_id.to_owned())
                .or_insert_with(|| {
                    let (send, receive) = watch::channel(None);
                    let inner = self.inner.clone();
                    let request_id = request_id.to_owned();
                    tokio::spawn(async move {
                        let result = inner.start(&request_id, params).await;
                        send.send_replace(Some(result));
                    });
                    receive
                })
                .clone()
        };
        result
            .wait_for(Option::is_some)
            .await
            .map_err(|_| RpcError::Disconnected)?;
        let completed = result.borrow().as_ref().expect("completed create").clone();
        if matches!(
            completed,
            Err(RpcError::Unavailable(_) | RpcError::Remote { .. })
        ) {
            let mut creates = self.inner.creates.lock().expect("creates");
            if creates
                .get(request_id)
                .is_some_and(|current| current.same_channel(&result))
            {
                creates.remove(request_id);
            }
        }
        completed
    }

    /// Start receipts only, never invented native files or history. A native
    /// rollout takes over this identity as soon as the daemon writes it.
    /// Creation receipts bridge the interval between first native write and
    /// inventory discovery without dropping the running session from /live.
    pub fn live_session_uids(&self, loaded: &[String]) -> Vec<String> {
        self.inner.receipts.live_uids(loaded)
    }

    pub fn pending(&self) -> Result<Vec<Value>, RpcError> {
        self.inner.receipts.pending()
    }

    pub fn pending_thread(&self, uid: &str) -> Option<String> {
        self.inner.receipts.thread(uid)
    }

    pub fn pending_session(&self, uid: &str) -> Option<Value> {
        self.inner.receipts.session(uid)
    }

    pub async fn discard(&self, uid: &str) -> Result<(), RpcError> {
        let Some(id) = self.pending_thread(uid) else {
            return Ok(());
        };
        let lock = self.inner.queue.lock(&id);
        let _dispatch = lock.lock().await;
        self.inner.retention.set(&id, false)?;
        let connection = self.inner.connection().await?;
        connection
            .request("thread/unsubscribe", json!({"threadId":id}))
            .await?;
        self.inner.receipts.discard(uid)?;
        self.inner.threads.lock().expect("threads").remove(&id);
        Ok(())
    }

    /// Resume `thread_id` on the shared connection (once per connection) and
    /// return the thread metadata the daemon reported.
    pub async fn open(&self, thread_id: &str) -> Result<Value, RpcError> {
        let thread = self.inner.ensure(thread_id).await?;
        self.inner
            .stopped
            .lock()
            .expect("stopped")
            .remove(thread_id);
        Ok(thread.summary())
    }

    /// Stop just this thread, preserving the shared daemon and other threads.
    pub async fn stop_thread(&self, thread_id: &str) -> Result<(), RpcError> {
        let lock = self.inner.queue.lock(thread_id);
        let _dispatch = lock.lock().await;
        self.inner.retention.set(thread_id, false)?;
        // Stopping must not resume or parse history. The latest rollout may
        // already be in trash while the daemon still reports the thread loaded.
        let connection = self.inner.connection_mode(false).await?;
        // The native empty-turn interrupt cancels the exact loaded thread's
        // startup/current work without requiring a recovered turn or history.
        let interrupted = connection
            .request("turn/interrupt", json!({"threadId":thread_id,"turnId":""}))
            .await;
        let result = connection
            .request("thread/unsubscribe", json!({"threadId":thread_id}))
            .await?;
        if result["status"] != "notLoaded" {
            interrupted?;
        }
        self.inner
            .stopped
            .lock()
            .expect("stopped")
            .insert(thread_id.to_owned());
        if let Some(thread) = self
            .inner
            .threads
            .lock()
            .expect("threads")
            .remove(thread_id)
        {
            thread.apply("bridge/stopped", json!({}));
        }
        Ok(())
    }

    /// Snapshot plus the event stream from the same instant: no gap, no
    /// duplicate between the two.
    pub async fn attach(&self, thread_id: &str) -> Result<Attached, RpcError> {
        let thread = self.inner.ensure(thread_id).await?;
        let (snapshot, events) = thread.attach();
        Ok(Attached {
            snapshot,
            events,
            guard: SubscriberGuard { thread },
        })
    }

    pub async fn interrupt(&self, thread_id: &str) -> Result<Value, RpcError> {
        let lock = self.inner.queue.lock(thread_id);
        let _dispatch = lock.lock().await;
        let thread = self.inner.ensure(thread_id).await?;
        let cancelled = thread.cancel_capacity_retry();
        let Some(turn_id) = thread.active_turn() else {
            if cancelled {
                return Ok(json!({"retry_cancelled":true}));
            }
            return Err(RpcError::NoActiveTurn);
        };
        let connection = self.inner.connection().await?;
        connection
            .request(
                "turn/interrupt",
                json!({"threadId": thread_id, "turnId": turn_id}),
            )
            .await?;
        Ok(json!({"turn_id": turn_id}))
    }

    /// Answer one open server request of this thread with the browser's
    /// result. The key must still be open on the current connection.
    pub async fn respond(&self, thread_id: &str, key: &str, result: Value) -> Result<(), RpcError> {
        let thread = self.inner.ensure(thread_id).await?;
        let connection = self.inner.connection().await?;
        let id = thread
            .request_id(key, connection.generation())
            .ok_or(RpcError::UnknownRequest)?;
        connection.reply(id, result)
    }

    /// `account/rateLimits/read`: the 5 h / weekly windows, credits and the
    /// earned reset cards. Account-wide, so it needs no thread.
    pub async fn rate_limits(&self) -> Result<Value, RpcError> {
        let connection = self.inner.connection().await?;
        connection
            .request("account/rateLimits/read", json!({}))
            .await
    }

    /// `account/rateLimitResetCredit/consume` for one chosen card. The caller's
    /// `idempotency_key` makes a retried redemption of the same attempt safe.
    pub async fn consume_reset(
        &self,
        credit_id: Option<&str>,
        idempotency_key: &str,
    ) -> Result<Value, RpcError> {
        let connection = self.inner.connection().await?;
        let mut params = json!({"idempotencyKey": idempotency_key});
        if let Some(id) = credit_id.filter(|id| !id.is_empty()) {
            params["creditId"] = json!(id);
        }
        connection
            .request("account/rateLimitResetCredit/consume", params)
            .await
    }

    pub async fn models(&self) -> Result<Value, RpcError> {
        if let Some((at, value)) = self.inner.models.lock().expect("models").clone()
            && at.elapsed() < MODEL_CACHE
        {
            return Ok(value);
        }
        let connection = self.inner.connection().await?;
        let value = connection.request("model/list", json!({})).await?;
        *self.inner.models.lock().expect("models") = Some((Instant::now(), value.clone()));
        Ok(value)
    }
}

impl Inner {
    async fn start(self: &Arc<Self>, request_id: &str, params: Value) -> Result<Value, RpcError> {
        let connection = self.connection().await?;
        let result = connection.request("thread/start", params).await?;
        self.adopt_result(connection.generation(), result, Some(request_id))
    }

    fn adopt_result(
        &self,
        generation: u64,
        result: Value,
        request_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        let id = result["thread"]["id"]
            .as_str()
            .ok_or(RpcError::Disconnected)?;
        let thread = {
            let mut threads = self.threads.lock().expect("threads");
            threads
                .entry(id.to_owned())
                .or_insert_with(|| LiveThread::new(id))
                .clone()
        };
        self.retention.set(id, true)?;
        thread.loaded(generation, result.clone());
        if let Some(path) = result["thread"]["path"].as_str() {
            let uid = crate::sessions::uid_for("codex", std::path::Path::new(path));
            let created = chrono::DateTime::from_timestamp(
                result["thread"]["createdAt"].as_i64().unwrap_or(0),
                0,
            )
            .unwrap_or_default()
            .to_rfc3339();
            let row = json!({"uid":uid,"source":"codex","sid":id,"title":"新建 Codex 会话",
                "cwd":result["thread"]["cwd"],"path":path,"created":created,"updated":created,
                "model":result["model"],"size":0,"agent_items":[],"sdk_pending":true});
            self.receipts.insert(request_id, result.clone(), row)?;
        }
        Ok(result)
    }

    async fn connection(self: &Arc<Self>) -> Result<Arc<Connection>, RpcError> {
        self.connection_mode(true).await
    }

    async fn connection_mode(
        self: &Arc<Self>,
        autostart: bool,
    ) -> Result<Arc<Connection>, RpcError> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref()
            && !connection.is_closed()
        {
            return Ok(connection.clone());
        }
        let generation = slot.as_ref().map_or(1, |old| old.generation() + 1);
        let mut endpoint = self.endpoint.clone();
        if !autostart {
            endpoint.autostart_home = None;
        }
        let connection = Connection::open(self.clone(), &endpoint, generation).await?;
        *slot = Some(connection.clone());
        Ok(connection)
    }

    async fn ensure(self: &Arc<Self>, thread_id: &str) -> Result<Arc<LiveThread>, RpcError> {
        let connection = self.connection().await?;
        self.ensure_on(&connection, thread_id).await
    }

    async fn ensure_on(
        self: &Arc<Self>,
        connection: &Arc<Connection>,
        thread_id: &str,
    ) -> Result<Arc<LiveThread>, RpcError> {
        let thread = {
            let mut threads = self.threads.lock().expect("threads");
            threads
                .entry(thread_id.to_string())
                .or_insert_with(|| LiveThread::new(thread_id))
                .clone()
        };
        let lock = thread.clone();
        let _resume = lock.resume_lock.lock().await;
        if thread.loaded_on() == Some(connection.generation()) {
            self.retention.set(thread_id, true)?;
            self.recover_active_turn(connection, &thread).await?;
            return Ok(thread);
        }
        match connection
            .request(
                "thread/resume",
                json!({"threadId": thread_id, "excludeTurns": true}),
            )
            .await
        {
            Ok(result) => {
                if let Some(path) = result["thread"]["path"].as_str() {
                    let path = PathBuf::from(path);
                    if let Ok(Some(usage)) =
                        tokio::task::spawn_blocking(move || usage::latest(&path)).await
                    {
                        thread.seed_usage(usage);
                    }
                }
                self.retention.set(thread_id, true)?;
                thread.loaded(connection.generation(), result);
                self.recover_active_turn(connection, &thread).await?;
                self.recover_receipts(connection, &thread).await?;
                self.queue_event(&thread);
                Ok(thread)
            }
            Err(error) => {
                let mut threads = self.threads.lock().expect("threads");
                if threads
                    .get(thread_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &thread))
                    && thread.loaded_on().is_none()
                {
                    threads.remove(thread_id);
                }
                Err(error)
            }
        }
    }

    async fn recover_receipts(
        &self,
        connection: &Arc<Connection>,
        thread: &LiveThread,
    ) -> Result<(), RpcError> {
        // The native process can consume an input while this service is down,
        // including in a turn that has already finished by the time we resume.
        let accepted = self.queue.accepted(thread.id());
        let mut turns: std::collections::HashSet<String> = accepted
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["turn_id"].as_str().map(str::to_owned))
            .collect();
        let mut cursor = Value::Null;
        while !turns.is_empty() {
            let page = connection
                .request(
                    "thread/turns/list",
                    json!({
                        "threadId":thread.id(), "sortDirection":"desc", "limit":1,
                        "itemsView":"full", "cursor":cursor,
                    }),
                )
                .await?;
            for turn in page["data"].as_array().into_iter().flatten() {
                let id = turn["id"].as_str().unwrap_or_default();
                if turns.remove(id) {
                    for item in turn["items"].as_array().into_iter().flatten() {
                        self.queue.consumed(thread.id(), id, item)?;
                    }
                }
            }
            cursor = page["nextCursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        Ok(())
    }

    async fn recover_active_turn(
        &self,
        connection: &Arc<Connection>,
        thread: &LiveThread,
    ) -> Result<(), RpcError> {
        if let Some(revision) = thread.missing_active_turn() {
            // Resume deliberately excludes history. Read only the newest turn
            // to recover both its expectedTurnId and the items written before
            // this bridge attached. The browser replaces that native turn's
            // history with these items; summary view omits its replies/tools.
            let turns = connection
                .request(
                    "thread/turns/list",
                    json!({
                        "threadId":thread.id(), "sortDirection":"desc", "limit":1,
                        "itemsView":"full",
                    }),
                )
                .await?;
            if let Some(turn) = turns["data"].as_array().and_then(|turns| turns.first()) {
                thread.recover_active_turn(revision, turn);
                for item in turn["items"].as_array().into_iter().flatten() {
                    self.queue.consumed(
                        thread.id(),
                        turn["id"].as_str().unwrap_or_default(),
                        item,
                    )?;
                }
                self.queue_event(thread);
            }
        }
        Ok(())
    }

    /// A notification from the daemon: route by `threadId`, apply to the
    /// live state and fan out under the same lock.
    pub(crate) fn notify(&self, method: &str, params: Value) {
        let Some(thread_id) = thread_of(&params) else {
            // Account-wide limit changes reach every attached page.
            if method == "account/rateLimits/updated" {
                let threads: Vec<_> = self
                    .threads
                    .lock()
                    .expect("threads")
                    .values()
                    .cloned()
                    .collect();
                for thread in threads {
                    thread.apply(method, params.clone());
                }
            }
            return;
        };
        if matches!(
            method,
            "thread/closed" | "thread/archived" | "thread/deleted"
        ) {
            if let Err(error) = self.retention.set(&thread_id, false) {
                eprintln!("Codex subscription persistence: {error}");
            }
            if let Some(thread) = self.threads.lock().expect("threads").remove(&thread_id) {
                thread.reset(method);
            }
            return;
        }
        let thread = self
            .threads
            .lock()
            .expect("threads")
            .get(&thread_id)
            .cloned();
        if let Some(thread) = thread {
            thread.apply(method, params.clone());
            if matches!(method, "item/started" | "item/completed")
                && params["item"]["type"] == "userMessage"
            {
                if self
                    .queue
                    .consumed(
                        &thread_id,
                        params["turnId"].as_str().unwrap_or_default(),
                        &params["item"],
                    )
                    .is_ok()
                {
                    self.queue_event(&thread);
                }
            }
        }
    }

    /// A server-initiated request. Unroutable ones are refused at once so
    /// the daemon never waits for an answer no page can give.
    pub(crate) fn server_request(
        &self,
        connection: &Arc<Connection>,
        id: Value,
        method: &str,
        params: Value,
    ) {
        let thread = thread_of(&params).and_then(|thread_id| {
            self.threads
                .lock()
                .expect("threads")
                .get(&thread_id)
                .cloned()
        });
        let supported = matches!(
            method,
            "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/tool/requestUserInput"
                | "item/permissions/requestApproval"
                | "mcpServer/elicitation/request"
        );
        match thread {
            Some(thread) if supported => {
                thread.request(connection.generation(), id, method, params)
            }
            _ => connection.refuse(id, method),
        }
    }

    /// The connection ended: every attached page gets `reset` and every
    /// thread is dropped, so the next attach resumes on a new connection.
    pub(crate) fn disconnected(&self, generation: u64, reason: &str) {
        let threads: Vec<_> = {
            let mut threads = self.threads.lock().expect("threads");
            let gone: Vec<String> = threads
                .iter()
                .filter(|(_, thread)| thread.loaded_on().is_none_or(|g| g <= generation))
                .map(|(id, _)| id.clone())
                .collect();
            gone.into_iter()
                .filter_map(|id| threads.remove(&id))
                .collect()
        };
        for thread in threads {
            thread.reset(reason);
        }
    }
}

fn thread_of(params: &Value) -> Option<String> {
    params["threadId"]
        .as_str()
        .or_else(|| params["thread"]["id"].as_str())
        .map(str::to_string)
}
