//! Pure unit coverage: protection topology, manifest round trip, planning and
//! move/restore mechanics on private temporary trees. No HTTP, no host.

use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{
    Liveness, RunState, TrashService, entry_id_is_valid,
    manifest::{EntryState, FileRole, Manifest, RunStateNote, Stamp},
    plan::Plan,
    protection_set,
};
use crate::sessions::SessionRoots;

fn row(source: &str, sid: &str, uid: &str, extra: Value) -> Value {
    let mut row =
        json!({"source": source, "sid": sid, "uid": uid, "supported": true, "title": sid});
    if let Some(object) = extra.as_object() {
        for (key, value) in object {
            row[key] = value.clone();
        }
    }
    row
}

fn private_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    #[cfg(not(unix))]
    fs::create_dir(path).unwrap();
}

#[test]
fn protection_set_follows_fixed_prefix_and_supported_fork_edges_only() {
    let rows = [
        row("codex", "parent", "codex:1", json!({})),
        row(
            "codex",
            "fork",
            "codex:2",
            json!({"forked_from_id": "parent", "history_base": {"thread_id": "parent", "end_byte_offset": 10}}),
        ),
        row(
            "codex",
            "grandchild",
            "codex:3",
            json!({"forked_from_id": "fork", "history_base": {"thread_id": "fork", "end_byte_offset": 5}, "supported": false}),
        ),
        // Cross-source SIDs never link.
        row("claude", "parent", "claude:4", json!({})),
        // Unsupported orphan subagents carry ownership edges, not history.
        row(
            "codex",
            "cycle-a",
            "codex:5",
            json!({"forked_from_id": "cycle-b", "supported": false}),
        ),
        row(
            "codex",
            "cycle-b",
            "codex:6",
            json!({"forked_from_id": "cycle-a", "supported": false}),
        ),
        // Legacy-shaped supported fork without history_base still protects.
        row("codex", "legacy-parent", "codex:7", json!({})),
        row(
            "codex",
            "legacy-child",
            "codex:8",
            json!({"forked_from_id": "legacy-parent"}),
        ),
        // history_base without thread_id falls back to forked_from_id.
        row("codex", "base-parent", "codex:9", json!({})),
        row(
            "codex",
            "base-child",
            "codex:10",
            json!({"forked_from_id": "base-parent", "history_base": {"end_byte_offset": 0}}),
        ),
    ];
    let protected = protection_set(&rows);
    assert_eq!(
        protected.iter().map(String::as_str).collect::<Vec<_>>(),
        ["codex:1", "codex:2", "codex:7", "codex:9"]
    );
}

#[test]
fn liveness_without_runtime_is_unknown_and_never_running() {
    let liveness = Liveness::from_runtime(None);
    assert!(!liveness.configured);
    assert_eq!(
        liveness.state("codex:1"),
        RunState::Unknown("no_runtime".into())
    );
    let configured = Liveness {
        configured: true,
        ..Default::default()
    };
    assert_eq!(
        configured.state("codex:1"),
        RunState::Unknown("no_instance".into())
    );
}

#[test]
fn entry_ids_and_file_names_are_strict() {
    assert!(entry_id_is_valid(
        "20260912T080000Z-claude-0123456789abcdef-a1b2c3d4"
    ));
    for bad in [
        "",
        ".",
        "..",
        "-x",
        "a/b",
        "a b",
        "a\u{0}",
        &"x".repeat(129),
    ] {
        assert!(!entry_id_is_valid(bad), "{bad:?}");
    }
    assert!(super::file_name_is_valid("0-session.jsonl"));
    for bad in ["", ".hidden", "..", "a/b", "a b"] {
        assert!(!super::file_name_is_valid(bad), "{bad:?}");
    }
}

