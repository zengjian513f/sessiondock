//! Raw HTTP terminal input (`/api/term/send`, `/api/term/scroll`) against one
//! isolated real ptyhost per test, each running a fixed free `/bin/sh` loop in
//! a private temporary directory, plus loopback fake peers for the ambiguous
//! and rejected acknowledgement boundaries. No native home, model CLI or
//! production host is touched. The real-host tests skip themselves when the
//! local ptyhost target has not been built (or point at it with
//! `SESSIONDOCK_TEST_PTYHOST_BINARY`).
#![cfg(unix)]

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sessiondock::{app_with_shutdown, config::Config, sessions::SessionRoots};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const NAME: &str = "input-shell";
const SID: &str = "synthetic-codex";
const SHELL: &str = "stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case \"$command\" in
    ping) printf 'RS_PING_OK\\n' ;;
    \"\t\") printf 'RS_TAB_OK\\n' ;;
    *\"[A\") printf 'RS_UP_OK\\n' ;;
    quit) printf 'RS_SHELL_DONE\\n'; exit 0 ;;
    *) printf 'RS_UNKNOWN_INPUT\\n' ;;
  esac
done
";

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn ptyhost_binary() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("SESSIONDOCK_TEST_PTYHOST_BINARY") {
        let path = PathBuf::from(explicit);
        assert!(path.is_absolute() && path.is_file(), "explicit host binary");
        return Some(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/ptyhost")
        .canonicalize()
        .ok()
        .filter(|path| path.is_file())
}

struct Harness {
    _directory: TempDir,
    hosts: PathBuf,
    app: Router,
    address: SocketAddr,
    uid: String,
    build: String,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Harness {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let native = root.join("native");
        let hosts = root.join("hosts");
        std::fs::create_dir(&native).unwrap();
        std::fs::create_dir(&hosts).unwrap();
        std::fs::write(
            native.join("rollout-synthetic-codex.jsonl"),
            include_bytes!("fixtures/codex/2026/09/11/rollout-synthetic-codex.jsonl"),
        )
        .unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let app = app_with_shutdown(
            Config {
                bind: address,
                web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
                roots: SessionRoots {
                    claude: None,
                    codex: Some(native),
                    grok: None,
                },
                ptyhost_dir: Some(hosts.clone()),
                ..Default::default()
            },
            cancel.clone(),
        )
        .unwrap();
        let task = {
            let (app, cancel) = (app.clone(), cancel.clone());
            tokio::spawn(async move {
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(cancel.cancelled_owned())
                .await
                .unwrap();
            })
        };
        let mut harness = Self {
            _directory: directory,
            hosts,
            app,
            address,
            uid: String::new(),
            build: String::new(),
            cancel,
            task,
        };
        let (_, sessions) = harness.request("GET", "/api/sessions", Value::Null).await;
        harness.uid = sessions["sessions"][0]["uid"].as_str().unwrap().into();
        let (_, meta) = harness.request("GET", "/api/meta", Value::Null).await;
        harness.build = meta["build"].as_str().unwrap().into();
        assert_eq!(meta["capabilities"]["terminal_input"], true);
        assert_eq!(meta["capabilities"]["outbox"], false);
        assert_eq!(meta["capabilities"]["terminal_create"], false);
        harness
    }

    async fn request(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", self.address.to_string())
            .header("content-type", "application/json")
            .body(if method == "GET" {
                Body::empty()
            } else {
                Body::from(body.to_string())
            })
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        if path.starts_with("/api/term/") {
            assert_eq!(response.headers()["cache-control"], "no-store", "{path}");
        }
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let encoded = body.to_string();
        for private in ["\"port\"", "\"sock\"", "\"argv\"", "PRIVATE"] {
            assert!(!encoded.contains(private), "{path}: {encoded}");
        }
        (status, body)
    }

    fn binding(&self, instance: &str) -> Value {
        json!({"uid":self.uid,"instance_id":instance})
    }

    async fn claim(&self, page: &str, instance: &str, force: bool) -> String {
        let mut body = self.binding(instance);
        body["name"] = json!(NAME);
        body["page"] = json!(page);
        body["force"] = json!(force);
        let (status, reply) = self.request("POST", "/api/term/claim", body).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        reply["token"].as_str().unwrap().into()
    }

    async fn send(
        &self,
        page: &str,
        token: &str,
        instance: &str,
        input: Value,
    ) -> (StatusCode, Value) {
        let mut body = self.binding(instance);
        body["name"] = json!(NAME);
        body["page"] = json!(page);
        body["token"] = json!(token);
        body["_build"] = json!(self.build);
        for (key, value) in input.as_object().unwrap() {
            body[key] = value.clone();
        }
        self.request("POST", "/api/term/send", body).await
    }

    async fn connect(&self, page: &str, token: &str, instance: &str) -> Ws {
        let uri = format!(
            "ws://{}/api/term/attach?name={NAME}&page={page}&token={token}&cols=80&rows=24&uid={}&instance_id={instance}",
            self.address, self.uid
        );
        let mut request = uri.into_client_request().unwrap();
        request.headers_mut().insert(
            "origin",
            format!("http://{}", self.address).parse().unwrap(),
        );
        timeout(Duration::from_secs(5), connect_async(request))
            .await
            .unwrap()
            .unwrap()
            .0
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

/// One isolated real host process; only this exact child is ever stopped.
struct RealHost {
    child: Child,
    record: PathBuf,
}

impl RealHost {
    async fn spawn(binary: &Path, harness: &Harness, instance: &str) -> Self {
        let meta = json!({"source":"codex","sid":SID,"uid":harness.uid,"instance_id":instance});
        let child = Command::new(binary)
            .arg("--dir")
            .arg(&harness.hosts)
            .args(["run", "--name", NAME, "--cwd"])
            .arg(&harness.hosts)
            .args(["--cols", "80", "--rows", "24", "--meta"])
            .arg(meta.to_string())
            .args(["--", "/bin/sh", "-c", SHELL])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let record = harness.hosts.join(format!("{NAME}.json"));
        let host = Self { child, record };
        timeout(Duration::from_secs(10), async {
            loop {
                let (_, list) = harness.request("GET", "/api/term/list", Value::Null).await;
                if list["sessions"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|row| row["instance_id"] == instance))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("isolated host must publish its bound instance");
        host
    }

    async fn wait_exit(&mut self) {
        timeout(Duration::from_secs(10), async {
            loop {
                if self.child.try_wait().unwrap().is_some() && !self.record.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("host must exit and remove its record after quit");
    }
}

impl Drop for RealHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn read_until(ws: &mut Ws, needle: &str) -> (String, Option<(u16, String)>) {
    let mut text = String::new();
    let outcome = timeout(Duration::from_secs(10), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Binary(bytes))) => {
                    text.push_str(&String::from_utf8_lossy(&bytes));
                    if text.contains(needle) {
                        return None;
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    let frame = frame.unwrap();
                    return Some((frame.code.into(), frame.reason.to_string()));
                }
                Some(Ok(_)) => {}
                _ => panic!("attachment ended before {needle:?}: {text:?}"),
            }
        }
    })
    .await;
    match outcome {
        Ok(close) => (text, close),
        Err(_) => panic!("timed out waiting for {needle:?}: {text:?}"),
    }
}

fn ok_receipt(status: StatusCode, body: &Value, bytes: usize) {
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        &json!({"ok":true,"bytes":bytes,"acknowledged":true,"processed":"unknown"})
    );
}

#[tokio::test]
async fn text_and_named_keys_reach_the_isolated_shell_under_bounds() {
    let Some(binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let h = Harness::new().await;
    let instance = "synthetic-input-instance-0001";
    let _host = RealHost::spawn(&binary, &h, instance).await;
    let token = h.claim("page", instance, false).await;
    let mut ws = h.connect("page", &token, instance).await;
    read_until(&mut ws, "RS_SHELL_READY").await;

    // Text is written as-is; Enter is a separate named key. The shell's reply
    // arrives on the existing attachment, the only evidence of processing.
    let (status, body) = h
        .send("page", &token, instance, json!({"data":"ping"}))
        .await;
    ok_receipt(status, &body, 4);
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["enter"]}))
        .await;
    ok_receipt(status, &body, 1);
    read_until(&mut ws, "RS_PING_OK").await;
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["Tab","Enter"]}))
        .await;
    ok_receipt(status, &body, 2);
    read_until(&mut ws, "RS_TAB_OK").await;
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["up","enter"]}))
        .await;
    ok_receipt(status, &body, 4);
    read_until(&mut ws, "RS_UP_OK").await;
    let (status, body) = h
        .send("page", &token, instance, json!({"data":"中文 ok"}))
        .await;
    ok_receipt(status, &body, 9);
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["ctrl-u","enter"]}))
        .await;
    ok_receipt(status, &body, 2);

    // Malformed requests never reach the host.
    for (input, status, code) in [
        (
            json!({"data":"x","keys":["enter"]}),
            StatusCode::BAD_REQUEST,
            "invalid_terminal_input",
        ),
        (json!({}), StatusCode::BAD_REQUEST, "invalid_terminal_input"),
        (
            json!({"data":""}),
            StatusCode::BAD_REQUEST,
            "invalid_terminal_input",
        ),
        (
            json!({"keys":[]}),
            StatusCode::BAD_REQUEST,
            "invalid_terminal_input",
        ),
        (
            json!({"keys":vec!["enter";257]}),
            StatusCode::BAD_REQUEST,
            "invalid_terminal_input",
        ),
        (
            json!({"text":"legacy submit"}),
            StatusCode::BAD_REQUEST,
            "invalid_terminal_input",
        ),
        (
            json!({"data":"a".repeat(1024 * 1024 + 1)}),
            StatusCode::PAYLOAD_TOO_LARGE,
            "terminal_input_too_large",
        ),
    ] {
        let (got, body) = h.send("page", &token, instance, input).await;
        assert_eq!(got, status, "{body}");
        assert_eq!(body["code"], code, "{body}");
    }
    // The host types unknown nonempty key names literally.
    for key in ["typed literally", "C-Escape"] {
        let (status, body) = h
            .send("page", &token, instance, json!({"keys":[key]}))
            .await;
        ok_receipt(status, &body, key.len());
    }
    let (status, body) = h
        .send("page", &token, instance, json!({"data":"x","enter":true}))
        .await;
    ok_receipt(status, &body, 1);
    // The legacy stale-build gate applies to text, never to keys.
    let mut stale = h.binding(instance);
    stale["name"] = json!(NAME);
    stale["page"] = json!("page");
    stale["token"] = json!(token);
    stale["data"] = json!("x");
    stale["_build"] = json!("stale-build");
    let (status, body) = h.request("POST", "/api/term/send", stale.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "stale_build");
    assert_eq!(body["reload"], true);
    assert_eq!(body["build"], h.build);
    stale["data"] = Value::Null;
    stale["keys"] = json!(["escape"]);
    let (status, body) = h.request("POST", "/api/term/send", stale).await;
    ok_receipt(status, &body, 1);
    // Exactly 1 MiB (ptyhost's own send ceiling) is accepted.
    let (status, body) = h
        .send(
            "page",
            &token,
            instance,
            json!({"data":"a".repeat(1024 * 1024)}),
        )
        .await;
    ok_receipt(status, &body, 1024 * 1024);

    // There is no per-second input count: bursts over the former limit
    // all reach the same leased host.
    for index in 0..32 {
        let (status, body) = h
            .send("page", &token, instance, json!({"keys":["escape"]}))
            .await;
        assert_eq!(status, StatusCode::OK, "request {index}: {body}");
    }
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["ctrl-u","enter"]}))
        .await;
    ok_receipt(status, &body, 2);

    // Scroll: no server-side position for ptyhost; the browser xterm scrolls.
    for input in [
        json!({"name":NAME,"up":true,"lines":3}),
        json!({"name":NAME,"up":false,"lines":30}),
        json!({"name":NAME,"cancel":true}),
    ] {
        let (status, body) = h.request("POST", "/api/term/scroll", input).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!({"pos":0,"scrollback":"browser"}));
    }
    let (status, body) = h
        .request(
            "POST",
            "/api/term/scroll",
            json!({"name":"absent-terminal","up":true}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "terminal_missing");
    for input in [
        json!({"name":NAME,"up":true,"lines":0}),
        json!({"name":NAME,"page":"x"}),
    ] {
        let (status, body) = h.request("POST", "/api/term/scroll", input).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!({"pos":0,"scrollback":"browser"}));
    }
    // The shell is still the same process and still answers on the socket.
    let (status, body) = h
        .send("page", &token, instance, json!({"data":"ping"}))
        .await;
    ok_receipt(status, &body, 4);
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["Enter"]}))
        .await;
    ok_receipt(status, &body, 1);
    read_until(&mut ws, "RS_PING_OK").await;
}

