//! Synthetic-only write-side tests: authorization boundaries, TOCTOU, atomic
//! no-overwrite, chunk offsets, trash and cancellation. No native homes or CLIs.
use super::*;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    write: PathBuf,
    state: PathBuf,
    reader: FileService,
    writer: WriteService,
    messages: Vec<Value>,
}
impl Fixture {
    fn new() -> Self {
        Self::with_limits(WriteLimits::default())
    }
    fn with_limits(limits: WriteLimits) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("files");
        let state = base.join("state");
        let write = root.join("w");
        fs::create_dir_all(write.join("sub")).unwrap();
        fs::create_dir_all(root.join("readonly")).unwrap();
        fs::create_dir_all(root.join("w2")).unwrap();
        fs::create_dir_all(&state).unwrap();
        let reader = FileService::open(vec![root.clone()]).unwrap();
        let writer = WriteService::open(
            vec![write.clone(), root.join("w2")],
            limits,
            Some(state.clone()),
        )
        .unwrap();
        let messages = vec![
            json!({"role":"assistant","text":format!("`{}/` `{}/`", root.display(), write.display())}),
        ];
        Self {
            _temp: temp,
            root,
            write,
            state,
            reader,
            writer,
            messages,
        }
    }
    fn scope(&self) -> FileScope<'_> {
        FileScope {
            uid: "claude:synthetic",
            agent: None,
            cwd: self.root.to_str().unwrap(),
            messages: &self.messages,
        }
    }
    fn anchor(&self) -> ResolvedTarget {
        self.reader
            .target(&self.scope(), &format!("{}/", self.write.display()), None)
            .unwrap()
    }
    fn act(&self, body: Value) -> Result<Outcome, FileError> {
        let request: ActionRequest = serde_json::from_value(body).unwrap();
        self.writer.action(&self.anchor(), &self.scope(), request)
    }
    fn act_err(&self, body: Value) -> (u16, &'static str) {
        match self.act(body) {
            Ok(outcome) => (
                outcome.status,
                Box::leak(
                    outcome.body["code"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned()
                        .into_boxed_str(),
                ),
            ),
            Err(error) => (error.status, error.code),
        }
    }
    fn upload(&self, job: &str, offset: u64, chunk: &[u8]) -> Result<Outcome, FileError> {
        self.writer
            .upload(&self.anchor(), &self.scope(), job, offset, chunk)
    }
    fn start_upload(&self, name: &str, size: u64, extra: Value) -> Value {
        let mut body = json!({"action":"upload","destination":self.write.to_str().unwrap(),"name":name,"size":size});
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        let outcome = self.act(body).unwrap();
        assert_eq!(outcome.status, 200, "{}", outcome.body);
        outcome.body["job"].clone()
    }
    fn w(&self, name: &str) -> String {
        boundary::wire_path(&self.write.join(name)).unwrap()
    }
    fn trash_dir(&self) -> PathBuf {
        self.state.join(FILE_TRASH_DIR)
    }
    fn trash_entries(&self) -> Vec<PathBuf> {
        let trash = self.trash_dir();
        if !trash.exists() {
            return vec![];
        }
        let mut found = vec![];
        for item in fs::read_dir(trash).unwrap() {
            for entry in fs::read_dir(item.unwrap().path()).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().unwrap() != "manifest.json" {
                    found.push(path);
                }
            }
        }
        found
    }
}

#[test]
fn names_are_validated_but_configured_directories_do_not_jail_writes() {
    let fixture = Fixture::new();
    for name in ["..", ".", "a/b", "a\\b", "/abs", "", "a\0b"] {
        assert_eq!(
            fixture
                .act_err(json!({"action":"mkdir","destination":fixture.w(""),"name":name}))
                .0,
            400
        );
    }
    for destination in [fixture.root.join("readonly"), fixture.write.join("sub/..")] {
        let result = fixture
            .act(json!({"action":"mkdir","destination":destination,"name":"allowed"}))
            .unwrap();
        assert_eq!(result.status, 200, "{}", result.body);
    }
    let outside = tempfile::tempdir().unwrap();
    let result = fixture.act(json!({"action":"new-file","destination":outside.path(),"name":".sessiondock-ordinary"})).unwrap();
    assert_eq!(result.status, 200, "{}", result.body);
    assert!(outside.path().join(".sessiondock-ordinary").is_file());
    assert_eq!(
        fixture
            .act_err(json!({"action":"delete","paths":[fixture.state]}))
            .0,
        403
    );
    #[cfg(unix)]
    assert_eq!(
        fixture.act_err(json!({"action":"delete","paths":["/"]})).0,
        403
    );
    for action in [
        "copy", "compress", "extract", "bundle", "restore", "purge", "retry", "trash",
    ] {
        assert_eq!(
            fixture
                .act_err(
                    json!({"action":action,"paths":[fixture.w("sub")],"destination":fixture.w("")})
                )
                .0,
            501
        );
    }
}