#[test]
fn manifest_round_trips_and_rejects_foreign_shapes() {
    let temp = tempfile::tempdir().unwrap();
    let entry = temp.path().join("entry");
    fs::create_dir(&entry).unwrap();
    let manifest = Manifest {
        version: 1,
        entry_id: "20260912T080000Z-claude-abcdef0123456789-01020304".into(),
        uid: "claude:abcdef0123456789".into(),
        source: "claude".into(),
        sid: "sid".into(),
        title: "标题".into(),
        cwd: "/work".into(),
        created: "2026-09-12T00:00:00Z".into(),
        updated: "2026-09-12T00:00:01Z".into(),
        origin: temp.path().join("claude/project/sid.jsonl"),
        root: temp.path().join("claude"),
        deleted_at: "2026-09-12T08:00:00Z".into(),
        deleted_at_unix: 1_789_200_000,
        state: EntryState::Trashed,
        forced: true,
        run_state: RunStateNote {
            state: "unknown".into(),
            detail: "no_runtime".into(),
        },
        bytes: 12,
        files: vec![super::manifest::FileRecord {
            name: "0-sid.jsonl".into(),
            origin: temp.path().join("claude/project/sid.jsonl"),
            role: FileRole::Data,
            stamp: Stamp {
                size: 12,
                mtime_secs: 1,
                mtime_nanos: 2,
                identity: "1:2".into(),
            },
            in_trash: true,
        }],
    };
    manifest.write(&entry).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(Manifest::path(&entry))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert!(!entry.join("manifest.json.tmp").exists());
    assert_eq!(Manifest::read(&entry).unwrap(), manifest);
    let encoded: Value =
        serde_json::from_slice(&fs::read(Manifest::path(&entry)).unwrap()).unwrap();
    assert_eq!(encoded["state"], "trashed");
    assert_eq!(encoded["files"][0]["role"], "data");

    let mut wrong_version = manifest.clone();
    wrong_version.version = 2;
    wrong_version.write(&entry).unwrap();
    assert_eq!(Manifest::read(&entry).unwrap_err().code, "manifest_invalid");
    let mut escaping = manifest.clone();
    escaping.files[0].origin = temp.path().join("elsewhere/sid.jsonl");
    escaping.write(&entry).unwrap();
    assert_eq!(Manifest::read(&entry).unwrap_err().code, "manifest_invalid");
    fs::write(Manifest::path(&entry), b"not json").unwrap();
    assert_eq!(Manifest::read(&entry).unwrap_err().code, "manifest_invalid");
    fs::remove_file(Manifest::path(&entry)).unwrap();
    assert_eq!(Manifest::read(&entry).unwrap_err().code, "manifest_missing");
}

struct Tree {
    temp: tempfile::TempDir,
    roots: SessionRoots,
}

impl Tree {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        for name in ["claude", "codex", "grok", "trash"] {
            private_dir(&temp.path().join(name));
        }
        let roots = SessionRoots {
            claude: Some(temp.path().join("claude").canonicalize().unwrap()),
            codex: Some(temp.path().join("codex").canonicalize().unwrap()),
            grok: Some(temp.path().join("grok").canonicalize().unwrap()),
        };
        Self { temp, roots }
    }

    fn service(&self) -> TrashService {
        TrashService::open(self.temp.path().join("trash"), self.roots.clone()).unwrap()
    }

    fn claude_session(&self, name: &str, agents: &[&str]) -> Value {
        let root = self.roots.claude.clone().unwrap();
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{name}.jsonl"));
        fs::write(&path, format!("{{\"sessionId\":\"{name}\"}}\n")).unwrap();
        let mut items = Vec::new();
        for agent in agents {
            let directory = project.join(name).join("subagents");
            fs::create_dir_all(&directory).unwrap();
            let agent_path = directory.join(format!("agent-{agent}.jsonl"));
            fs::write(&agent_path, b"{\"agent\":true}\n").unwrap();
            fs::write(agent_path.with_extension("meta.json"), b"{}").unwrap();
            items.push(json!({"id": agent, "path": agent_path.to_string_lossy()}));
        }
        let mut row = row(
            "claude",
            name,
            &format!("claude:{name}"),
            json!({"path": path.to_string_lossy(), "cwd": "/work"}),
        );
        if !items.is_empty() {
            row["agent_items"] = json!(items);
        }
        row
    }

    fn grok_session(&self, name: &str, chat: bool) -> Value {
        let directory = self.roots.grok.clone().unwrap().join("project").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("summary.json"), b"{\"info\":{}}").unwrap();
        if chat {
            fs::write(directory.join("chat_history.jsonl"), b"{}\n").unwrap();
        }
        fs::write(directory.join("attachment.bin"), b"keep").unwrap();
        row(
            "grok",
            name,
            &format!("grok:{name}"),
            json!({"path": directory.to_string_lossy(), "chat_exists": chat}),
        )
    }
}

