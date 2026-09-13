//! Session recycle bin over the real router. Synthetic Claude/Codex/Grok files
//! copied into a private temporary tree, an explicit private trash directory,
//! and (Linux only) a loopback fake host whose child is a real `sleep` so one
//! session can be observed as verified `running`. No CLI, no native home.

use std::{
    fs,
    path::{Path, PathBuf},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use sessiondock::{
    app_with_shutdown,
    config::Config,
    sessions::SessionRoots,
    trash::{TrashService, manifest::RunStateNote},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn private_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .unwrap();
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path).unwrap();
}

fn comparable_path(path: impl AsRef<str>) -> String {
    let path = path.as_ref().replace('\\', "/");
    path.strip_prefix("//?/UNC/")
        .map(|rest| format!("//{rest}"))
        .or_else(|| path.strip_prefix("//?/").map(str::to_owned))
        .unwrap_or(path)
}

fn claude_row(
    sid: &str,
    kind: &str,
    uuid: &str,
    parent: Option<&str>,
    text: &str,
    agent: Option<&str>,
) -> Value {
    let mut row = json!({"type": kind, "uuid": uuid, "parentUuid": parent, "sessionId": sid,
        "cwd": "/workspace/demo", "timestamp": "2026-09-11T10:00:00Z", "isSidechain": agent.is_some(),
        "message": {"role": kind, "content": text}});
    if let Some(agent) = agent {
        row["agentId"] = json!(agent);
    }
    if kind == "assistant" {
        row["message"]["stop_reason"] = json!("end_turn");
    }
    row
}

fn lines(rows: &[Value]) -> Vec<u8> {
    rows.iter()
        .flat_map(|row| format!("{row}\n").into_bytes())
        .collect()
}

struct Fixture {
    temp: tempfile::TempDir,
    trash: PathBuf,
    claude_main: PathBuf,
    claude_agent: PathBuf,
    codex_parent: PathBuf,
    codex_fork: Option<PathBuf>,
    grok_dir: PathBuf,
}

impl Fixture {
    fn new(with_fork: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for source in ["claude", "codex", "grok"] {
            copy_tree(&Path::new(FIXTURES).join(source), &root.join(source));
        }
        let trash = root.join("trash");
        private_dir(&trash);
        let claude_main = root.join("claude/project-demo/synthetic-claude.jsonl");
        let agents = root.join("claude/project-demo/synthetic-claude/subagents");
        fs::create_dir_all(&agents).unwrap();
        let claude_agent = agents.join("agent-alpha.jsonl");
        fs::write(
            &claude_agent,
            lines(&[
                claude_row(
                    "synthetic-claude",
                    "user",
                    "agent-u",
                    None,
                    "Agent question",
                    Some("alpha"),
                ),
                claude_row(
                    "synthetic-claude",
                    "assistant",
                    "agent-a",
                    Some("agent-u"),
                    "Agent answer",
                    Some("alpha"),
                ),
            ]),
        )
        .unwrap();
        fs::write(
            claude_agent.with_extension("meta.json"),
            b"{\"description\":\"Synthetic child\",\"agentType\":\"reviewer\"}",
        )
        .unwrap();
        let codex_parent = root.join("codex/2026/09/11/rollout-synthetic-codex.jsonl");
        let codex_fork = with_fork.then(|| {
            let cut = fs::metadata(&codex_parent).unwrap().len();
            let path = root.join("codex/2026/09/11/rollout-synthetic-fork.jsonl");
            fs::write(&path, lines(&[
                json!({"type":"session_meta","timestamp":"2026-09-11T11:30:00.000Z","payload":{
                    "id":"synthetic-fork","session_id":"synthetic-fork","timestamp":"2026-09-11T11:30:00.000Z",
                    "cwd":"/workspace/demo","thread_source":"user","forked_from_id":"synthetic-codex",
                    "history_mode":"paginated","history_base":{"thread_id":"synthetic-codex","end_byte_offset":cut}}}),
                json!({"type":"response_item","timestamp":"2026-09-11T11:30:01.000Z","payload":{"type":"message","role":"user",
                    "content":[{"type":"input_text","text":"Fork question"}]}}),
            ])).unwrap();
            path
        });
        let grok_dir = root.join("grok/project-demo/synthetic-grok");
        fs::write(grok_dir.join("attachment.bin"), b"unnamed attachment stays").unwrap();
        Self {
            temp,
            trash,
            claude_main,
            claude_agent,
            codex_parent,
            codex_fork,
            grok_dir,
        }
    }

