//! Executor tests: real engine/ledger/session store, fake terminal driver and
//! fake resolver. The fake driver plays the CLI: it keeps a composer buffer,
//! renders a Claude-like screen, and appends a synthetic `user` record when
//! Enter is pressed (unless told to swallow it). No process is started.
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
    net::{IpAddr, Ipv4Addr},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const SID: &str = "0d3c5a8e-4b7f-4c21-9a6d-2e1f3b4c5d6e";
const INSTANCE: &str = "synthetic-instance-0001abcd";
const RULE: &str = "────────────────────────────────────────────────";

struct FakeState {
    buffer: String,
    no_composer: bool,
    pasted: Vec<String>,
    keys: Vec<Vec<&'static str>>,
    conflict: Option<IpAddr>,
    paste_fail_once: bool,
    hide_pasted_composer: bool,
    enter_ambiguous: bool,
    swallow: bool,
    record_path: PathBuf,
    parent: Option<String>,
    records: usize,
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
                paste_fail_once: false,
                hide_pasted_composer: false,
                enter_ambiguous: false,
                swallow: false,
                record_path,
                // Claude chains every record to the previous one; a second
                // root would be a branch switch, not an append.
                parent: Some("seed-user".into()),
                records: 0,
            }),
        })
    }
    fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }
    fn screen(state: &FakeState) -> ScreenCapture {
        if state.no_composer || (state.hide_pasted_composer && !state.buffer.is_empty()) {
            return ScreenCapture {
                text: "Choose an option\n❯ 1. yes\n  2. no".into(),
                cursor: (2, 1),
                lag: Some(0),
                dropped: Some(0),
                resets: Some(0),
                alt: false,
            };
        }
        let text = [
            "FAKE_CLAUDE_READY".to_owned(),
            String::new(),
            RULE.to_owned(),
            format!("❯ {}", state.buffer),
            RULE.to_owned(),
        ]
        .join("\n");
        ScreenCapture {
            text,
            cursor: ((2 + state.buffer.chars().count()) as u16, 3),
            lag: Some(0),
            dropped: Some(0),
            resets: Some(0),
            alt: false,
        }
    }
    fn write_record(state: &mut FakeState, text: &str) {
        state.records += 1;
        let uuid = format!("fake-user-{}", state.records);
        let row = json!({
            "type": "user", "uuid": uuid, "parentUuid": state.parent,
            "sessionId": SID, "cwd": "/synthetic", "timestamp": "2026-09-12T10:00:05Z",
            "isSidechain": false, "message": {"role": "user", "content": text},
        });
        let mut bytes = fs::read(&state.record_path).unwrap();
        bytes.extend_from_slice(format!("{row}\n").as_bytes());
        fs::write(&state.record_path, bytes).unwrap();
        state.parent = Some(uuid);
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
            let mut state = self.state();
            if state.paste_fail_once {
                state.paste_fail_once = false;
                return Err(DriverError {
                    status: 503,
                    code: "terminal_unavailable",
                    message: "synthetic paste failure".into(),
                    ambiguous: false,
                    owner_ip: None,
                });
            }
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
                            Self::write_record(&mut state, &text);
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

fn synthetic_target(name: &str, uid: &str) -> DeliveryTarget {
    let observation = HostObservation {
        summary: SessionSummary {
            name: name.into(),
            created: 1,
            attached: false,
            pid: 4242,
            host_pid: 4241,
            cwd: "/synthetic".into(),
            cmd: "fake".into(),
            cols: 80,
            rows: 24,
            owned: true,
            server: "ptyhost",
            backend: "ptyhost",
        },
        association: AssociationState::Declared(Association {
            source: Source::Claude,
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
    let bound = BoundTarget::from_observation(&observation, Source::Claude, SID, uid).unwrap();
    DeliveryTarget {
        name: name.into(),
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
    format!("claude:{}", &hex[..16])
}

fn limits() -> ExecutorLimits {
    ExecutorLimits {
        workers: 2,
        confirm_timeout: Duration::from_millis(50),
        tracking_window: Duration::from_secs(3600),
        // The background loop stays inert; tests drive `tick()` explicitly.
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
        let root = base.join("claude");
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let record_path = project.join(format!("{SID}.jsonl"));
        let seed = json!({
            "type": "user", "uuid": "seed-user", "parentUuid": null, "sessionId": SID,
            "cwd": "/synthetic", "timestamp": "2026-09-12T10:00:00Z", "isSidechain": false,
            "message": {"role": "user", "content": "seed prompt"},
        });
        fs::write(&record_path, format!("{seed}\n")).unwrap();
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
                claude: Some(self.root.clone()),
                ..Default::default()
            })),
            workers: Arc::new(Semaphore::new(4)),
        };
        let resolver = Arc::new(FakeResolver {
            target: synthetic_target("sessiondock-claude-0d3c5a8e", &self.uid),
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
    /// The session store rescans its inventory at most every 500 ms; the
    /// tracker ticks until the receipt reaches `wanted` or five seconds pass.
    async fn settle(&self, id: &str, wanted: State) -> Receipt {
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
    fn request(&self, id: &str, text: &str, overwrite: &str) -> SendRequest {
        SendRequest {
            uid: self.uid.clone(),
            agent: String::new(),
            name: "sessiondock-claude-0d3c5a8e".into(),
            text: text.into(),
            media: Vec::new(),
            request_id: id.into(),
            overwrite_draft: overwrite.into(),
            page_lease: None,
        }
    }
    async fn receipt(&self, id: &str) -> Option<Receipt> {
        let id = id.to_owned();
        self.svc()
            .with_engine(move |engine| engine.claude_receipt(&id))
            .await
            .unwrap()
            .unwrap()
    }
    async fn outbox(&self) -> Value {
        let native = self.exec().native(&self.uid, None).await.unwrap();
        self.exec()
            .outbox(&Scope {
                uid: native.scope.uid,
                session_id: native.scope.session_id,
                agent_id: None,
            })
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn send_persists_injects_two_steps_and_confirms_from_native_record() {
    let harness = Harness::new().await;
    let reply = harness
        .exec()
        .send(harness.request("request-0001", "hello reliable send", ""))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["ok"], true);
    assert_eq!(reply.body["item"]["id"], "request-0001");
    assert_eq!(reply.body["item"]["state"], "ambiguous");
    assert_eq!(reply.body["item"]["attempts"], 1);
    assert_eq!(reply.body["item"]["server"], true);
    assert!(reply.body["outbox_version"]["epoch"].is_string());
    {
        let state = harness.driver.state();
        assert_eq!(state.pasted, vec!["hello reliable send".to_owned()]);
        assert_eq!(state.keys, vec![vec!["Enter"]]);
    }
    let receipt = harness.receipt("request-0001").await.unwrap();
    assert_eq!(receipt.state, State::Uncertain);
    assert!(receipt.attempted && receipt.enter.is_some());
    assert!(receipt.confirmation.is_some());
    assert_eq!(receipt.request.payload.target.host_instance, INSTANCE);
    assert_eq!(
        receipt.request.payload.target.ownership_epoch,
        OWNERSHIP_EPOCH
    );
    // The native record the fake CLI wrote on Enter confirms the receipt.
    let receipt = harness.settle("request-0001", State::Accepted).await;
    assert_eq!(receipt.state, State::Accepted);
    let accepted = receipt.accepted.unwrap();
    assert_eq!(accepted.turn.user_uuid, "fake-user-1");
    assert!(accepted.record.start >= receipt.confirmation.unwrap().offset);
    assert_eq!(
        accepted.association,
        claude::Association::VerifiedEnter(receipt.enter.unwrap())
    );
    assert_eq!(
        harness.outbox().await["outbox"].as_array().unwrap().len(),
        0
    );
    // Nothing was injected twice.
    let state = harness.driver.state();
    assert_eq!(state.pasted.len(), 1);
    assert_eq!(state.keys.len(), 1);
}

#[tokio::test]
async fn same_request_id_replays_without_touching_the_terminal() {
    let harness = Harness::new().await;
    harness.driver.state().swallow = true;
    let first = harness
        .exec()
        .send(harness.request("request-0002", "once only", ""))
        .await;
    assert_eq!(first.status, 200);
    let again = harness
        .exec()
        .send(harness.request("request-0002", "once only", "ignored-token"))
        .await;
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(again.body["item"]["id"], "request-0002");
    assert_eq!(again.body["item"]["state"], "ambiguous");
    assert_eq!(
        again.body["outbox_version"]["revision"],
        first.body["outbox_version"]["revision"]
    );
    let conflict = harness
        .exec()
        .send(harness.request("request-0002", "different text", ""))
        .await;
    assert_eq!(conflict.status, 400);
    assert_eq!(conflict.body["code"], "request_conflict");
    assert_eq!(conflict.body["error"], "重复发送 ID 对应了不同消息");
    let state = harness.driver.state();
    assert_eq!(state.pasted.len(), 1);
    assert_eq!(state.keys.len(), 1);
}

#[tokio::test]
async fn draft_conflict_needs_consent_token_then_clears_and_sends() {
    let harness = Harness::new().await;
    harness.driver.state().buffer = "old draft".into();
    let probe = harness
        .exec()
        .draft_status(&harness.uid, "sessiondock-claude-0d3c5a8e", None)
        .await;
    assert_eq!(probe.status, 200);
    assert_eq!(probe.body["draft_state"], "editing");
    assert_eq!(probe.body["draft_conflict"], true);
    let token = probe.body["draft_token"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 64);
    let refused = harness
        .exec()
        .send(harness.request("request-0003", "new prompt", ""))
        .await;
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.body["draft_conflict"], true);
    assert_eq!(refused.body["draft_token"], token);
    assert!(refused.body["outbox"].as_array().unwrap().is_empty());
    assert!(
        harness.receipt("request-0003").await.is_none(),
        "no receipt without consent"
    );
    assert!(harness.driver.state().keys.is_empty());
    let stale = harness
        .exec()
        .send(harness.request("request-0003", "new prompt", "0".repeat(64).as_str()))
        .await;
    assert_eq!(stale.status, 409);
    assert_eq!(stale.body["draft_token"], token);
    let sent = harness
        .exec()
        .send(harness.request("request-0003", "new prompt", &token))
        .await;
    assert_eq!(sent.status, 200, "{}", sent.body);
    {
        let state = harness.driver.state();
        assert_eq!(state.keys, vec![vec!["C-u", "C-k"], vec!["Enter"]]);
        assert_eq!(state.pasted, vec!["new prompt".to_owned()]);
    }
    assert_eq!(
        harness.settle("request-0003", State::Accepted).await.state,
        State::Accepted
    );
    let probe = harness
        .exec()
        .draft_status(&harness.uid, "sessiondock-claude-0d3c5a8e", None)
        .await;
    assert_eq!(probe.body, json!({"ok": true, "draft_state": "empty"}));
}

#[tokio::test]
async fn lease_held_elsewhere_is_the_documented_ownership_error() {
    let harness = Harness::new().await;
    harness.driver.state().conflict = Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 9)));
    let reply = harness
        .exec()
        .send(harness.request("request-0004", "blocked", ""))
        .await;
    assert_eq!(reply.status, 409, "{}", reply.body);
    assert_eq!(reply.body["code"], "terminal_ownership");
    assert_eq!(reply.body["owner"]["ip"], "127.0.0.9");
    assert!(harness.receipt("request-0004").await.is_none());
    let probe = harness
        .exec()
        .draft_status(&harness.uid, "sessiondock-claude-0d3c5a8e", None)
        .await;
    assert_eq!(probe.status, 409);
    assert_eq!(probe.body["code"], "terminal_ownership");
}

#[tokio::test]
async fn wrong_name_unknown_session_and_media_rules() {
    let harness = Harness::new().await;
    let mut request = harness.request("request-0005", "x", "");
    request.name = "other-terminal".into();
    let reply = harness.exec().send(request).await;
    assert_eq!(reply.status, 409);
    assert_eq!(reply.body["code"], "terminal_unlinked");
    let mut request = harness.request("request-0005", "x", "");
    request.uid = "claude:0000000000000000".into();
    let reply = harness.exec().send(request).await;
    assert_eq!(reply.status, 400);
    assert_eq!(
        reply.body["error"],
        "服务端发送账本只用于已有 Claude/Codex 会话"
    );
    let reply = harness
        .exec()
        .send(harness.request("request-0005", "   ", ""))
        .await;
    assert_eq!(reply.status, 400);
    assert!(harness.driver.state().keys.is_empty());
}

#[tokio::test]
async fn swallowed_enter_stays_uncertain_retry_refused_discard_retires() {
    let harness = Harness::new().await;
    harness.driver.state().swallow = true;
    let reply = harness
        .exec()
        .send(harness.request("request-0006", "lost line", ""))
        .await;
    assert_eq!(reply.status, 200);
    for _ in 0..6 {
        tokio::time::sleep(Duration::from_millis(120)).await;
        harness.exec().tick().await.unwrap();
    }
    let receipt = harness.receipt("request-0006").await.unwrap();
    assert_eq!(receipt.state, State::Uncertain);
    assert!(receipt.watch.is_some() && receipt.confirmation.is_some());
    let retry = harness
        .exec()
        .retry(&harness.uid, "request-0006", "", None)
        .await;
    assert_eq!(retry.status, 409, "{}", retry.body);
    assert_eq!(
        retry.body["error"],
        "消息已经提交到终端，禁止盲目重发；请等待原生记录或打开终端检查"
    );
    assert_eq!(retry.body["outbox"].as_array().unwrap().len(), 1);
    let missing = harness
        .exec()
        .retry(&harness.uid, "request-none-0", "", None)
        .await;
    assert_eq!(missing.status, 404);
    let discard = harness.exec().discard(&harness.uid, "request-0006").await;
    assert_eq!(discard.status, 200, "{}", discard.body);
    assert_eq!(discard.body["uid"], harness.uid);
    assert!(discard.body["outbox"].as_array().unwrap().is_empty());
    let again = harness.exec().discard(&harness.uid, "request-0006").await;
    assert_eq!(again.status, 404);
    let receipt = harness.receipt("request-0006").await.unwrap();
    assert!(receipt.dismissed && receipt.state == State::Uncertain);
    // Dismissal keeps the deduplication identity: a replay is still a lookup.
    let replay = harness
        .exec()
        .send(harness.request("request-0006", "lost line", ""))
        .await;
    assert_eq!(replay.status, 200);
    assert_eq!(replay.body["item"]["state"], "confirmed");
    let state = harness.driver.state();
    assert_eq!(state.pasted.len(), 1);
    assert_eq!(state.keys.len(), 1);
}

#[tokio::test]
async fn ambiguous_enter_and_failed_paste_are_uncertain_never_repeated() {
    let harness = Harness::new().await;
    harness.driver.state().enter_ambiguous = true;
    let reply = harness
        .exec()
        .send(harness.request("request-0007", "ambiguous enter", ""))
        .await;
    assert_eq!(reply.status, 200);
    let receipt = harness.receipt("request-0007").await.unwrap();
    assert_eq!(receipt.state, State::Uncertain);
    assert_eq!(
        receipt.issue.as_deref(),
        Some("终端调用结果不明；禁止重新注入")
    );
    harness.driver.state().enter_ambiguous = false;
    harness.driver.state().paste_fail_once = true;
    let reply = harness
        .exec()
        .send(harness.request("request-0008", "paste fails", ""))
        .await;
    assert_eq!(reply.status, 200);
    let receipt = harness.receipt("request-0008").await.unwrap();
    assert_eq!(receipt.state, State::Uncertain);
    assert!(receipt.attempted && receipt.enter.is_none());
    harness.exec().tick().await.unwrap();
    harness.exec().tick().await.unwrap();
    let state = harness.driver.state();
    assert_eq!(state.pasted, vec!["ambiguous enter".to_owned()]);
    assert_eq!(state.keys, vec![vec!["Enter"]]);
}

#[tokio::test]
async fn restart_recovers_uncertain_rows_without_reinjection() {
    let mut harness = Harness::new().await;
    harness.driver.state().swallow = true;
    let first = harness
        .exec()
        .send(harness.request("request-0009", "before restart", ""))
        .await;
    assert_eq!(first.status, 200);
    let epoch = first.body["outbox_version"]["epoch"]
        .as_str()
        .unwrap()
        .to_owned();
    harness.restart(limits()).await;
    let receipt = harness.receipt("request-0009").await.unwrap();
    assert_eq!(receipt.state, State::Uncertain);
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(120)).await;
        harness.exec().tick().await.unwrap();
    }
    let outbox = harness.outbox().await;
    assert_ne!(outbox["outbox_version"]["epoch"], epoch);
    assert_eq!(outbox["outbox"][0]["id"], "request-0009");
    assert_eq!(outbox["outbox"][0]["state"], "ambiguous");
    {
        let state = harness.driver.state();
        assert!(state.pasted.is_empty() && state.keys.is_empty());
    }
    // A late native record still confirms after the restart.
    {
        let mut state = harness.driver.state();
        FakeDriver::write_record(&mut state, "before restart");
    }
    assert_eq!(
        harness.settle("request-0009", State::Accepted).await.state,
        State::Accepted
    );
}