fn forced() -> Liveness {
    Liveness::default()
}

#[test]
fn plan_names_exactly_the_inventory_files_and_rejects_links() {
    let tree = Tree::new();
    let row = tree.claude_session("main", &["one"]);
    let plan = Plan::derive(&row, &tree.roots).unwrap();
    let roles: Vec<_> = plan.files.iter().map(|file| file.role).collect();
    assert_eq!(
        roles,
        [FileRole::Data, FileRole::Agent, FileRole::AgentMeta]
    );
    assert_eq!(
        plan.bytes(),
        plan.files.iter().map(|f| f.stamp.size).sum::<u64>()
    );
    // WP-E (Python parity): a Grok session is its whole directory; the
    // reported bytes are the regular files below it.
    let grok = tree.grok_session("g", true);
    let plan = Plan::derive(&grok, &tree.roots).unwrap();
    assert_eq!(
        plan.files.iter().map(|file| file.role).collect::<Vec<_>>(),
        [FileRole::Directory]
    );
    assert_eq!(
        plan.files[0].origin,
        Path::new(grok["path"].as_str().unwrap())
    );
    assert_eq!(
        plan.bytes(),
        (b"{\"info\":{}}".len() + b"{}\n".len() + b"keep".len()) as u64
    );
    // A Grok directory without its summary is not a session any more.
    let mut headless = tree.grok_session("h", true);
    fs::remove_file(Path::new(headless["path"].as_str().unwrap()).join("summary.json")).unwrap();
    headless["chat_exists"] = json!(true);
    assert_eq!(
        Plan::derive(&headless, &tree.roots).unwrap_err().code,
        "changed_since_inventory"
    );
    let outside = row.clone();
    let mut outside = outside;
    outside["path"] = json!(tree.temp.path().join("codex/x.jsonl").to_string_lossy());
    assert_eq!(
        Plan::derive(&outside, &tree.roots).unwrap_err().code,
        "path_outside_root"
    );
    let mut relative = row.clone();
    relative["path"] = json!(format!(
        "{}/project/../project/main.jsonl",
        tree.roots.claude.as_ref().unwrap().display()
    ));
    assert_eq!(
        Plan::derive(&relative, &tree.roots).unwrap_err().code,
        "path_outside_root"
    );
    #[cfg(unix)]
    {
        let link = tree
            .roots
            .claude
            .clone()
            .unwrap()
            .join("project/link.jsonl");
        std::os::unix::fs::symlink(
            tree.roots
                .claude
                .clone()
                .unwrap()
                .join("project/main.jsonl"),
            &link,
        )
        .unwrap();
        let mut linked = row.clone();
        linked["path"] = json!(link.to_string_lossy());
        assert_eq!(
            Plan::derive(&linked, &tree.roots).unwrap_err().code,
            "symlink_rejected"
        );
        assert!(Stamp::capture(&link).is_err_and(|error| error.code == "not_regular_file"));
        let aliased_dir = tree.roots.claude.clone().unwrap().join("alias");
        std::os::unix::fs::symlink(
            tree.roots.claude.clone().unwrap().join("project"),
            &aliased_dir,
        )
        .unwrap();
        assert_eq!(
            super::plan::trusted_within(
                tree.roots.claude.as_ref().unwrap(),
                &aliased_dir.join("main.jsonl")
            )
            .unwrap_err()
            .code,
            "symlink_rejected"
        );
    }
}