    fn roots(&self) -> SessionRoots {
        SessionRoots {
            claude: Some(self.temp.path().join("claude")),
            codex: Some(self.temp.path().join("codex")),
            grok: Some(self.temp.path().join("grok")),
        }
    }

    fn config(&self, host: Option<PathBuf>) -> Config {
        Config {
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            roots: self.roots(),
            ptyhost_dir: host,
            trash_dir: Some(self.trash.clone()),
            ..Config::default()
        }
    }

    fn app(&self) -> Router {
        app_with_shutdown(self.config(None), CancellationToken::new()).unwrap()
    }

    fn entries(&self) -> Vec<PathBuf> {
        let mut entries: Vec<_> = fs::read_dir(&self.trash)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        entries
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    fn walk(directory: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), files);
            } else {
                files.push((entry.path(), fs::read(entry.path()).unwrap()));
            }
        }
    }
    for source in ["claude", "codex", "grok"] {
        walk(&root.join(source), &mut files);
    }
    files.sort();
    files
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "127.0.0.1:8741");
    let body = match body {
        Some(value) => {
            builder = builder.header("Content-Type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response: Response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes)}))
    };
    (status, value)
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    call(app, Method::GET, uri, None).await
}

async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    call(app, Method::POST, uri, Some(body)).await
}

async fn delete(app: &Router, uri: &str) -> (StatusCode, Value) {
    call(app, Method::DELETE, uri, None).await
}

async fn sessions(app: &Router) -> Vec<Value> {
    let (status, body) = get(app, "/api/sessions").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["sessions"].as_array().cloned().unwrap()
}