#[cfg(unix)]
#[test]
fn writes_resolve_symlink_parents_but_rename_and_trash_the_leaf_link() {
    let fixture = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), fixture.write.join("escape")).unwrap();
    fs::write(outside.path().join("victim.txt"), b"outside").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("victim.txt"),
        fixture.write.join("victim-link"),
    )
    .unwrap();
    let created = fixture
        .act(json!({"action":"mkdir","destination":fixture.w("escape"),"name":"x"}))
        .unwrap();
    assert_eq!(created.status, 200, "{}", created.body);
    assert!(outside.path().join("x").is_dir());
    let renamed = fixture
        .act(json!({"action":"rename","paths":[fixture.w("escape")],"name":"renamed"}))
        .unwrap();
    assert_eq!(renamed.status, 200, "{}", renamed.body);
    assert!(
        fixture
            .write
            .join("renamed")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
    let deleted = fixture
        .act(json!({"action":"delete","paths":[fixture.w("victim-link")]}))
        .unwrap();
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert!(!fixture.write.join("victim-link").exists());
    assert_eq!(
        fs::read(outside.path().join("victim.txt")).unwrap(),
        b"outside"
    );
    assert_eq!(fixture.trash_entries().len(), 1);
}

#[cfg(unix)]
#[test]
fn moving_and_deleting_a_hardlink_alias_preserves_the_other_alias() {
    let fixture = Fixture::new();
    fs::write(fixture.write.join("shared.txt"), b"shared").unwrap();
    fs::hard_link(
        fixture.write.join("shared.txt"),
        fixture.write.join("alias.txt"),
    )
    .unwrap();
    for body in [
        json!({"action":"rename","paths":[fixture.w("alias.txt")],"name":"renamed.txt"}),
        json!({"action":"move","paths":[fixture.w("renamed.txt")],"destination":fixture.w("sub")}),
        json!({"action":"delete","paths":[fixture.w("sub/renamed.txt")]}),
    ] {
        let result = fixture.act(body).unwrap();
        assert_eq!(result.status, 200, "{}", result.body);
        assert_eq!(
            fs::read(fixture.write.join("shared.txt")).unwrap(),
            b"shared"
        );
    }
    assert_eq!(fixture.trash_entries().len(), 1);
}

#[test]
fn replace_preserves_old_destination_in_trash_and_large_upload_defaults_match_python() {
    let fixture = Fixture::new();
    fs::write(fixture.write.join("source.txt"), b"new content").unwrap();
    fs::write(fixture.write.join("destination.txt"), b"old content").unwrap();
    let result = fixture
        .act(json!({"action":"rename","paths":[fixture.w("source.txt")],
        "name":"destination.txt","conflict":"replace"}))
        .unwrap();
    assert_eq!(result.status, 200, "{}", result.body);
    assert_eq!(
        fs::read(fixture.write.join("destination.txt")).unwrap(),
        b"new content"
    );
    let trash = fixture.trash_entries();
    assert_eq!(trash.len(), 1);
    assert_eq!(fs::read(&trash[0]).unwrap(), b"old content");
    let large = fixture.start_upload("large.bin", 1024 * 1024 * 1024 * 1024, json!({}));
    assert_eq!(large["state"], "uploading");
    assert_eq!(fixture.writer.limits().max_chunk_bytes, 8 * 1024 * 1024);
}