#[tokio::test]
async fn unknown_composer_keeps_row_persisted_until_redispatch_succeeds() {
    let harness = Harness::new().await;
    harness.driver.state().no_composer = true;
    let reply = harness
        .exec()
        .send(harness.request("request-0010", "wait for composer", ""))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body["item"]["state"], "persisted");
    assert_eq!(reply.body["item"]["attempts"], 0);
    let receipt = harness.receipt("request-0010").await.unwrap();
    assert_eq!(receipt.state, State::Queued);
    assert!(harness.driver.state().pasted.is_empty());
    // Retry of an unwritten waiter is the domain's only retry: re-inspection.
    let retry = harness
        .exec()
        .retry(&harness.uid, "request-0010", "", None)
        .await;
    assert_eq!(retry.status, 200);
    assert!(harness.driver.state().pasted.is_empty());
    harness.driver.state().no_composer = false;
    harness.exec().tick().await.unwrap();
    assert_eq!(
        harness.driver.state().pasted,
        vec!["wait for composer".to_owned()]
    );
    assert_eq!(
        harness.settle("request-0010", State::Accepted).await.state,
        State::Accepted
    );
}

#[tokio::test]
async fn tracking_window_annotates_once_and_still_confirms_late_native_input() {
    let mut limits = limits();
    limits.tracking_window = Duration::ZERO;
    let harness = Harness::with_limits(limits).await;
    harness.driver.state().swallow = true;
    let reply = harness
        .exec()
        .send(harness.request("request-0011", "expired", ""))
        .await;
    assert_eq!(reply.status, 200);
    harness.exec().tick().await.unwrap();
    let receipt = harness.receipt("request-0011").await.unwrap();
    assert_eq!(receipt.state, State::Uncertain);
    assert_eq!(
        receipt.issue.as_deref(),
        Some("等待确认超时，只允许限频复核，不能重新注入")
    );
    let revision = receipt.revision;
    harness.exec().tick().await.unwrap();
    assert_eq!(
        harness.receipt("request-0011").await.unwrap().revision,
        revision
    );
    // A record after the window still confirms without a second injection.
    {
        let mut state = harness.driver.state();
        FakeDriver::write_record(&mut state, "expired");
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    harness.exec().tick().await.unwrap();
    let receipt = harness.settle("request-0011", State::Accepted).await;
    assert_eq!(receipt.state, State::Accepted);
    assert!(receipt.issue.is_none());
    assert_eq!(harness.driver.state().pasted, vec!["expired".to_owned()]);
}

#[tokio::test]
async fn manually_submitted_unverified_paste_confirms_after_expired_restart() {
    let mut harness = Harness::new().await;
    harness.driver.state().hide_pasted_composer = true;
    let text = "manual submit\n\nattachment: ./synthetic/file.json";
    let id = "request-manual-paste";
    let reply = harness.exec().send(harness.request(id, text, "")).await;
    assert_eq!(reply.status, 200);
    let receipt = harness.receipt(id).await.unwrap();
    assert_eq!(receipt.state, State::Uncertain);
    assert!(receipt.attempted && receipt.enter.is_none());
    assert!(receipt.issue.as_deref().unwrap().contains("未发送 Enter"));
    assert_eq!(harness.driver.state().pasted, vec![text.to_owned()]);
    assert!(harness.driver.state().keys.is_empty());
    assert_eq!(
        harness
            .exec()
            .retry(&harness.uid, id, "", None)
            .await
            .status,
        409
    );

    // The operator submits the prepared buffer; this is not a managed Enter.
    {
        let mut state = harness.driver.state();
        FakeDriver::write_record(&mut state, text);
        state.buffer.clear();
    }
    let mut expired = limits();
    expired.tracking_window = Duration::ZERO;
    harness.restart(expired).await;
    let receipt = harness.settle(id, State::Accepted).await;
    assert_eq!(receipt.state, State::Accepted);
    assert!(receipt.enter.is_none());
    assert_eq!(
        receipt.accepted.unwrap().association,
        claude::Association::PossibleTextMatch
    );
    assert!(
        harness.outbox().await["outbox"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(harness.driver.state().pasted.is_empty());
    assert!(harness.driver.state().keys.is_empty());

    // The installed ledger must remain valid on another restart, and replay
    // of the original ID must remain a confirmed lookup.
    harness.restart(expired).await;
    assert_eq!(harness.receipt(id).await.unwrap().state, State::Accepted);
    let reply = harness.exec().send(harness.request(id, text, "")).await;
    assert_eq!(reply.body["item"]["state"], "confirmed");
    assert!(harness.driver.state().pasted.is_empty());
}

#[tokio::test]
async fn fifo_second_request_waits_for_the_first_and_epoch_revision_advance() {
    let harness = Harness::new().await;
    let a = harness
        .exec()
        .send(harness.request("request-0012", "first", ""))
        .await;
    let b = harness
        .exec()
        .send(harness.request("request-0013", "second", ""))
        .await;
    assert_eq!((a.status, b.status), (200, 200));
    assert_eq!(
        a.body["outbox_version"]["epoch"],
        b.body["outbox_version"]["epoch"]
    );
    assert!(
        b.body["outbox_version"]["revision"].as_u64()
            > a.body["outbox_version"]["revision"].as_u64()
    );
    let rows = b.body["outbox"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], "request-0012");
    assert_eq!(rows[1]["id"], "request-0013");
    assert_eq!(
        harness.settle("request-0012", State::Accepted).await.state,
        State::Accepted
    );
    assert_eq!(
        harness.settle("request-0013", State::Accepted).await.state,
        State::Accepted
    );
    assert_eq!(
        harness.driver.state().pasted,
        vec!["first".to_owned(), "second".to_owned()]
    );
}

#[test]
fn request_id_normalization_matches_python_characters_without_trimming() {
    for id in ["x", "短🦀", "../bad ?#", "  ", "\n\0"] {
        assert_eq!(normalize_request_id(id).unwrap(), id);
    }
    let id = normalize_request_id("").unwrap();
    assert_eq!(id.len(), 36);
    assert_eq!(id.chars().filter(|&ch| ch == '-').count(), 4);
    assert_eq!(&id[14..15], "4");
    assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    assert_ne!(id, normalize_request_id("").unwrap());
    assert_eq!(
        normalize_request_id(&"🦀".repeat(129)).unwrap(),
        "🦀".repeat(128)
    );
}

#[tokio::test]
async fn request_ids_reopen_replay_and_lookup_without_byte_or_ascii_gates() {
    for raw in ["x".to_owned(), " ../短🦀 ?# ".to_owned(), "🦀".repeat(129)] {
        let id: String = raw.chars().take(128).collect();
        let mut harness = Harness::new().await;
        harness.driver.state().no_composer = true;
        let sent = harness
            .exec()
            .send(harness.request(&raw, "persist once", ""))
            .await;
        assert_eq!(sent.status, 200, "{}", sent.body);
        assert_eq!(sent.body["item"]["id"], id);
        harness.restart(limits()).await;
        harness.driver.state().no_composer = true;
        assert_eq!(harness.receipt(&id).await.unwrap().request.id, id);
        let replay = harness
            .exec()
            .send(harness.request(&raw, "persist once", ""))
            .await;
        assert_eq!(replay.status, 200, "{}", replay.body);
        assert_eq!(replay.body["item"]["id"], id);
        assert_eq!(replay.body["outbox"].as_array().unwrap().len(), 1);
        let conflict = harness
            .exec()
            .send(harness.request(&raw, "different", ""))
            .await;
        assert_eq!(conflict.status, 400);
        assert_eq!(conflict.body["code"], "request_conflict");
        let absent = if id.trim() != id {
            id.trim().to_owned()
        } else {
            format!("{id}!")
        };
        assert_eq!(
            harness
                .exec()
                .retry(&harness.uid, &absent, "", None)
                .await
                .status,
            404
        );
        assert_eq!(
            harness.exec().discard(&harness.uid, &absent).await.status,
            404
        );
        assert_eq!(
            harness
                .exec()
                .retry(&harness.uid, &id, "", None)
                .await
                .status,
            200
        );
        assert_eq!(harness.exec().discard(&harness.uid, &id).await.status, 200);
        assert!(harness.driver.state().pasted.is_empty());
        assert!(harness.driver.state().keys.is_empty());
        harness.restart(limits()).await;
        let replay = harness
            .exec()
            .send(harness.request(&raw, "persist once", ""))
            .await;
        assert_eq!(replay.status, 200, "{}", replay.body);
        assert!(harness.driver.state().pasted.is_empty());
    }
}
