//! Codex executor tests: real engine/ledger/session store, a fake
//! terminal driver that plays the Codex TUI (a `›` composer with a model
//! footer, braille particle glyphs, a dim placeholder) and appends the real
//! rollout shape for every Enter (`turn_context`, `task_started`, the user
//! `response_item` with its turn ID, `user_message`, `task_complete`) unless
//! told to swallow it or omit the turn ID. No process is started.
#![cfg(unix)]

use super::*;
use crate::{
    delivery::{driver::ScreenCapture, engine::DeliveryEngine},
    sessions::{SessionRoots, SessionStore},
};
use ptyhost_client::{
    Association, AssociationState, BoundTarget, HostObservation, LaunchState, NativeBindingState,
    SessionSummary, Source,
};
use serde_json::json;
use std::{
    fs,
    io::Write,
    net::{IpAddr, Ipv4Addr},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const SID: &str = "7c1e4d2a-9b3f-4e5a-8d6c-1f2e3a4b5c6d";
const INSTANCE: &str = "synthetic-codex-instance-0001";
const NAME: &str = "sessiondock-codex-7c1e4d2a";
const FOOTER: &str = "gpt-5.6-luna low · /synthetic/codex-area";
const PLACEHOLDER: &str = "Ask Codex to do anything";

struct FakeState {
    buffer: String,
    no_composer: bool,
    pasted: Vec<String>,
    keys: Vec<Vec<&'static str>>,
    conflict: Option<IpAddr>,
    enter_ambiguous: bool,
    swallow: bool,
    no_turn_id: bool,
    duplicate_record: bool,
    lag: u64,
    record_path: PathBuf,
    turns: usize,
    pause_paste: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    pause_enter: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}

pub struct FakeDriver {
    state: Mutex<FakeState>,
}

impl FakeDriver {
    fn new(record_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FakeState {
                buffer: String::new(),
                no_composer: false,
                pasted: Vec::new(),
                keys: Vec::new(),
                conflict: None,
                enter_ambiguous: false,
                swallow: false,
                no_turn_id: false,
                duplicate_record: false,
                lag: 0,
                record_path,
                turns: 0,
                pause_paste: None,
                pause_enter: None,
            }),
        })
    }
    fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }
    /// The Codex 0.154 layout: transcript, a particle padding row painted in
    /// RGB colour, the `›` input row (dim placeholder when empty, particles
    /// in its blank cells), another particle row, then the model footer.
    fn screen(state: &FakeState) -> ScreenCapture {
        if state.no_composer {
            return ScreenCapture {
                text: "Would you like to run the following command?\n\n  1. Yes (y)\n  2. No (esc)\nPress enter to confirm or esc to cancel".into(),
                cursor: (0, 4),
                lag: Some(state.lag),
                dropped: Some(0),
                resets: Some(0),
                alt: false,
            };
        }
        let particles = "\x1b[38;2;90;90;90m⠁⠂⠄ ⠈⠐⠠\x1b[0m";
        let input = if state.buffer.is_empty() {
            format!("› \x1b[2m{PLACEHOLDER}\x1b[0m   \x1b[38;2;90;90;90m⠁⠂\x1b[0m")
        } else {
            format!("› {}", state.buffer)
        };
        let text = [
            "FAKE_CODEX_TUI".to_owned(),
            "> earlier prompt".to_owned(),
            String::new(),
            particles.to_owned(),
            input,
            particles.to_owned(),
            String::new(),
            FOOTER.to_owned(),
        ]
        .join("\n");
        ScreenCapture {
            text,
            cursor: ((2 + state.buffer.chars().count()) as u16, 4),
            lag: Some(state.lag),
            dropped: Some(0),
            resets: Some(0),
            alt: false,
        }
    }
    fn write_turn(state: &mut FakeState, text: &str) {
        state.turns += 1;
        let turn = format!("fake-turn-{}", state.turns);
        // The executor's timestamp check compares the record against the
        // Submit clock (now); a fixed literal turns into "earlier than the
        // send" once wall time passes it.
        let ts = chrono::DateTime::from_timestamp_millis(super::now_ms() as i64)
            .expect("current time")
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut user = json!({"type": "message", "role": "user",
            "content": [{"type": "input_text", "text": text}]});
        if !state.no_turn_id {
            user["internal_chat_message_metadata_passthrough"] = json!({"turn_id": turn});
        }
        let mut rows = vec![
            json!({"timestamp": ts, "type": "turn_context",
                "payload": {"model": "gpt-5.6-luna", "effort": "low", "turn_id": turn}}),
            json!({"timestamp": ts, "type": "event_msg",
                "payload": {"type": "task_started", "turn_id": turn}}),
            json!({"timestamp": ts, "type": "response_item", "payload": user.clone()}),
            json!({"timestamp": ts, "type": "event_msg",
                "payload": {"type": "user_message", "message": text}}),
        ];
        if state.duplicate_record {
            rows.push(json!({"timestamp": ts, "type": "response_item", "payload": user}));
        }
        rows.push(json!({"timestamp": ts, "type": "event_msg",
            "payload": {"type": "task_complete", "turn_id": turn, "duration_ms": 3}}));
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&state.record_path)
            .unwrap();
        for row in rows {
            writeln!(file, "{row}").unwrap();
        }
    }
}