async fn uid_of(app: &Router, sid: &str) -> String {
    sessions(app)
        .await
        .iter()
        .find(|row| row["sid"] == sid)
        .unwrap_or_else(|| panic!("session {sid} listed"))["uid"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn unconfigured_routes_stay_501_and_capability_is_false() {
    let fixture = Fixture::new(true);
    let mut config = fixture.config(None);
    config.trash_dir = None;
    let app = app_with_shutdown(config, CancellationToken::new()).unwrap();
    let (_, meta) = get(&app, "/api/meta").await;
    assert_eq!(meta["capabilities"]["trash"], false);
    let uid = uid_of(&app, "synthetic-claude").await;
    for (method, uri, body) in [
        (Method::DELETE, format!("/api/session/{uid}"), None),
        (
            Method::POST,
            "/api/sessions/delete".into(),
            Some(json!({"uids": [uid]})),
        ),
        (Method::GET, "/api/trash".into(), None),
        (
            Method::POST,
            "/api/trash/restore".into(),
            Some(json!({"id": "x"})),
        ),
        (
            Method::POST,
            "/api/trash/purge".into(),
            Some(json!({"all": true})),
        ),
    ] {
        let (status, value) = call(&app, method.clone(), &uri, body).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{method} {uri}");
        assert_eq!(value["code"], "not_implemented");
    }
    assert!(fixture.entries().is_empty());
}

#[test]
fn configuration_accepts_ordinary_trash_paths() {
    let fixture = Fixture::new(false);
    fixture.config(None).validate().unwrap();
    let mut inside = fixture.config(None);
    inside.trash_dir = Some(fixture.temp.path().join("claude"));
    inside.validate().unwrap();
    let nested = fixture.temp.path().join("claude/nested-trash");
    private_dir(&nested);
    inside.trash_dir = Some(nested);
    inside.validate().unwrap();
    let mut missing = fixture.config(None);
    missing.trash_dir = Some(fixture.temp.path().join("absent"));
    missing.validate().unwrap();
    assert!(
        fixture.temp.path().join("absent").is_dir(),
        "ordinary trash directories are created when absent"
    );
    let mut relative = fixture.config(None);
    relative.trash_dir = Some(PathBuf::from("."));
    relative.validate().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let open = fixture.temp.path().join("open-trash");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = fixture.config(None);
        config.trash_dir = Some(open);
        config.validate().unwrap();
    }
}

#[tokio::test]
async fn delete_list_restore_purge_round_trip_for_all_three_sources() {
    let fixture = Fixture::new(false);
    let before = snapshot(fixture.temp.path());
    let app = fixture.app();
    let (_, meta) = get(&app, "/api/meta").await;
    assert_eq!(meta["capabilities"]["trash"], true);
    let claude = uid_of(&app, "synthetic-claude").await;
    let codex = uid_of(&app, "synthetic-codex").await;
    let grok = uid_of(&app, "synthetic-grok").await;
    let rows = sessions(&app).await;
    let claude_row = rows.iter().find(|row| row["uid"] == claude).unwrap();
    assert_eq!(
        claude_row["agent_items"].as_array().unwrap().len(),
        1,
        "agent sidecar indexed"
    );

    let (status, body) = delete(&app, &format!("/api/session/{claude}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["files"], 3);
    assert_eq!(body["forced"], false);
    let entry_id = body["entry_id"].as_str().unwrap().to_owned();
    let entry_dir = PathBuf::from(body["trash"].as_str().unwrap());
    assert_eq!(entry_dir, fixture.trash.join(&entry_id));
    assert!(!fixture.claude_main.exists() && !fixture.claude_agent.exists());
    assert!(!fixture.claude_agent.with_extension("meta.json").exists());
    assert!(
        fixture.claude_agent.parent().unwrap().is_dir(),
        "directories are never removed"
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(entry_dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["state"], "trashed");
    assert_eq!(manifest["uid"], claude);
    assert_eq!(manifest["title"], claude_row["title"]);
    assert_eq!(manifest["files"].as_array().unwrap().len(), 3);
    assert_eq!(
        comparable_path(manifest["files"][0]["origin"].as_str().unwrap()),
        comparable_path(fixture.claude_main.to_string_lossy())
    );
    assert!(manifest["files"][0]["stamp"]["size"].as_u64().unwrap() > 0);
    assert!(fs::read_dir(entry_dir.join("files")).unwrap().count() == 3);
    // The published list excludes the session at once, without ?force=1.
    assert!(sessions(&app).await.iter().all(|row| row["uid"] != claude));
    let (status, again) = delete(&app, &format!("/api/session/{claude}?force=1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{again}");
    assert_eq!(again["code"], "not_found");

    // Codex rollout; the Grok session moves as a whole directory (WP-E,
    // Python parity) — attachment included, nothing left in the native root.
    let (status, body) = delete(&app, &format!("/api/session/{codex}?force=1")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"], 1);
    assert!(!fixture.codex_parent.exists());
    let (status, body) = delete(&app, &format!("/api/session/{grok}?force=1")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"], 1);
    assert!(!fixture.grok_dir.exists(), "grok directory moved whole");
    let grok_entry_dir = fixture.trash.join(body["entry_id"].as_str().unwrap());
    let grok_manifest: Value =
        serde_json::from_slice(&fs::read(grok_entry_dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(grok_manifest["files"][0]["role"], "directory");
    let held = grok_entry_dir
        .join("files")
        .join(grok_manifest["files"][0]["name"].as_str().unwrap());
    assert_eq!(
        fs::read(held.join("attachment.bin")).unwrap(),
        b"unnamed attachment stays"
    );
    assert!(held.join("summary.json").is_file() && held.join("chat_history.jsonl").is_file());
    assert!(sessions(&app).await.is_empty());

    // Listing: newest first, titles from manifests, pagination bounded.
    let (status, listing) = get(&app, "/api/trash").await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(listing["count"], 3);
    assert_eq!(
        comparable_path(listing["dir"].as_str().unwrap()),
        comparable_path(fixture.trash.canonicalize().unwrap().to_string_lossy())
    );
    let items = listing["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    for item in items {
        assert_eq!(item["restorable"], true, "{item}");
        assert!(!item["title"].as_str().unwrap().is_empty());
        assert!(item["deleted_at"].as_str().unwrap().ends_with('Z'));
        assert!(item["bytes"].as_u64().unwrap() > 0 && item["size"] == item["bytes"]);
        assert_eq!(item["id"], item["entry_id"]);
    }
    let sum: u64 = items
        .iter()
        .map(|item| item["bytes"].as_u64().unwrap())
        .sum();
    assert_eq!(listing["size"], sum);
    assert_eq!(items[0]["uid"], grok, "{items:?}");
    assert_eq!(items[0]["kind"], "dir");
    assert_eq!(items[2]["uid"], claude);
    let (status, page) = get(&app, "/api/trash?limit=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();
    let (_, rest) = get(&app, &format!("/api/trash?limit=2&cursor={cursor}")).await;
    assert_eq!(rest["items"].as_array().unwrap().len(), 1);
    assert!(rest["next_cursor"].is_null());
    assert_eq!(rest["items"][0]["uid"], claude);
    for bad in ["/api/trash?cursor=../x", "/api/trash?cursor=unknown-entry"] {
        let (status, body) = get(&app, bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} {body}");
    }

    // Restore the Claude session: all three files return, list shows it again.
    let (status, restored) = post(&app, "/api/trash/restore", json!({"id": entry_id})).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["uid"], claude);
    assert_eq!(restored["files"], 3);
    assert_eq!(
        comparable_path(restored["path"].as_str().unwrap()),
        comparable_path(fixture.claude_main.to_string_lossy())
    );
    assert!(!entry_dir.exists());
    assert!(sessions(&app).await.iter().any(|row| row["uid"] == claude));
    let (status, missing) = post(&app, "/api/trash/restore", json!({"id": entry_id})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    let (status, _) = post(&app, "/api/trash/restore", json!({"id": "../escape"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Purge by age keeps fresh entries; explicit id and all:true remove them.
    let (status, aged) = post(&app, "/api/trash/purge", json!({"days": 30})).await;
    assert_eq!(status, StatusCode::OK, "{aged}");
    assert_eq!(
        (aged["removed"].as_u64(), aged["remaining"].as_u64()),
        (Some(0), Some(2))
    );
    let (_, listing) = get(&app, "/api/trash").await;
    let codex_entry = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["uid"] == codex)
        .unwrap()["id"]
        .clone();
    let (status, purged) = post(&app, "/api/trash/purge", json!({"id": codex_entry})).await;
    assert_eq!(status, StatusCode::OK, "{purged}");
    assert_eq!(purged["removed"], 1);
    assert!(purged["freed"].as_u64().unwrap() > 0);
    let (status, gone) = post(&app, "/api/trash/purge", json!({"id": codex_entry})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{gone}");
    let (status, _) = post(&app, "/api/trash/purge", json!({"id": "x", "days": 1})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, all) = post(&app, "/api/trash/purge", json!({"all": true})).await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(
        (all["removed"].as_u64(), all["remaining"].as_u64()),
        (Some(1), Some(0))
    );
    assert!(fixture.entries().is_empty());
    assert!(!fixture.codex_parent.exists() && !fixture.grok_dir.exists());
    // The restored Claude files are byte-identical to the originals.
    let after = snapshot(fixture.temp.path());
    for (path, bytes) in &before {
        if path.starts_with(fixture.temp.path().join("claude")) {
            assert_eq!(
                after.iter().find(|(p, _)| p == path).map(|(_, b)| b),
                Some(bytes),
                "{}",
                path.display()
            );
        }
    }
}

#[tokio::test]
async fn fork_parent_is_refused_and_batches_report_partial_results() {
    let fixture = Fixture::new(true);
    let app = fixture.app();
    let parent = uid_of(&app, "synthetic-codex").await;
    let fork = uid_of(&app, "synthetic-fork").await;
    let (status, body) = delete(&app, &format!("/api/session/{parent}?force=1")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "fork_parent_protected");
    assert!(fixture.codex_parent.exists());

    // Same batch: the child goes, the parent stays protected by the frozen
    // set even though its child is already gone, a missing uid fails, and
    // the whole answer is still 200.
    let (status, body) = post(
        &app,
        "/api/sessions/delete",
        json!({"uids": [fork.clone(), parent.clone(), "codex:0000000000000000", fork.clone()]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["deleted"].as_array().unwrap().len(), 1);
    assert_eq!(body["deleted"][0]["uid"], fork);
    assert_eq!(body["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(body["skipped"][0]["uid"], parent);
    assert_eq!(body["skipped"][0]["code"], "fork_parent_protected");
    assert_eq!(body["failed"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["errors"].as_array().unwrap().len(),
        2,
        "legacy errors = skipped + failed"
    );
    assert!(fixture.codex_parent.exists());
    assert!(!fixture.codex_fork.as_ref().unwrap().exists());
    let listed = sessions(&app).await;
    assert!(listed.iter().any(|row| row["uid"] == parent));
    assert!(listed.iter().all(|row| row["uid"] != fork));
    // With the child gone (and a fresh request), the parent is deletable.
    let (status, body) = delete(&app, &format!("/api/session/{parent}?force=1")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for invalid in [json!({"uids": []}), json!({}), json!({"uids": [" "]})] {
        let (status, body) = post(&app, "/api/sessions/delete", invalid.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid} {body}");
    }
    let many: Vec<String> = (0..201).map(|i| format!("codex:{i:016x}")).collect();
    let (status, body) = post(&app, "/api/sessions/delete", json!({"uids": many})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["failed"].as_array().unwrap().len(), 201);
}

#[tokio::test]
async fn changed_contents_move_and_restore_conflicts_preserve_files() {
    let fixture = Fixture::new(false);
    let app = fixture.app();
    let claude = uid_of(&app, "synthetic-claude").await;
    let original = fs::read(&fixture.claude_main).unwrap();

    // A late append remains part of the entry selected for deletion.
    let service = TrashService::open(fixture.trash.clone(), fixture.roots()).unwrap();
    let rows = sessions(&app).await;
    let row = rows.iter().find(|row| row["uid"] == claude).unwrap();
    let plan = service.plan_for(row).unwrap();
    assert_eq!(plan.files.len(), 3);
    let mut appended = original.clone();
    appended.extend_from_slice(b"{\"type\":\"user\",\"uuid\":\"late\",\"parentUuid\":\"claude-final-1\",\"sessionId\":\"synthetic-claude\",\"message\":{\"role\":\"user\",\"content\":\"late\"}}\n");
    fs::write(&fixture.claude_main, &appended).unwrap();
    let moved = service
        .move_planned(
            &plan,
            RunStateNote {
                state: "unknown".into(),
                detail: "no_runtime".into(),
            },
            true,
        )
        .unwrap();
    assert!(!fixture.claude_main.exists() && !fixture.claude_agent.exists());
    let entry_id = moved.entry_id;
    drop(service);

    // Restore conflict: a new file at the original path is never overwritten.
    fs::write(&fixture.claude_main, b"{\"type\":\"user\",\"uuid\":\"new\",\"parentUuid\":null,\"sessionId\":\"synthetic-claude\",\"message\":{\"role\":\"user\",\"content\":\"new\"}}\n").unwrap();
    let (status, conflict) = post(&app, "/api/trash/restore", json!({"id": entry_id})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(conflict["code"], "restore_conflict");
    assert!(conflict["error"].as_str().unwrap().contains("原路径已存在"));
    assert!(
        fs::read(&fixture.claude_main)
            .unwrap()
            .starts_with(b"{\"type\":\"user\",\"uuid\":\"new\"")
    );
    assert!(!fixture.claude_agent.exists(), "no partial restore");
    let (_, listing) = get(&app, "/api/trash").await;
    assert_eq!(listing["items"][0]["restorable"], false);
    assert!(
        listing["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("原路径已存在")
    );
    // Remove the conflict: the entry restores including the agent sidecars.
    fs::remove_file(&fixture.claude_main).unwrap();
    let (status, restored) = post(&app, "/api/trash/restore", json!({"id": entry_id})).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(fs::read(&fixture.claude_main).unwrap(), appended);
    assert!(
        fixture.claude_agent.exists() && fixture.claude_agent.with_extension("meta.json").exists()
    );
}

#[cfg(target_os = "linux")]
mod running {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    struct Sleeper(std::process::Child);
    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    async fn serving_host(directory: &Path, meta: Value, pid: u32) -> tokio::task::JoinHandle<()> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let record = json!({"name":"synthetic","host_pid":std::process::id(),"pid":pid,"created":created,
            "cols":80,"rows":24,"port":listener.local_addr().unwrap().port(),
            "token":"SYNTHETIC_SECRET","meta":meta,"argv":["PRIVATE_ARGUMENT"],"cwd":"/synthetic","cmd":"synthetic"});
        tokio::fs::write(
            directory.join("synthetic.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .await
        .unwrap();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let record = record.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    loop {
                        let Ok(byte) = stream.read_u8().await else {
                            return;
                        };
                        if byte == b'\n' {
                            break;
                        }
                        bytes.push(byte);
                    }
                    let mut reply = serde_json::to_vec(&json!({"ok":true,"info":record,"exited":false,"capabilities":{"instance_guard":1}})).unwrap();
                    reply.push(b'\n');
                    let _ = stream.write_all(&reply).await;
                });
            }
        })
    }

    #[tokio::test]
    async fn verified_running_session_is_refused_even_with_force() {
        let fixture = Fixture::new(false);
        let host = tempfile::tempdir().unwrap();
        let child = Sleeper(
            std::process::Command::new("sleep")
                .arg("60")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let server = serving_host(
            host.path(),
            json!({"source":"codex","sid":"synthetic-codex","instance_id":"synthetic-instance-0001"}),
            child.0.id(),
        )
        .await;
        let app = app_with_shutdown(
            fixture.config(Some(host.path().into())),
            CancellationToken::new(),
        )
        .unwrap();
        let codex = uid_of(&app, "synthetic-codex").await;
        let claude = uid_of(&app, "synthetic-claude").await;
        let (_, live) = get(&app, "/api/live").await;
        assert_eq!(
            live["managed"]["sessions"][&codex]["state"], "running",
            "{live}"
        );
        let (status, body) = delete(&app, &format!("/api/session/{codex}?force=1")).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], "session_running");
        assert_eq!(body["run_state"]["state"], "running");
        assert_eq!(body["run_state"]["detail"], "host_info");
        assert!(fixture.codex_parent.exists());
        let (status, body) = post(
            &app,
            "/api/sessions/delete",
            json!({"uids": [codex.clone(), claude.clone()]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["deleted"][0]["uid"], claude);
        assert_eq!(body["skipped"][0]["code"], "session_running");
        server.abort();
        drop(child);
    }
}