#[cfg(unix)]
#[test]
fn dangling_symlink_rename_preserves_link_without_following_its_missing_target() {
    let fixture = Fixture::new();
    let missing = fixture.write.join("does-not-exist");
    std::os::unix::fs::symlink(&missing, fixture.write.join("dangling")).unwrap();
    let result = fixture
        .act(json!({"action":"rename","paths":[fixture.w("dangling")],"name":"renamed"}))
        .unwrap();
    assert_eq!(result.status, 200, "{}", result.body);
    assert_eq!(
        fs::read_link(fixture.write.join("renamed")).unwrap(),
        missing
    );
    assert!(!missing.exists());
}

#[cfg(unix)]
#[test]
fn component_swapped_for_symlink_between_resolve_and_write_is_detected() {
    let outside = tempfile::tempdir().unwrap();
    let outside_path = outside.path().to_path_buf();
    // mkdir into `sub` after `sub` became a link to an outside directory.
    let fixture = Fixture::new();
    let sub = fixture.write.join("sub");
    let swap = {
        let sub = sub.clone();
        let outside_path = outside_path.clone();
        move || {
            fs::rename(&sub, sub.with_file_name("sub-moved")).unwrap();
            std::os::unix::fs::symlink(&outside_path, &sub).unwrap();
        }
    };
    *fixture.writer.before_write.lock().unwrap() = Some(Box::new(swap));
    assert_eq!(
        fixture.act_err(json!({"action":"mkdir","destination":fixture.w("sub"),"name":"created"})),
        (409, "file_changed")
    );
    assert!(outside_path.join("created").symlink_metadata().is_err());
    assert!(
        fixture
            .write
            .join("sub-moved/created")
            .symlink_metadata()
            .is_err()
    );
    *fixture.writer.before_write.lock().unwrap() = None;

    // Upload chunk after the destination directory was swapped for a link.
    let fixture = Fixture::new();
    let job = fixture.start_upload("late.bin", 4, json!({}));
    assert_eq!(
        fixture
            .upload(job["id"].as_str().unwrap(), 0, b"ab")
            .unwrap()
            .status,
        200
    );
    let sub = fixture.write.clone();
    let swap = {
        let outside_path = outside_path.clone();
        move || {
            fs::rename(&sub, sub.with_file_name("w-moved")).unwrap();
            std::os::unix::fs::symlink(&outside_path, &sub).unwrap();
        }
    };
    *fixture.writer.before_write.lock().unwrap() = Some(Box::new(swap));
    let error = fixture
        .upload(job["id"].as_str().unwrap(), 2, b"cd")
        .err()
        .unwrap();
    assert_eq!(error.code, "file_changed");
    assert!(outside_path.join("late.bin").symlink_metadata().is_err());
    assert!(outside_path.join(UPLOAD_DIR).symlink_metadata().is_err());
}

#[test]
#[cfg(not(windows))]
fn existing_upload_rejects_replacement_but_new_requests_can_use_the_new_directory() {
    let fixture = Fixture::new();
    let job = fixture.start_upload("moved-root.bin", 4, json!({}));
    let id = job["id"].as_str().unwrap().to_owned();
    assert_eq!(fixture.upload(&id, 0, b"ab").unwrap().status, 200);
    let old = fixture.root.join("w-old");
    fs::rename(&fixture.write, &old).unwrap();
    fs::create_dir(&fixture.write).unwrap();
    let error = fixture.upload(&id, 2, b"cd").err().unwrap();
    assert_eq!(error.code, "file_changed");
    let fresh = fixture
        .act(json!({"action":"mkdir","destination":fixture.w(""),"name":"x"}))
        .unwrap();
    assert_eq!(fresh.status, 200, "{}", fresh.body);
    assert!(fixture.write.join("x").is_dir());
    assert!(!fixture.write.join("moved-root.bin").exists());
    fs::remove_dir(fixture.write.join("x")).unwrap();
    fs::remove_dir(&fixture.write).unwrap();
    fs::rename(&old, &fixture.write).unwrap();
    assert_eq!(fixture.upload(&id, 2, b"cd").unwrap().status, 200);
    assert_eq!(
        fs::read(fixture.write.join("moved-root.bin")).unwrap(),
        b"abcd"
    );
}