impl TerminalDriver for FakeDriver {
    fn acquire<'a>(
        &'a self,
        target: &'a DeliveryTarget,
        _page: Option<&'a PageLease>,
    ) -> BoxFuture<'a, Result<LeaseHandle, DriverError>> {
        Box::pin(async move {
            if let Some(ip) = self.state().conflict {
                return Err(DriverError {
                    status: 409,
                    code: "terminal_ownership",
                    message: "终端控制权正由其他页面持有".into(),
                    ambiguous: false,
                    owner_ip: Some(ip),
                });
            }
            Ok(crate::delivery::driver::test_lease(
                &target.name,
                &target.uid,
                &target.instance_id,
            ))
        })
    }
    fn capture<'a>(
        &'a self,
        _lease: &'a LeaseHandle,
    ) -> BoxFuture<'a, Result<ScreenCapture, DriverError>> {
        Box::pin(async move { Ok(Self::screen(&self.state())) })
    }
    fn paste<'a>(
        &'a self,
        _lease: &'a LeaseHandle,
        text: &'a str,
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async move {
            let pause = self.state().pause_paste.take();
            if let Some((arrived, resume)) = pause {
                arrived.notify_one();
                resume.notified().await;
            }
            let mut state = self.state();
            state.pasted.push(text.to_owned());
            state.buffer.push_str(text);
            Ok(())
        })
    }
    fn keys<'a>(
        &'a self,
        _lease: &'a LeaseHandle,
        keys: &'a [&'static str],
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async move {
            let pause = if keys.contains(&"Enter") {
                self.state().pause_enter.take()
            } else {
                None
            };
            if let Some((arrived, resume)) = pause {
                arrived.notify_one();
                resume.notified().await;
            }
            let mut state = self.state();
            state.keys.push(keys.to_vec());
            for key in keys {
                match *key {
                    "C-u" => state.buffer.clear(),
                    "Enter" => {
                        let text = std::mem::take(&mut state.buffer);
                        if state.enter_ambiguous {
                            return Err(DriverError {
                                status: 504,
                                code: "terminal_input_ambiguous",
                                message: "no ack".into(),
                                ambiguous: true,
                                owner_ip: None,
                            });
                        }
                        if !state.swallow && !text.trim().is_empty() {
                            Self::write_turn(&mut state, text.trim());
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        })
    }
    fn release<'a>(&'a self, _lease: LeaseHandle) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

struct FakeResolver {
    target: DeliveryTarget,
}

impl TargetResolver for FakeResolver {
    fn resolve<'a>(&'a self, uid: &'a str) -> BoxFuture<'a, Result<DeliveryTarget, Failure>> {
        Box::pin(async move {
            if uid == self.target.uid {
                Ok(self.target.clone())
            } else {
                Err(unlinked())
            }
        })
    }
}

fn synthetic_target(uid: &str) -> DeliveryTarget {
    let observation = HostObservation {
        summary: SessionSummary {
            grid: false,
            name: NAME.into(),
            created: 1,
            attached: false,
            pid: 4343,
            host_pid: 4342,
            cwd: "/synthetic".into(),
            cmd: "fake".into(),
            cols: 80,
            rows: 24,
            owned: true,
            server: "ptyhost",
            backend: "ptyhost",
        },
        association: AssociationState::Declared(Association {
            source: Source::Codex,
            sid: Some(SID.into()),
            uid: None,
        }),
        instance_id: Some(INSTANCE.into()),
        exited: false,
        instance_guard_v1: true,
        launch: LaunchState::Missing,
        launch_guard_v1: false,
        native_binding: NativeBindingState::Unsupported,
    };
    let bound = BoundTarget::from_observation(&observation, Source::Codex, SID, uid).unwrap();
    DeliveryTarget {
        name: NAME.into(),
        uid: uid.into(),
        instance_id: INSTANCE.into(),
        bound: Arc::new(bound),
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    delivery_dir: PathBuf,
    root: PathBuf,
    record_path: PathBuf,
    uid: String,
    driver: Arc<FakeDriver>,
    shutdown: CancellationToken,
    executor: Option<Arc<DeliveryExecutor>>,
    service: Option<Arc<DeliveryService>>,
}

fn uid_of(path: &Path) -> String {
    use sha1::Digest;
    let digest = sha1::Sha1::digest(path.to_str().unwrap().as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("codex:{}", &hex[..16])
}

fn limits() -> ExecutorLimits {
    ExecutorLimits {
        workers: 2,
        confirm_timeout: Duration::from_millis(50),
        tracking_window: Duration::from_secs(3600),
        tick: Duration::from_secs(3600),
        redispatch_interval: Duration::ZERO,
        prepare_timeout: Duration::from_millis(400),
    }
}

impl Harness {
    async fn new() -> Self {
        Self::with_limits(limits()).await
    }
    async fn with_limits(limits: ExecutorLimits) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("codex");
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let record_path = sessions.join(format!("rollout-{SID}.jsonl"));
        let rows = [
            json!({"timestamp": "2026-09-12T10:00:00.000Z", "type": "session_meta",
                "payload": {"id": SID, "timestamp": "2026-09-12T10:00:00.000Z", "cwd": "/synthetic"}}),
            json!({"timestamp": "2026-09-12T10:00:01.000Z", "type": "response_item",
                "payload": {"type": "message", "role": "user", "turn_id": "seed-turn",
                "content": [{"type": "input_text", "text": "seed prompt"}]}}),
        ];
        fs::write(
            &record_path,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let delivery_dir = base.join("delivery");
        fs::create_dir_all(&delivery_dir).unwrap();
        fs::set_permissions(&delivery_dir, fs::Permissions::from_mode(0o700)).unwrap();
        drop(DeliveryEngine::initialize(&delivery_dir).unwrap());
        let uid = uid_of(&record_path);
        let driver = FakeDriver::new(record_path.clone());
        let mut harness = Self {
            _temp: temp,
            delivery_dir,
            root,
            record_path,
            uid,
            driver,
            shutdown: CancellationToken::new(),
            executor: None,
            service: None,
        };
        harness.open(limits).await;
        harness
    }
    async fn open(&mut self, limits: ExecutorLimits) {
        self.shutdown = CancellationToken::new();
        let service = Arc::new(
            DeliveryService::open(
                self.delivery_dir.clone(),
                Default::default(),
                self.shutdown.clone(),
            )
            .await
            .unwrap(),
        );
        let reader = Reader {
            store: Arc::new(SessionStore::new(SessionRoots {
                codex: Some(self.root.clone()),
                ..Default::default()
            })),
            workers: Arc::new(Semaphore::new(4)),
        };
        let resolver = Arc::new(FakeResolver {
            target: synthetic_target(&self.uid),
        });
        self.executor = Some(DeliveryExecutor::start(
            service.clone(),
            self.driver.clone(),
            resolver,
            reader,
            limits,
            self.shutdown.clone(),
        ));
        self.service = Some(service);
    }
    fn exec(&self) -> &DeliveryExecutor {
        self.executor.as_ref().unwrap()
    }
    fn svc(&self) -> &DeliveryService {
        self.service.as_ref().unwrap()
    }
    async fn restart(&mut self, limits: ExecutorLimits) {
        self.shutdown.cancel();
        self.svc().shutdown().await.unwrap();
        self.executor = None;
        self.service = None;
        self.driver = FakeDriver::new(self.record_path.clone());
        self.open(limits).await;
    }
    /// The session store rescans at most every 500 ms; tick until the
    /// receipt reaches `wanted` or five seconds pass.
    async fn settle(&self, id: &str, wanted: codex::State) -> codex::Receipt {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.exec().tick().await.unwrap();
            let receipt = self.receipt(id).await.unwrap();
            if receipt.state == wanted || Instant::now() >= deadline {
                return receipt;
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
    }
    async fn ticks(&self, count: usize) {
        for _ in 0..count {
            tokio::time::sleep(Duration::from_millis(120)).await;
            self.exec().tick().await.unwrap();
        }
    }
    fn request(&self, id: &str, text: &str, overwrite: &str) -> SendRequest {
        SendRequest {
            uid: self.uid.clone(),
            agent: String::new(),
            name: NAME.into(),
            text: text.into(),
            media: Vec::new(),
            request_id: id.into(),
            overwrite_draft: overwrite.into(),
            page_lease: None,
        }
    }
    async fn receipt(&self, id: &str) -> Option<codex::Receipt> {
        let id = id.to_owned();
        self.svc()
            .with_engine(move |engine| engine.codex_receipt(&id))
            .await
            .unwrap()
            .unwrap()
    }
    async fn outbox(&self) -> Value {
        self.exec().codex_outbox(&self.uid).await.unwrap()
    }
    fn rollout_lines(&self) -> Vec<Value> {
        fs::read_to_string(&self.record_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[tokio::test]
async fn codex_send_persists_injects_two_steps_and_confirms_with_operation_turn() {
    let harness = Harness::new().await;
    let reply = harness
        .exec()
        .send(harness.request("codex-req-0001", "hello codex reliable send", ""))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["ok"], true);
    assert_eq!(reply.body["item"]["id"], "codex-req-0001");
    assert_eq!(reply.body["item"]["state"], "failed");
    assert_eq!(reply.body["item"]["attempts"], 1);
    assert_eq!(reply.body["item"]["error"], "发送结果待核对；禁止自动重试");
    assert_eq!(reply.body["item"]["server"], true);
    assert!(reply.body["outbox_version"]["epoch"].is_string());
    {
        let state = harness.driver.state();
        assert_eq!(state.pasted, vec!["hello codex reliable send".to_owned()]);
        assert_eq!(state.keys, vec![vec!["Enter"]]);
    }
    let receipt = harness.receipt("codex-req-0001").await.unwrap();
    assert_eq!(receipt.state, codex::State::Uncertain);
    assert_eq!(receipt.attempts, 1);
    assert!(receipt.enter_operation.is_some());
    let confirmation = receipt.confirmation.clone().expect("fixed fence");
    assert_eq!(receipt.watch, Some(confirmation.clone()));
    assert_eq!(receipt.request.payload.target.host_instance, INSTANCE);
    assert_eq!(receipt.request.payload.target.session_id, SID);
    assert_eq!(
        receipt.request.payload.target.ownership_epoch,
        OWNERSHIP_EPOCH
    );
    // The fence was captured before the paste: it is the seed file's end.
    let seed_len = harness.rollout_lines()[..2]
        .iter()
        .map(|row| format!("{row}\n").len() as u64)
        .sum::<u64>();
    assert_eq!(confirmation.position, seed_len);

    let receipt = harness
        .settle("codex-req-0001", codex::State::Completed)
        .await;
    assert!(
        matches!(
            receipt.state,
            codex::State::Acknowledged | codex::State::Completed
        ),
        "{receipt:?}"
    );
    let accepted = receipt.accepted.clone().unwrap();
    assert_eq!(accepted.turn_id, "fake-turn-1");
    assert!(accepted.start >= confirmation.position);
    assert!(accepted.record_id.starts_with("codex-line:"));
    assert_eq!(
        receipt.association,
        Some(codex::Correlation::PossibleTextMatch)
    );
    assert_eq!(receipt.state, codex::State::Completed);
    assert_eq!(receipt.completion, Some(codex::Completion::Succeeded));
    assert!(
        harness.outbox().await["outbox"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let state = harness.driver.state();
    assert_eq!(state.pasted.len(), 1);
    assert_eq!(state.keys.len(), 1);
}

#[tokio::test]
async fn codex_same_request_id_replays_without_touching_the_terminal() {
    let harness = Harness::new().await;
    harness.driver.state().swallow = true;
    let first = harness
        .exec()
        .send(harness.request("codex-req-0002", "once only", ""))
        .await;
    assert_eq!(first.status, 200, "{}", first.body);
    let again = harness
        .exec()
        .send(harness.request("codex-req-0002", "once only", "ignored-token"))
        .await;
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(again.body["item"]["id"], "codex-req-0002");
    assert_eq!(again.body["item"]["state"], "failed");
    assert_eq!(again.body["item"]["attempts"], 1);
    assert_eq!(
        again.body["outbox_version"]["revision"],
        first.body["outbox_version"]["revision"]
    );
    let conflict = harness
        .exec()
        .send(harness.request("codex-req-0002", "different text", ""))
        .await;
    assert_eq!(conflict.status, 400);
    assert_eq!(conflict.body["code"], "request_conflict");
    assert_eq!(conflict.body["error"], "重复发送 ID 对应了不同消息");
    let state = harness.driver.state();
    assert_eq!(state.pasted.len(), 1);
    assert_eq!(state.keys.len(), 1);
}

#[tokio::test]
async fn codex_draft_conflict_needs_consent_then_clears_and_sends() {
    let harness = Harness::new().await;
    harness.driver.state().buffer = "old draft".into();
    let probe = harness.exec().draft_status(&harness.uid, NAME, None).await;
    assert_eq!(probe.status, 200, "{}", probe.body);
    assert_eq!(probe.body["draft_state"], "editing");
    assert_eq!(probe.body["draft_conflict"], true);
    let token = probe.body["draft_token"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 64);
    let refused = harness
        .exec()
        .send(harness.request("codex-req-0003", "new prompt", ""))
        .await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.body["draft_conflict"], true);
    assert_eq!(refused.body["draft_token"], token);
    assert!(refused.body["outbox"].as_array().unwrap().is_empty());
    assert!(
        harness.receipt("codex-req-0003").await.is_none(),
        "no receipt without consent"
    );
    assert!(harness.driver.state().keys.is_empty());
    let stale = harness
        .exec()
        .send(harness.request("codex-req-0003", "new prompt", "0".repeat(64).as_str()))
        .await;
    assert_eq!(stale.status, 409);
    assert_eq!(stale.body["draft_token"], token);
    let sent = harness
        .exec()
        .send(harness.request("codex-req-0003", "new prompt", &token))
        .await;
    assert_eq!(sent.status, 200, "{}", sent.body);
    {
        let state = harness.driver.state();
        assert_eq!(state.keys, vec![vec!["C-u", "C-k"], vec!["Enter"]]);
        assert_eq!(state.pasted, vec!["new prompt".to_owned()]);
    }
    let receipt = harness
        .settle("codex-req-0003", codex::State::Completed)
        .await;
    assert_eq!(receipt.state, codex::State::Completed);
    let probe = harness.exec().draft_status(&harness.uid, NAME, None).await;
    assert_eq!(probe.body, json!({"ok": true, "draft_state": "empty"}));
}

#[tokio::test]
async fn codex_lease_held_elsewhere_wrong_name_and_unknown_session() {
    let harness = Harness::new().await;
    harness.driver.state().conflict = Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 9)));
    let reply = harness
        .exec()
        .send(harness.request("codex-req-0004", "blocked", ""))
        .await;
    assert_eq!(reply.status, 409, "{}", reply.body);
    assert_eq!(reply.body["code"], "terminal_ownership");
    assert_eq!(reply.body["owner"]["ip"], "127.0.0.9");
    assert!(harness.receipt("codex-req-0004").await.is_none());
    harness.driver.state().conflict = None;
    let mut request = harness.request("codex-req-0005", "x", "");
    request.name = "other-terminal".into();
    let reply = harness.exec().send(request).await;
    assert_eq!(reply.status, 409);
    assert_eq!(reply.body["code"], "terminal_unlinked");
    assert!(
        reply.body["error"]
            .as_str()
            .unwrap()
            .starts_with("Codex 终端会话未连接")
    );
    let mut request = harness.request("codex-req-0005", "x", "");
    request.uid = "codex:0000000000000000".into();
    let reply = harness.exec().send(request).await;
    assert_eq!(reply.status, 400);
    assert_eq!(
        reply.body["error"],
        "服务端发送账本只用于已有 Claude/Codex 会话"
    );
    assert!(harness.driver.state().keys.is_empty());
}

#[tokio::test]
async fn codex_swallowed_enter_stays_uncertain_retry_refused_dismiss_hides() {
    let mut harness = Harness::new().await;
    harness.driver.state().swallow = true;
    let reply = harness
        .exec()
        .send(harness.request("codex-req-0006", "lost line", ""))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let epoch = reply.body["outbox_version"]["epoch"]
        .as_str()
        .unwrap()
        .to_owned();
    harness.ticks(6).await;
    let receipt = harness.receipt("codex-req-0006").await.unwrap();
    assert_eq!(receipt.state, codex::State::Uncertain);
    assert!(receipt.watch.is_some() && receipt.confirmation.is_some());
    assert!(!receipt.retryable());
    let retry = harness
        .exec()
        .retry(&harness.uid, "codex-req-0006", "", None)
        .await;
    assert_eq!(retry.status, 409, "{}", retry.body);
    assert_eq!(
        retry.body["error"],
        "消息已经写入终端或仍在确认，禁止重复发送"
    );
    assert_eq!(retry.body["outbox"].as_array().unwrap().len(), 1);
    let missing = harness
        .exec()
        .retry(&harness.uid, "codex-req-none", "", None)
        .await;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.body["error"], "待发送消息不存在");

    // Web restart: the uncertain row survives with a fresh epoch and is
    // never pasted again.
    harness.restart(limits()).await;
    let receipt = harness.receipt("codex-req-0006").await.unwrap();
    assert_eq!(receipt.state, codex::State::Uncertain);
    harness.ticks(3).await;
    let outbox = harness.outbox().await;
    assert_ne!(outbox["outbox_version"]["epoch"], epoch);
    assert_eq!(outbox["outbox"][0]["id"], "codex-req-0006");
    assert_eq!(outbox["outbox"][0]["state"], "failed");
    assert_eq!(outbox["outbox"][0]["attempts"], 1);
    {
        let state = harness.driver.state();
        assert!(state.pasted.is_empty() && state.keys.is_empty());
    }

    // Dismiss hides the row, keeps the tombstone; a second
    // discard is idempotent `ok`.
    let discard = harness.exec().discard(&harness.uid, "codex-req-0006").await;
    assert_eq!(discard.status, 200, "{}", discard.body);
    assert_eq!(discard.body["uid"], harness.uid);
    assert!(discard.body["outbox"].as_array().unwrap().is_empty());
    let again = harness.exec().discard(&harness.uid, "codex-req-0006").await;
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(again.body["ok"], true);
    let receipt = harness.receipt("codex-req-0006").await.unwrap();
    assert!(receipt.dismissed && receipt.state == codex::State::Uncertain);
    let replay = harness
        .exec()
        .send(harness.request("codex-req-0006", "lost line", ""))
        .await;
    assert_eq!(replay.status, 200);
    assert_eq!(replay.body["item"]["state"], "confirmed");
    // A late identical record does not resurrect the dismissed row, and a
    // fresh request with the same text is acknowledged by its own record.
    {
        let mut state = harness.driver.state();
        FakeDriver::write_turn(&mut state, "lost line");
    }
    harness.ticks(3).await;
    let receipt = harness.receipt("codex-req-0006").await.unwrap();
    assert!(receipt.dismissed && receipt.state == codex::State::Uncertain);
    let fresh = harness
        .exec()
        .send(harness.request("codex-req-0007", "lost line", ""))
        .await;
    assert_eq!(fresh.status, 200, "{}", fresh.body);
    let receipt = harness
        .settle("codex-req-0007", codex::State::Completed)
        .await;
    assert_eq!(receipt.state, codex::State::Completed);
    // The restarted fake numbers its turns from one again: the late record
    // was turn 1, the fresh request's own record is turn 2.
    assert_eq!(receipt.accepted.unwrap().turn_id, "fake-turn-2");
    assert_eq!(harness.driver.state().pasted, vec!["lost line".to_owned()]);
}

#[tokio::test]
async fn codex_record_without_turn_id_and_duplicates_confirm_in_row_order() {
    let harness = Harness::new().await;
    harness.driver.state().no_turn_id = true;
    let reply = harness
        .exec()
        .send(harness.request("codex-req-0008", "no turn", ""))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let receipt = harness
        .settle("codex-req-0008", codex::State::Acknowledged)
        .await;
    assert_eq!(receipt.state, codex::State::Acknowledged);
    assert!(receipt.accepted.is_some());
    let rows = harness.rollout_lines();
    assert!(
        rows.iter()
            .any(|row| row["payload"]["type"] == "task_started"),
        "task_started remains completion-only evidence"
    );

    harness.driver.state().no_turn_id = false;
    harness.driver.state().duplicate_record = true;
    let reply = harness
        .exec()
        .send(harness.request("codex-req-0009", "twice", ""))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let receipt = harness
        .settle("codex-req-0009", codex::State::Completed)
        .await;
    assert_eq!(receipt.state, codex::State::Completed);
    assert!(receipt.accepted.is_some());
    let state = harness.driver.state();
    assert_eq!(state.pasted, vec!["no turn".to_owned(), "twice".to_owned()]);
    assert_eq!(state.keys.len(), 2);
}

#[tokio::test]
async fn codex_ambiguous_enter_is_uncertain_and_unknown_composer_is_retryable() {
    let harness = Harness::new().await;
    harness.driver.state().enter_ambiguous = true;
    let reply = harness
        .exec()
        .send(harness.request("codex-req-0010", "ambiguous enter", ""))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let receipt = harness.receipt("codex-req-0010").await.unwrap();
    assert_eq!(receipt.state, codex::State::Uncertain);
    assert_eq!(
        receipt.issue.as_deref(),
        Some("终端调用结果不明；禁止重新注入")
    );
    harness.exec().tick().await.unwrap();
    harness.exec().tick().await.unwrap();
    {
        let state = harness.driver.state();
        assert_eq!(state.pasted, vec!["ambiguous enter".to_owned()]);
        assert_eq!(state.keys, vec![vec!["Enter"]]);
    }
    harness.exec().discard(&harness.uid, "codex-req-0010").await;

    // An approval prompt instead of the composer: nothing is pasted, the row
    // is a pre-write failure ("failed", attempts 0) and an explicit
    // retry re-inspects once the composer is back.
    harness.driver.state().enter_ambiguous = false;
    harness.driver.state().no_composer = true;
    let reply = harness
        .exec()
        .send(harness.request("codex-req-0011", "wait for composer", ""))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["item"]["state"], "failed");
    assert_eq!(reply.body["item"]["attempts"], 0);
    let receipt = harness.receipt("codex-req-0011").await.unwrap();
    assert_eq!(receipt.state, codex::State::FailedBeforeWrite);
    assert!(receipt.retryable());
    assert_eq!(harness.driver.state().pasted.len(), 1);
    harness.ticks(2).await;
    assert_eq!(
        harness.driver.state().pasted.len(),
        1,
        "no automatic re-dispatch of a pre-write failure"
    );
    harness.driver.state().no_composer = false;
    let retry = harness
        .exec()
        .retry(&harness.uid, "codex-req-0011", "", None)
        .await;
    assert_eq!(retry.status, 200, "{}", retry.body);
    assert_eq!(
        harness.driver.state().pasted,
        vec!["ambiguous enter".to_owned(), "wait for composer".to_owned()]
    );
    let receipt = harness
        .settle("codex-req-0011", codex::State::Completed)
        .await;
    assert_eq!(receipt.state, codex::State::Completed);
    assert_eq!(receipt.attempts, 1);
}

#[tokio::test]
async fn codex_lagging_capture_is_never_idle() {
    let harness = Harness::new().await;
    harness.driver.state().lag = 3;
    let probe = harness.exec().draft_status(&harness.uid, NAME, None).await;
    assert_eq!(probe.body, json!({"ok": true, "draft_state": "unknown"}));
    let reply = harness
        .exec()
        .send(harness.request("codex-req-0012", "lagging", ""))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["item"]["state"], "failed");
    assert_eq!(reply.body["item"]["attempts"], 0);
    assert!(harness.driver.state().pasted.is_empty());
}

#[tokio::test]
async fn codex_tracking_window_stops_polling_without_changing_state() {
    let mut limits = limits();
    limits.tracking_window = Duration::ZERO;
    let harness = Harness::with_limits(limits).await;
    harness.driver.state().swallow = true;
    let reply = harness
        .exec()
        .send(harness.request("codex-req-0013", "expired", ""))
        .await;
    assert_eq!(reply.status, 200);
    harness.exec().tick().await.unwrap();
    let receipt = harness.receipt("codex-req-0013").await.unwrap();
    assert_eq!(receipt.state, codex::State::Uncertain);
    let revision = receipt.revision;
    {
        let mut state = harness.driver.state();
        FakeDriver::write_turn(&mut state, "expired");
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    harness.exec().tick().await.unwrap();
    let receipt = harness.receipt("codex-req-0013").await.unwrap();
    assert_eq!(receipt.state, codex::State::Uncertain);
    assert_eq!(receipt.revision, revision);
    assert!(!receipt.retryable());
}

#[tokio::test]
async fn codex_follow_up_is_pasted_immediately_and_both_confirm() {
    // Codex owns queueing while working: the second send does not wait for
    // the first to be confirmed (only its own fence is captured).
    let harness = Harness::new().await;
    let a = harness
        .exec()
        .send(harness.request("codex-req-0014", "first", ""))
        .await;
    let b = harness
        .exec()
        .send(harness.request("codex-req-0015", "second", ""))
        .await;
    assert_eq!((a.status, b.status), (200, 200), "{} {}", a.body, b.body);
    assert_eq!(
        harness.driver.state().pasted,
        vec!["first".to_owned(), "second".to_owned()]
    );
    let rows = b.body["outbox"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], "codex-req-0014");
    assert_eq!(rows[1]["id"], "codex-req-0015");
    let first = harness
        .settle("codex-req-0014", codex::State::Completed)
        .await;
    let second = harness
        .settle("codex-req-0015", codex::State::Completed)
        .await;
    assert_eq!(first.accepted.unwrap().turn_id, "fake-turn-1");
    assert_eq!(second.accepted.unwrap().turn_id, "fake-turn-2");
}

#[tokio::test]
async fn codex_can_dismiss_during_paste_and_enter_without_repeating_the_write() {
    for preparing in [true, false] {
        let mut configured = limits();
        configured.prepare_timeout = Duration::from_secs(10);
        let harness = Harness::with_limits(configured).await;
        let arrived = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        {
            let mut state = harness.driver.state();
            let pause = Some((arrived.clone(), resume.clone()));
            if preparing {
                state.pause_paste = pause;
            } else {
                state.pause_enter = pause;
            }
        }
        let executor = harness.executor.as_ref().unwrap().clone();
        let request = harness.request("dismiss-during-write", "single write", "");
        let sending = tokio::spawn(async move { executor.send(request).await });
        tokio::time::timeout(Duration::from_secs(5), arrived.notified())
            .await
            .unwrap();
        let before = harness.receipt("dismiss-during-write").await.unwrap();
        assert_eq!(
            before.state,
            if preparing {
                codex::State::PrepareInFlight
            } else {
                codex::State::EnterInFlight
            }
        );
        let dismissed = tokio::time::timeout(
            Duration::from_secs(2),
            harness.exec().discard(&harness.uid, "dismiss-during-write"),
        )
        .await;
        resume.notify_one();
        let dismissed = dismissed.expect("retiring a receipt must not await terminal I/O");
        assert_eq!(dismissed.status, 200, "{}", dismissed.body);
        assert!(dismissed.body["outbox"].as_array().unwrap().is_empty());
        resume.notify_one();
        let reply = tokio::time::timeout(Duration::from_secs(5), sending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply.status, 200, "{}", reply.body);
        harness.ticks(3).await;
        let row = harness.receipt("dismiss-during-write").await.unwrap();
        assert!(row.dismissed && !row.visible_in_outbox() && !row.retryable());
        let state = harness.driver.state();
        assert_eq!(state.pasted, vec!["single write"]);
        assert_eq!(
            state
                .keys
                .iter()
                .flatten()
                .filter(|key| **key == "Enter")
                .count(),
            1
        );
    }
}