#[test]
fn delete_restore_purge_round_trip_keeps_unnamed_files_and_refuses_conflicts() {
    let tree = Tree::new();
    let service = tree.service();
    let claude = tree.claude_session("main", &["one"]);
    let grok = tree.grok_session("g", true);
    let rows = vec![claude.clone(), grok.clone()];
    let refused = service.delete(&rows, &forced(), &["claude:main".into()], false);
    assert!(refused.deleted.is_empty());
    assert_eq!(refused.skipped[0].code, "run_state_unknown");
    assert!(refused.skipped[0].needs_force);
    let outcome = service.delete(
        &rows,
        &forced(),
        &[
            "claude:main".into(),
            "grok:g".into(),
            "claude:main".into(),
            "nope".into(),
        ],
        true,
    );
    assert_eq!(outcome.deleted.len(), 2, "{outcome:?}");
    assert_eq!(outcome.failed.len(), 1);
    assert_eq!(outcome.failed[0].code, "not_found");
    let claude_origin = Path::new(claude["path"].as_str().unwrap());
    assert!(!claude_origin.exists());
    let agents_dir = claude_origin.with_extension("").join("subagents");
    assert!(agents_dir.is_dir() && fs::read_dir(&agents_dir).unwrap().count() == 0);
    // WP-E: the whole Grok directory moved (Python parity), every file with it.
    let grok_dir = Path::new(grok["path"].as_str().unwrap());
    assert!(!grok_dir.exists(), "grok directory moves whole");
    let grok_entry = outcome.deleted.iter().find(|d| d.uid == "grok:g").unwrap();
    assert_eq!(grok_entry.files, 1);
    let held = Path::new(&grok_entry.trash).join("files").join("0-g");
    assert!(held.join("summary.json").is_file() && held.join("attachment.bin").is_file());

    let listing = service.list(10, None).unwrap();
    assert_eq!(listing.count, 2);
    assert!(listing.items.iter().all(|item| item["restorable"] == true));
    assert_eq!(
        listing.size,
        outcome.deleted.iter().map(|d| d.bytes).sum::<u64>()
    );
    let page = service.list(1, None).unwrap();
    assert_eq!(page.items.len(), 1);
    let next = service.list(1, page.next_cursor.as_deref()).unwrap();
    assert_eq!(next.items.len(), 1);
    assert!(next.next_cursor.is_none());
    assert_ne!(page.items[0]["id"], next.items[0]["id"]);
    assert_eq!(
        service.list(1, Some("missing-cursor")).unwrap_err().code,
        "invalid_cursor"
    );

    let entry = outcome
        .deleted
        .iter()
        .find(|d| d.uid == "claude:main")
        .unwrap();
    fs::write(claude_origin, b"recreated").unwrap();
    let conflict = service.restore(&entry.entry_id).unwrap_err();
    assert_eq!(conflict.code, "restore_conflict");
    assert_eq!(fs::read(claude_origin).unwrap(), b"recreated");
    let listing = service.list(10, None).unwrap();
    let item = listing
        .items
        .iter()
        .find(|item| item["uid"] == "claude:main")
        .unwrap();
    assert_eq!(item["restorable"], false);
    assert!(item["reason"].as_str().unwrap().contains("原路径已存在"));
    fs::remove_file(claude_origin).unwrap();
    // Parent directories the user removed are recreated without following links.
    fs::remove_dir_all(claude_origin.with_extension("")).unwrap();
    let restored = service.restore(&entry.entry_id).unwrap();
    assert_eq!(restored.files, 3);
    assert_eq!(
        fs::read_to_string(claude_origin).unwrap(),
        "{\"sessionId\":\"main\"}\n"
    );
    assert!(
        agents_dir.join("agent-one.jsonl").exists()
            && agents_dir.join("agent-one.meta.json").exists()
    );
    assert!(!Path::new(&entry.trash).exists());
    assert_eq!(
        service.restore(&entry.entry_id).unwrap_err().code,
        "entry_not_found"
    );
    assert_eq!(
        service.restore("../etc").unwrap_err().code,
        "invalid_entry_id"
    );

    // Restore puts the directory back whole, then a second delete + purge
    // removes it from the bin (never from the native root).
    let back = service.restore(&grok_entry.entry_id).unwrap();
    assert_eq!(back.files, 1);
    assert!(
        grok_dir.join("attachment.bin").is_file() && grok_dir.join("chat_history.jsonl").is_file()
    );
    let again = service.delete(&rows, &forced(), &["grok:g".into()], true);
    assert_eq!(again.deleted.len(), 1, "{again:?}");
    let grok_entry = &again.deleted[0];
    let purge = service
        .purge_ids(&[grok_entry.entry_id.clone(), "missing-entry".into()])
        .unwrap();
    assert_eq!(
        (purge.removed, purge.errors.len(), purge.remaining),
        (1, 1, 0)
    );
    assert_eq!(purge.freed, grok_entry.bytes);
    assert!(!grok_dir.exists());
    assert!(fs::read_dir(service.directory()).unwrap().next().is_none());
}