#[test]
fn chunk_offsets_are_idempotent_for_replay_and_conflicting_otherwise() {
    let fixture = Fixture::with_limits(WriteLimits {
        max_chunk_bytes: 4,
        ..Default::default()
    });
    let data = b"0123456789";
    use sha2::Digest;
    let digest = format!("{:x}", sha2::Sha256::digest(data));
    let job = fixture.start_upload(
        "chunked.bin",
        10,
        json!({"sha256": digest, "modified": 1700000000.5}),
    );
    assert_eq!(job["state"], "uploading");
    assert_eq!(job["total_bytes"], 10);
    assert_eq!(job["upload_modified"], 1700000000.5);
    let id = job["id"].as_str().unwrap().to_owned();
    let first = fixture.upload(&id, 0, &data[..4]).unwrap();
    assert_eq!(first.body["job"]["bytes"], 4);
    // Same chunk again: accepted without change.
    let replay = fixture.upload(&id, 0, &data[..4]).unwrap();
    assert_eq!(
        (replay.status, replay.body["job"]["bytes"].as_u64()),
        (200, Some(4))
    );
    // Replay with different bytes, a gap, or a partial rewind is a conflict.
    for (offset, chunk) in [
        (0u64, b"XXXX".as_slice()),
        (8, &data[8..]),
        (2, &data[2..4]),
        (1, &data[1..4]),
    ] {
        let error = fixture.upload(&id, offset, chunk).err().unwrap();
        assert_eq!(
            (error.status, error.code),
            (409, "file_upload_offset"),
            "{offset}"
        );
        assert_eq!(error.details["expected"], 4);
    }
    let error = fixture.upload(&id, 4, b"12345").err().unwrap();
    assert_eq!(
        (error.status, error.code),
        (413, "file_upload_chunk_too_large")
    );
    assert_eq!(error.details["limit"], 4);
    assert_eq!(
        fixture.upload(&id, 4, &data[4..8]).unwrap().body["job"]["bytes"],
        8
    );
    let error = fixture.upload(&id, 8, b"890").err().unwrap();
    assert_eq!((error.status, error.code), (413, "file_upload_overflow"));
    let done = fixture.upload(&id, 8, &data[8..]).unwrap();
    assert_eq!(done.status, 200, "{}", done.body);
    assert_eq!(done.body["job"]["state"], "completed");
    assert_eq!(done.body["job"]["path"], fixture.w("chunked.bin"));
    assert_eq!(done.body["job"]["sha256"], digest);
    assert_eq!(fs::read(fixture.write.join("chunked.bin")).unwrap(), data);
    assert!(
        fixture
            .write
            .join(UPLOAD_DIR)
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(fixture.write.join(UPLOAD_DIR)).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(fixture.write.join("chunked.bin"))
                .unwrap()
                .nlink(),
            1
        );
    }
    // Final chunk replay after completion stays idempotent; anything else 409.
    assert_eq!(fixture.upload(&id, 8, &data[8..]).unwrap().status, 200);
    assert_eq!(
        fixture.upload(&id, 0, &data[..4]).err().unwrap().code,
        "file_upload_finished"
    );
    let listed = fixture.writer.jobs(&fixture.scope());
    assert_eq!(listed["jobs"][0]["id"], id);
    assert!(listed["jobs"][0].get("scope").is_none());

    // Declared digest mismatch discards the staging data explicitly.
    let job = fixture.start_upload("wrong.bin", 2, json!({"sha256": digest}));
    let outcome = fixture
        .upload(job["id"].as_str().unwrap(), 0, b"zz")
        .unwrap();
    assert_eq!(
        (outcome.status, outcome.body["code"].as_str()),
        (409, Some("file_upload_checksum"))
    );
    assert_eq!(outcome.body["job"]["state"], "failed");
    assert!(fixture.write.join("wrong.bin").symlink_metadata().is_err());
    assert!(
        fixture
            .write
            .join(UPLOAD_DIR)
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
    // Empty declared size publishes immediately and the browser's zero-length
    // chunk is an idempotent no-op.
    let job = fixture.start_upload("empty.bin", 0, json!({}));
    assert_eq!(job["state"], "completed");
    assert_eq!(
        fixture
            .upload(job["id"].as_str().unwrap(), 0, b"")
            .unwrap()
            .status,
        200
    );
    assert_eq!(fs::read(fixture.write.join("empty.bin")).unwrap(), b"");
    let other = FileScope {
        uid: "claude:other",
        ..fixture.scope()
    };
    assert_eq!(
        fixture
            .writer
            .upload(&fixture.anchor(), &other, &id, 8, &data[8..])
            .err()
            .unwrap()
            .code,
        "file_job_unknown"
    );
    assert_eq!(fixture.upload("nothex", 0, b"").err().unwrap().status, 400);
}