#[tokio::test]
async fn input_requires_the_exact_current_lease_and_stops_after_revoke_or_exit() {
    let Some(binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let h = Harness::new().await;
    let instance = "synthetic-input-instance-0002";
    let mut host = RealHost::spawn(&binary, &h, instance).await;

    // No lease at all: 403 with the ownership reason, no host effect.
    let (status, body) = h
        .send("page", &"0".repeat(64), instance, json!({"keys":["enter"]}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "terminal_ownership");
    assert!(body["error"].as_str().unwrap().contains("控制权已失效"));

    let token = h.claim("page", instance, false).await;
    let mut ws = h.connect("page", &token, instance).await;
    read_until(&mut ws, "RS_SHELL_READY").await;
    // Wrong page, raw downgrade, other instance or malformed token: refused.
    let (status, body) = h
        .send("other-page", &token, instance, json!({"keys":["enter"]}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("接管或重新预约"));
    let (status, body) = h
        .request(
            "POST",
            "/api/term/send",
            json!({"name":NAME,"page":"page","token":token,"keys":["enter"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("不能降级"));
    let (status, body) = h
        .send(
            "page",
            &token,
            "synthetic-input-instance-9999",
            json!({"keys":["enter"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = h
        .send("page", "not-hex", instance, json!({"keys":["enter"]}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "terminal_ownership");
    let (status, body) = h
        .send(
            "page",
            &token,
            instance,
            json!({"keys":["enter"],"launch_id":"x"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "terminal_binding_unavailable");
    // The right lease still works and the shell still answers.
    let (status, body) = h
        .send("page", &token, instance, json!({"data":"ping"}))
        .await;
    ok_receipt(status, &body, 4);
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["enter"]}))
        .await;
    ok_receipt(status, &body, 1);
    read_until(&mut ws, "RS_PING_OK").await;

    // Another page forces the lease: the old socket is revoked and the old
    // credential is refused for HTTP input, while the new reservation may
    // already type before it attaches.
    let taker = h.claim("taker", instance, true).await;
    let (_, close) = read_until(&mut ws, "\u{0}never").await;
    assert_eq!(close.unwrap().0, 4001);
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["enter"]}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "terminal_ownership");
    assert!(body["error"].as_str().unwrap().contains("接管"));
    let (status, body) = h
        .send("taker", &taker, instance, json!({"data":"ping"}))
        .await;
    ok_receipt(status, &body, 4);
    let (status, body) = h
        .send("taker", &taker, instance, json!({"keys":["enter"]}))
        .await;
    ok_receipt(status, &body, 1);
    let mut ws = h.connect("taker", &taker, instance).await;
    // Replay reconstructs the screen, so the reply typed before attaching is visible.
    read_until(&mut ws, "RS_PING_OK").await;

    // Host exit through the shell's own quit: the bridge reports it, releases
    // the lease, and further input is refused with no lease left to use.
    let (status, body) = h
        .send("taker", &taker, instance, json!({"data":"quit"}))
        .await;
    ok_receipt(status, &body, 4);
    let (status, body) = h
        .send("taker", &taker, instance, json!({"keys":["enter"]}))
        .await;
    ok_receipt(status, &body, 1);
    let (text, close) = read_until(&mut ws, "\u{0}never").await;
    assert!(text.contains("RS_SHELL_DONE"), "{text:?}");
    assert_eq!(close, Some((1000, "host exited".into())));
    host.wait_exit().await;
    let (status, body) = h
        .send("taker", &taker, instance, json!({"keys":["enter"]}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "terminal_ownership");
    assert!(host.child.try_wait().unwrap().unwrap().success());
}

/// The conversation view's Esc, question cards and Grok text arrive with an
/// empty token and the pinned identity (`term.js termRowBinding`): written
/// while nobody holds the lease, the documented ownership conflict while any
/// page does, never a name-only write.
#[tokio::test]
async fn a_page_without_a_lease_writes_under_the_ordinary_claimant_rule() {
    let Some(binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let h = Harness::new().await;
    let instance = "synthetic-input-instance-0004";
    let mut host = RealHost::spawn(&binary, &h, instance).await;

    // Grok's composer: a bracketed paste, then Enter as a separate request.
    let (status, body) = h.send("page", "", instance, json!({"paste":"ping"})).await;
    ok_receipt(status, &body, 4);
    let (status, body) = h
        .send("page", "", instance, json!({"keys":["enter"]}))
        .await;
    ok_receipt(status, &body, 1);
    // No identity or the wrong instance: refused before any host write.
    let (status, body) = h
        .request(
            "POST",
            "/api/term/send",
            json!({"name":NAME,"page":"page","token":"","keys":["enter"],"_build":h.build}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "terminal_binding_unavailable");
    let (status, body) = h
        .send(
            "page",
            "",
            "synthetic-input-instance-9999",
            json!({"keys":["enter"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "terminal_binding_unavailable");

    // Nothing lingers: the console claims without conflict, and replay shows
    // the shell answered the lease-less submit.
    let token = h.claim("page", instance, false).await;
    let mut ws = h.connect("page", &token, instance).await;
    read_until(&mut ws, "RS_PING_OK").await;

    // While a page holds the lease, another page without one is refused with
    // the owner, exactly as a reliable send would be; the holder keeps typing.
    let (status, body) = h
        .send("other", "", instance, json!({"keys":["enter"]}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "terminal_ownership");
    assert!(
        body["error"].as_str().unwrap().contains("其他页面持有"),
        "{body}"
    );
    let (status, body) = h
        .send("page", &token, instance, json!({"data":"ping"}))
        .await;
    ok_receipt(status, &body, 4);
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["enter"]}))
        .await;
    ok_receipt(status, &body, 1);
    read_until(&mut ws, "RS_PING_OK").await;

    // Closing the console releases the lease; the lease-less path writes again
    // and the shell's own quit ends the host.
    drop(ws);
    timeout(Duration::from_secs(10), async {
        loop {
            let (status, body) = h.send("page", "", instance, json!({"data":"quit"})).await;
            if status == StatusCode::OK {
                ok_receipt(status, &body, 4);
                break;
            }
            assert_eq!(body["code"], "terminal_ownership", "{body}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the bound lease must be released after the socket closes");
    let (status, body) = h
        .send("page", "", instance, json!({"keys":["enter"]}))
        .await;
    ok_receipt(status, &body, 1);
    host.wait_exit().await;
    let (status, body) = h
        .send("page", "", instance, json!({"keys":["enter"]}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "terminal_binding_unavailable");
    assert!(host.child.try_wait().unwrap().unwrap().success());
}

#[tokio::test]
async fn reservation_input_reports_a_gone_instance_after_exit() {
    let Some(binary) = ptyhost_binary() else {
        eprintln!("SKIP: build the local ptyhost target first (cargo build -p ptyhost)");
        return;
    };
    let h = Harness::new().await;
    let instance = "synthetic-input-instance-0003";
    let mut host = RealHost::spawn(&binary, &h, instance).await;
    // An unexpired reservation authorizes input without a WebSocket.
    let token = h.claim("page", instance, false).await;
    let (status, body) = h
        .send("page", &token, instance, json!({"data":"quit"}))
        .await;
    ok_receipt(status, &body, 4);
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["enter"]}))
        .await;
    ok_receipt(status, &body, 1);
    host.wait_exit().await;
    // The lease still exists but the exact instance is gone: 410, not 404.
    let (status, body) = h
        .send("page", &token, instance, json!({"keys":["enter"]}))
        .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert_eq!(body["code"], "terminal_exited");
    let (_, list) = h.request("GET", "/api/term/list", Value::Null).await;
    assert_eq!(list["sessions"], json!([]));
}

#[tokio::test]
async fn transport_off_keeps_send_and_scroll_unimplemented() {
    let directory = tempfile::tempdir().unwrap();
    let app = app_with_shutdown(
        Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: SessionRoots {
                claude: None,
                codex: Some(directory.path().to_owned()),
                grok: None,
            },
            ..Default::default()
        },
        CancellationToken::new(),
    )
    .unwrap();
    for (path, body) in [
        (
            "/api/term/send",
            json!({"name":NAME,"page":"p","token":"t","keys":["enter"]}),
        ),
        ("/api/term/scroll", json!({"name":NAME,"up":true})),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("host", "127.0.0.1")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED, "{path}");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "terminal_disabled");
    }
    let request = Request::builder()
        .uri("/api/meta")
        .header("host", "127.0.0.1")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let meta: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(meta["capabilities"]["terminal_input"], false);
}

/// Loopback fake peer with a guard-capable record: answers Info, and for the
/// guarded input either stays silent (ambiguous) or rejects it.
struct FakePeer {
    requests: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl FakePeer {
    async fn new(directory: &Path, uid: &str, instance: &str, reject: bool) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let record = json!({"name":NAME,"host_pid":41,"pid":42,"created":10,"cols":80,"rows":24,
            "port":listener.local_addr().unwrap().port(),"token":"PRIVATE_PEER_TOKEN","argv":["PRIVATE_ARGV"],
            "meta":{"source":"codex","sid":SID,"uid":uid,"instance_id":instance}});
        tokio::fs::write(
            directory.join(format!("{NAME}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .await
        .unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let task = {
            let requests = requests.clone();
            tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let (record, requests) = (record.clone(), requests.clone());
                    tokio::spawn(async move {
                        let mut line = Vec::new();
                        while let Ok(byte) = socket.read_u8().await {
                            if byte == b'\n' {
                                break;
                            }
                            line.push(byte);
                            assert!(line.len() < 65536);
                        }
                        let request: Value = serde_json::from_slice(&line).unwrap();
                        assert_eq!(request["token"], "PRIVATE_PEER_TOKEN");
                        if request["op"] == "info" {
                            let reply = json!({"ok":true,"info":record,"exited":false,"capabilities":{"instance_guard":1}});
                            let _ = socket.write_all(format!("{reply}\n").as_bytes()).await;
                            return;
                        }
                        assert_eq!(request["op"], "guarded_v1", "input must stay guarded");
                        assert_eq!(
                            request["expected_instance_id"],
                            record["meta"]["instance_id"]
                        );
                        assert_eq!(request["request"]["op"], "keys");
                        requests.fetch_add(1, Ordering::SeqCst);
                        if reject {
                            let _ = socket
                                .write_all(b"{\"ok\":false,\"error\":\"guard rejected\"}\n")
                                .await;
                            return;
                        }
                        // Ambiguous: the write may have happened; never acknowledge.
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    });
                }
            })
        };
        Self { requests, task }
    }
}

impl Drop for FakePeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn unacknowledged_or_rejected_guarded_input_is_reported_once_and_never_retried() {
    for reject in [false, true] {
        let h = Harness::new().await;
        let instance = "synthetic-fake-instance-0004";
        let peer = FakePeer::new(&h.hosts, &h.uid, instance, reject).await;
        let token = h.claim("page", instance, false).await;
        let started = std::time::Instant::now();
        let (status, body) = h
            .send("page", &token, instance, json!({"keys":["enter"]}))
            .await;
        if reject {
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            assert_eq!(body["code"], "terminal_identity");
        } else {
            assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{body}");
            assert_eq!(body["code"], "terminal_input_ambiguous");
            assert!(started.elapsed() < Duration::from_secs(12));
        }
        assert_eq!(
            peer.requests.load(Ordering::SeqCst),
            1,
            "exactly one submission"
        );
        // The lease itself is untouched by an ambiguous or rejected write.
        let (status, body) = h
            .send("page", &token, instance, json!({"keys":["enter"]}))
            .await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(peer.requests.load(Ordering::SeqCst), 2);
    }
}