#[test]
fn stamp_change_between_plan_and_move_is_refused_and_rolled_back() {
    let tree = Tree::new();
    let service = tree.service();
    let claude = tree.claude_session("main", &["one", "two"]);
    let plan = Plan::derive(&claude, &tree.roots).unwrap();
    // Grow the second agent after planning: the first file must return.
    let touched = plan.files[3].origin.clone();
    fs::write(&touched, b"{\"agent\":true}\n{\"appended\":true}\n").unwrap();
    let error = service
        .move_into_trash(
            &plan,
            RunStateNote {
                state: "unknown".into(),
                detail: "no_runtime".into(),
            },
            true,
        )
        .unwrap_err();
    assert_eq!(error.code, "changed_since_inventory");
    for file in &plan.files {
        assert!(file.origin.exists(), "{}", file.origin.display());
    }
    assert!(
        fs::read_dir(service.directory()).unwrap().next().is_none(),
        "no entry left behind"
    );
}

#[test]
fn purge_by_age_is_bounded_and_skips_foreign_content() {
    let tree = Tree::new();
    let service = tree.service();
    let mut rows = Vec::new();
    for index in 0..3 {
        rows.push(tree.claude_session(&format!("s{index}"), &[]));
    }
    let uids: Vec<String> = rows
        .iter()
        .map(|row| row["uid"].as_str().unwrap().to_owned())
        .collect();
    let outcome = service.delete(&rows, &forced(), &uids, true);
    assert_eq!(outcome.deleted.len(), 3);
    // Backdate one entry; keep the others "fresh".
    let old = Path::new(&outcome.deleted[0].trash);
    let mut manifest = Manifest::read(old).unwrap();
    manifest.deleted_at_unix -= 10 * 86_400;
    manifest.write(old).unwrap();
    // A stray file inside another entry blocks its purge only.
    fs::write(Path::new(&outcome.deleted[1].trash).join("stray"), b"x").unwrap();
    let aged = service.purge_older_than(7).unwrap();
    assert_eq!((aged.removed, aged.remaining), (1, 2));
    let all = service.purge_older_than(0).unwrap();
    assert_eq!((all.removed, all.errors.len(), all.remaining), (1, 1, 1));
    assert_eq!(all.failed[0]["code"], "unexpected_content");
    assert!(Path::new(&outcome.deleted[1].trash).join("files").is_dir());
}