#[test]
fn nothing_is_ever_overwritten() {
    let fixture = Fixture::new();
    fs::write(fixture.write.join("taken.txt"), b"keep me").unwrap();
    fs::create_dir(fixture.write.join("taken-dir")).unwrap();
    fs::write(fixture.write.join("taken-dir/inner.txt"), b"inner").unwrap();
    fs::write(fixture.write.join("source.txt"), b"source").unwrap();
    fs::create_dir(fixture.write.join("source-dir")).unwrap();
    // Creation onto an existing name.
    assert_eq!(
        fixture.act_err(json!({"action":"mkdir","destination":fixture.w(""),"name":"taken.txt"})),
        (409, "file_exists")
    );
    assert_eq!(
        fixture
            .act_err(json!({"action":"new-file","destination":fixture.w(""),"name":"taken-dir"})),
        (409, "file_exists")
    );
    // Upload: fail fast at creation, and again at publish if the name appears later.
    assert_eq!(
        fixture.act_err(
            json!({"action":"upload","destination":fixture.w(""),"name":"taken.txt","size":1})
        ),
        (409, "file_exists")
    );
    let job = fixture.start_upload("appears.txt", 3, json!({}));
    fs::write(fixture.write.join("appears.txt"), b"first").unwrap();
    let outcome = fixture
        .upload(job["id"].as_str().unwrap(), 0, b"new")
        .unwrap();
    assert_eq!(
        (outcome.status, outcome.body["code"].as_str()),
        (409, Some("file_exists"))
    );
    assert_eq!(outcome.body["job"]["state"], "failed");
    assert_eq!(
        fs::read(fixture.write.join("appears.txt")).unwrap(),
        b"first"
    );
    // Rename/move of files and directories onto existing names.
    assert_eq!(
        fixture.act_err(
            json!({"action":"rename","paths":[fixture.w("source.txt")],"name":"taken.txt"})
        ),
        (409, "file_exists")
    );
    assert_eq!(
        fixture.act_err(
            json!({"action":"rename","paths":[fixture.w("source-dir")],"name":"taken-dir"})
        ),
        (409, "file_exists")
    );
    assert_eq!(
        fixture.act_err(
            json!({"action":"rename","paths":[fixture.w("source-dir")],"name":"taken.txt"})
        ),
        (409, "file_exists")
    );
    assert_eq!(
        fixture.act_err(
            json!({"action":"rename","paths":[fixture.w("source.txt")],"name":"taken-dir"})
        ),
        (409, "file_exists")
    );
    fs::create_dir(fixture.write.join("sub/source.txt")).unwrap();
    assert_eq!(fixture.act_err(json!({"action":"move","paths":[fixture.w("source.txt")],"destination":fixture.w("sub")})), (409, "file_exists"));
    assert_eq!(
        fs::read(fixture.write.join("taken.txt")).unwrap(),
        b"keep me"
    );
    assert_eq!(
        fs::read(fixture.write.join("taken-dir/inner.txt")).unwrap(),
        b"inner"
    );
    assert_eq!(
        fs::read(fixture.write.join("source.txt")).unwrap(),
        b"source"
    );
    // keep → numbered sibling; skip → explicit skipped item.
    let outcome = fixture.act(json!({"action":"rename","paths":[fixture.w("source.txt")],"name":"taken.txt","conflict":"keep"})).unwrap();
    assert_eq!(
        outcome.body["job"]["completed"][0]["renamed"],
        fixture.w("taken (1).txt")
    );
    assert_eq!(
        fs::read(fixture.write.join("taken (1).txt")).unwrap(),
        b"source"
    );
    let outcome = fixture.act(json!({"action":"mkdir","destination":fixture.w(""),"name":"taken-dir","conflict":"skip"})).unwrap();
    assert_eq!(outcome.body["job"]["completed"][0]["skipped"], true);
    let outcome = fixture.act(json!({"action":"mkdir","destination":fixture.w(""),"name":"taken-dir","conflict":"keep"})).unwrap();
    assert_eq!(
        outcome.body["job"]["completed"][0]["created"],
        fixture.w("taken-dir (1)")
    );
    assert!(fixture.write.join("taken-dir (1)").is_dir());
}

#[test]
fn actions_move_within_one_root_and_delete_into_root_trash_with_partial_results() {
    let fixture = Fixture::new();
    fs::write(fixture.write.join("a.txt"), b"a").unwrap();
    fs::write(fixture.write.join("sub/b.txt"), b"b").unwrap();
    let outcome = fixture.act(json!({"action":"move","paths":[fixture.w("a.txt"), fixture.w("sub")],"destination":fixture.w("")})).unwrap();
    // `sub` into its own parent is the same path; `a.txt` likewise.
    assert_eq!(outcome.status, 400, "{}", outcome.body);
    fs::create_dir(fixture.write.join("dest")).unwrap();
    let outcome = fixture.act(json!({"action":"move","paths":[fixture.w("a.txt"), fixture.w("sub"), fixture.w("sub/b.txt"), fixture.w("missing")],"destination":fixture.w("dest")})).unwrap();
    assert_eq!(outcome.status, 200);
    let job = &outcome.body["job"];
    assert_eq!(job["state"], "failed");
    assert_eq!(
        job["items_total"], 3,
        "child of a selected parent is dropped"
    );
    assert_eq!(job["completed"].as_array().unwrap().len(), 2);
    assert_eq!(job["errors"][0]["path"], fixture.w("missing"));
    assert_eq!(job["errors"][0]["status"], 404);
    assert_eq!(fs::read(fixture.write.join("dest/a.txt")).unwrap(), b"a");
    assert_eq!(
        fs::read(fixture.write.join("dest/sub/b.txt")).unwrap(),
        b"b"
    );
    assert_eq!(
        fixture.act_err(json!({"action":"move","paths":[fixture.w("dest/sub")],"destination":fixture.w("dest/sub")})),
        (400, "file_move_into_self")
    );
    let result = fixture.act(json!({"action":"move","paths":[fixture.w("dest/a.txt")],"destination":fixture.root.join("w2")})).unwrap();
    assert_eq!(result.status, 200, "{}", result.body);
    let result = fixture.act(json!({"action":"move","paths":[fixture.root.join("w2/a.txt")],"destination":fixture.w("dest")})).unwrap();
    assert_eq!(result.status, 200, "{}", result.body);
    let before = fs::read(fixture.write.join("dest/a.txt")).unwrap();
    let outcome = fixture.act(json!({"action":"delete","paths":[fixture.w("dest/a.txt"), fixture.w("dest/sub"), fixture.w("nope")]})).unwrap();
    assert_eq!(outcome.status, 200);
    let job = &outcome.body["job"];
    assert_eq!(job["state"], "failed");
    assert_eq!(job["completed"].as_array().unwrap().len(), 2);
    assert_eq!(job["errors"].as_array().unwrap().len(), 1);
    let trashed = fixture.trash_entries();
    assert_eq!(trashed.len(), 2, "{trashed:?}");
    let file = trashed
        .iter()
        .find(|p| p.file_name().unwrap() == "a.txt")
        .unwrap();
    assert_eq!(fs::read(file).unwrap(), before);
    assert!(file.starts_with(fixture.trash_dir()));
    assert!(!fixture.write.join(".sessiondock-trash").exists());
    assert!(!fixture.write.join(".agenthub-trash").exists());
    let manifest: Value =
        serde_json::from_slice(&fs::read(file.parent().unwrap().join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["path"], fixture.w("dest/a.txt"));
    assert_eq!(manifest["uid"], "claude:synthetic");
    assert_eq!(
        job["completed"][0]["trash"]["path"],
        boundary::wire_path(file).unwrap()
    );
    assert_eq!(
        fs::read(
            trashed
                .iter()
                .find(|p| p.file_name().unwrap() == "sub")
                .unwrap()
                .join("b.txt")
        )
        .unwrap(),
        b"b"
    );
    assert!(fixture.write.join("dest/a.txt").symlink_metadata().is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(fixture.trash_dir()).unwrap().mode() & 0o777,
            0o700
        );
    }
    // A single failing item reports its own status; the job is still listed.
    assert_eq!(
        fixture.act_err(json!({"action":"delete","paths":[fixture.w("nope")]})),
        (404, "file_not_found")
    );
    let jobs = fixture.writer.jobs(&fixture.scope());
    assert!(jobs["jobs"].as_array().unwrap().len() >= 4);
    assert!(
        jobs["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|job| job["state"] != "uploading")
    );
}

#[test]
fn upload_sizes_cancel_and_scope_binding_match_file_manager() {
    let fixture = Fixture::with_limits(WriteLimits {
        max_job_bytes: 8,
        max_chunk_bytes: 4,
    });
    let error = fixture
        .act(json!({"action":"upload","destination":fixture.w(""),"name":"big","size":9}))
        .err()
        .unwrap();
    assert_eq!(
        (error.status, error.code, error.details["limit"].as_u64()),
        (413, "file_job_too_large", Some(8))
    );
    let first = fixture.start_upload("one", 8, json!({}));
    let _second = fixture.start_upload("two", 8, json!({}));
    assert_eq!(
        fixture.write.join(UPLOAD_DIR).read_dir().unwrap().count(),
        2
    );
    let cancelled = fixture
        .act(json!({"action":"cancel","job":first["id"]}))
        .unwrap();
    assert_eq!(cancelled.body["job"]["state"], "cancelled");
    assert_eq!(
        fixture.write.join(UPLOAD_DIR).read_dir().unwrap().count(),
        1
    );
    assert_eq!(
        fixture
            .upload(first["id"].as_str().unwrap(), 0, b"x")
            .err()
            .unwrap()
            .code,
        "file_upload_finished"
    );
    let third = fixture.start_upload("three", 8, json!({}));
    assert_eq!(
        fixture
            .upload(third["id"].as_str().unwrap(), 0, b"abcd")
            .unwrap()
            .status,
        200
    );
    let error = fixture.act(json!({"action":"delete","paths":(0..2001).map(|i| fixture.w(&format!("f{i}"))).collect::<Vec<_>>()})).err().unwrap();
    assert_eq!((error.status, error.code), (413, "file_items_limit"));
    fixture
        .upload(third["id"].as_str().unwrap(), 4, b"efgh")
        .unwrap();
    assert_eq!(
        fixture.writer.jobs(&fixture.scope())["jobs"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        fixture.upload(&"0".repeat(32), 0, b"").err().unwrap().code,
        "file_job_unknown"
    );
    assert!(WriteService::open(vec![], WriteLimits::default(), None).is_ok());
    assert!(
        WriteService::open(
            vec![fixture.write.clone(), fixture.write.join("sub")],
            WriteLimits::default(),
            None
        )
        .is_ok()
    );
    assert_eq!(
        WriteService::open(
            vec![fixture.write.clone()],
            WriteLimits {
                max_chunk_bytes: 0,
                ..Default::default()
            },
            None
        )
        .err()
        .unwrap()
        .code,
        "file_write_limits_invalid"
    );
    // Roots enable the feature; actual private-state guards protect mutations.
    assert!(
        WriteService::open(
            vec![fixture.write.clone()],
            WriteLimits::default(),
            Some(fixture.write.join("sub"))
        )
        .is_ok()
    );
    let without_trash =
        WriteService::open(vec![fixture.write.clone()], WriteLimits::default(), None).unwrap();
    assert!(
        !without_trash.capabilities()["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action == "delete")
    );
    fs::write(fixture.write.join("keep.txt"), b"keep").unwrap();
    let request: ActionRequest =
        serde_json::from_value(json!({"action":"delete","paths":[fixture.w("keep.txt")]})).unwrap();
    let outcome = without_trash
        .action(&fixture.anchor(), &fixture.scope(), request)
        .unwrap();
    assert_eq!(outcome.status, 501, "{}", outcome.body);
    assert_eq!(outcome.body["code"], "file_trash_unconfigured");
    assert_eq!(fs::read(fixture.write.join("keep.txt")).unwrap(), b"keep");
    assert!(
        fixture.writer.capabilities()["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action == "delete")
    );
    assert!(fixture.writer.writable(&fixture.write.join("anything")));
    assert!(fixture.writer.writable(&fixture.root.join("readonly")));
    assert_eq!(fixture.writer.capabilities()["chunk_bytes"], 4);
}

#[cfg(target_os = "linux")]
#[test]
fn cross_device_moves_and_trash_preserve_trees_links_and_recoverable_originals() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    let fixture = Fixture::new();
    let destination = match tempfile::tempdir_in("/dev/shm") {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("SKIP cross-device fixture: {error}");
            return;
        }
    };
    if fs::metadata(destination.path()).unwrap().dev()
        == fs::metadata(&fixture.write).unwrap().dev()
    {
        eprintln!("SKIP: /dev/shm and fixture have the same device");
        return;
    }
    fs::create_dir_all(fixture.write.join("tree/sub")).unwrap();
    fs::write(fixture.write.join("tree/sub/note"), b"cross-device bytes").unwrap();
    fs::hard_link(
        fixture.write.join("tree/sub/note"),
        fixture.write.join("tree/hard-alias"),
    )
    .unwrap();
    fs::set_permissions(
        fixture.write.join("tree/sub/note"),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let victim = fixture.root.join("keep-target");
    fs::write(&victim, b"link target untouched").unwrap();
    symlink(&victim, fixture.write.join("tree/absolute-link")).unwrap();
    symlink("missing", fixture.write.join("tree/dangling")).unwrap();
    let moved = fixture
        .act(json!({"action":"move","paths":[fixture.w("tree")],"destination":destination.path()}))
        .unwrap();
    assert_eq!(moved.status, 200, "{}", moved.body);
    assert!(!fixture.write.join("tree").exists());
    let target = destination.path().join("tree");
    assert_eq!(
        fs::read(target.join("sub/note")).unwrap(),
        b"cross-device bytes"
    );
    assert_eq!(
        fs::metadata(target.join("sub/note"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert_eq!(fs::read_link(target.join("absolute-link")).unwrap(), victim);
    assert_eq!(
        fs::read_link(target.join("dangling")).unwrap(),
        PathBuf::from("missing")
    );
    let recovered = fixture.trash_entries();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        fs::read(recovered[0].join("sub/note")).unwrap(),
        b"cross-device bytes"
    );
    // The destination tree now lives on another device from the private trash.
    let deleted = fixture
        .act(json!({"action":"delete","paths":[target]}))
        .unwrap();
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert!(!target.exists());
    assert_eq!(fixture.trash_entries().len(), 2);
    assert_eq!(fs::read(&victim).unwrap(), b"link target untouched");
    // Ordinary file moves must also retain their original, rather than unlink it.
    fs::write(fixture.write.join("single"), b"regular original").unwrap();
    let moved = fixture
        .act(
            json!({"action":"move","paths":[fixture.w("single")],"destination":destination.path()}),
        )
        .unwrap();
    assert_eq!(moved.status, 200, "{}", moved.body);
    assert_eq!(
        fs::read(destination.path().join("single")).unwrap(),
        b"regular original"
    );
    assert!(!fixture.write.join("single").exists());
    assert!(
        fixture
            .trash_entries()
            .iter()
            .any(|p| p.file_name().unwrap() == "single"
                && fs::read(p).unwrap() == b"regular original")
    );
}

#[cfg(windows)]
#[test]
fn copied_directory_symlink_is_removed_as_a_link_without_traversing_it() {
    use std::os::windows::fs::symlink_dir;

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let source = temp.path().join("source-link");
    let destination = temp.path().join("copied-link");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("kept.txt"), b"target remains").unwrap();
    if let Err(error) = symlink_dir(&target, &source) {
        eprintln!("SKIP directory symlink unavailable: {error}");
        return;
    }

    super::write::copy_then_remove_entry_for_test(&source, &destination).unwrap();

    assert!(fs::symlink_metadata(&source).is_err());
    assert!(
        fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read(destination.join("kept.txt")).unwrap(),
        b"target remains"
    );
    assert_eq!(
        fs::read(target.join("kept.txt")).unwrap(),
        b"target remains"
    );
}

#[test]
fn attachment_above_the_old_32_mib_cap_is_saved_without_truncation() {
    let fixture = Fixture::new();
    let payload = vec![b'x'; 33 * 1024 * 1024];
    let result = fixture
        .writer
        .bug_report_upload(
            &fixture.root,
            Some("1"),
            "large.bin",
            "application/octet-stream",
            &payload,
        )
        .unwrap();
    let path = result["path"].as_str().unwrap();
    assert_eq!(fs::read(path).unwrap(), payload);
}
