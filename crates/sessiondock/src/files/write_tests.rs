//! Synthetic-only write-side tests: authorization boundaries, TOCTOU, atomic
//! no-overwrite, chunk offsets, trash and expiry. No native homes or CLIs.
use super::*;
use serde_json::{Value, json};
use std::{fs, path::PathBuf, time::Duration};

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
        self.write.join(name).to_str().unwrap().to_owned()
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
fn names_paths_and_roots_are_explicit_boundaries() {
    let fixture = Fixture::new();
    for name in [
        "..",
        ".",
        "a/b",
        "/abs",
        "",
        "a\0b",
        "bad\nname",
        ".sessiondock-x",
    ] {
        let (status, code) =
            fixture.act_err(json!({"action":"mkdir","destination":fixture.w(""),"name":name}));
        assert_eq!(status, 400, "{name:?} {code}");
    }
    #[cfg(not(windows))]
    {
        let (status, code) =
            fixture.act_err(json!({"action":"mkdir","destination":fixture.w(""),"name":"a\\b"}));
        assert_eq!((status, code), (400, "file_foreign_path"));
        let (status, code) =
            fixture.act_err(json!({"action":"mkdir","destination":"C:\\temp","name":"x"}));
        assert_eq!((status, code), (400, "file_foreign_path"));
    }
    // Relative and `..` write paths are refused before any root lookup.
    assert_eq!(
        fixture
            .act_err(json!({"action":"mkdir","destination":"sub","name":"x"}))
            .0,
        400
    );
    assert_eq!(
        fixture.act_err(
            json!({"action":"mkdir","destination":format!("{}/sub/..", fixture.w("")),"name":"x"})
        ),
        (400, "file_path_invalid")
    );
    assert_eq!(
        fixture.act_err(json!({"action":"delete","paths":[format!("{}/../w/sub", fixture.w(""))]})),
        (400, "file_path_invalid")
    );
    // Inside the read root but not inside a write root: read roots never
    // become writable implicitly.
    assert_eq!(
        fixture.act_err(json!({"action":"mkdir","destination":fixture.root.join("readonly").to_str().unwrap(),"name":"x"})),
        (403, "file_outside_write_roots")
    );
    // Outside the anchor's read root entirely.
    let outside = tempfile::tempdir().unwrap();
    assert_eq!(
        fixture
            .act_err(
                json!({"action":"mkdir","destination":outside.path().to_str().unwrap(),"name":"x"})
            )
            .0,
        403
    );
    // The reserved staging name cannot be targeted through paths either; a
    // legacy `.agenthub-*` directory is an ordinary (deletable) entry now.
    assert_eq!(
        fixture.act_err(json!({"action":"delete","paths":[fixture.w(UPLOAD_DIR)]})),
        (400, "file_reserved_name")
    );
    assert_eq!(
        fixture.act_err(json!({"action":"delete","paths":[fixture.w(".agenthub-trash")]})),
        (404, "file_not_found")
    );
    assert_eq!(
        fixture.act_err(json!({"action":"delete","paths":[fixture.w("")]})),
        (403, "file_root_immutable")
    );
    assert_eq!(
        fixture.act_err(
            json!({"action":"mkdir","destination":fixture.w(""),"name":"x","conflict":"replace"})
        ),
        (400, "file_conflict_replace_unsupported")
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
            501,
            "{action}"
        );
    }
    assert_eq!(fixture.act_err(json!({"action":"format"})).0, 400);
    assert!(
        fixture
            .root
            .join("readonly")
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
    assert!(outside.path().read_dir().unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn symlinks_inside_root_are_refused_as_destination_source_and_leaf() {
    let fixture = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), fixture.write.join("escape")).unwrap();
    fs::write(outside.path().join("victim.txt"), b"outside").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("victim.txt"),
        fixture.write.join("victim-link"),
    )
    .unwrap();
    assert_eq!(
        fixture.act_err(json!({"action":"mkdir","destination":fixture.w("escape"),"name":"x"})),
        (403, "file_symlink_forbidden")
    );
    assert_eq!(
        fixture.act_err(
            json!({"action":"upload","destination":fixture.w("escape"),"name":"x","size":1})
        ),
        (403, "file_symlink_forbidden")
    );
    assert_eq!(
        fixture.act_err(json!({"action":"rename","paths":[fixture.w("escape")],"name":"renamed"})),
        (403, "file_symlink_forbidden")
    );
    assert_eq!(
        fixture.act_err(json!({"action":"delete","paths":[fixture.w("victim-link")]})),
        (403, "file_symlink_forbidden")
    );
    assert_eq!(
        fixture.act_err(
            json!({"action":"move","paths":[fixture.w("sub")],"destination":fixture.w("escape")})
        ),
        (403, "file_symlink_forbidden")
    );
    assert!(outside.path().join("x").symlink_metadata().is_err());
    assert_eq!(
        fs::read(outside.path().join("victim.txt")).unwrap(),
        b"outside"
    );
    assert!(
        fixture
            .write
            .join("escape")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
    assert!(fixture.trash_entries().is_empty());
}

#[cfg(unix)]
#[test]
fn multiply_linked_files_are_readable_but_never_moved_renamed_or_deleted() {
    let mut fixture = Fixture::new();
    fs::write(fixture.write.join("shared.txt"), b"shared").unwrap();
    fs::hard_link(
        fixture.write.join("shared.txt"),
        fixture.write.join("alias.txt"),
    )
    .unwrap();
    fixture
        .messages
        .push(json!({"role":"assistant","text":"`alias.txt`"}));
    // Read side: an ordinary file for listing and reading.
    let target = fixture
        .reader
        .target(&fixture.scope(), "alias.txt", None)
        .unwrap();
    assert_eq!(target.kind(), "file");
    let listing = fixture
        .reader
        .list(&fixture.anchor(), &ListOptions::default())
        .unwrap();
    assert!(
        listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "alias.txt" && row["kind"] == "file")
    );
    // Write side: neither alias may be detached from the shared data.
    for body in [
        json!({"action":"rename","paths":[fixture.w("alias.txt")],"name":"renamed.txt"}),
        json!({"action":"move","paths":[fixture.w("alias.txt")],"destination":fixture.w("sub")}),
        json!({"action":"delete","paths":[fixture.w("shared.txt")]}),
    ] {
        assert_eq!(fixture.act_err(body), (403, "file_hardlink_forbidden"));
    }
    assert_eq!(
        fs::read(fixture.write.join("alias.txt")).unwrap(),
        b"shared"
    );
    assert_eq!(
        fs::read(fixture.write.join("shared.txt")).unwrap(),
        b"shared"
    );
    assert!(fixture.trash_entries().is_empty());
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
fn root_replaced_mid_job_is_refused_and_nothing_lands_in_the_replacement() {
    let fixture = Fixture::new();
    let job = fixture.start_upload("moved-root.bin", 4, json!({}));
    let id = job["id"].as_str().unwrap().to_owned();
    assert_eq!(fixture.upload(&id, 0, b"ab").unwrap().status, 200);
    let old = fixture.root.join("w-old");
    fs::rename(&fixture.write, &old).unwrap();
    fs::create_dir(&fixture.write).unwrap();
    let error = fixture.upload(&id, 2, b"cd").err().unwrap();
    assert_eq!(error.code, "file_changed");
    assert_eq!(
        fixture.act_err(json!({"action":"mkdir","destination":fixture.w(""),"name":"x"})),
        (409, "file_changed")
    );
    assert!(fixture.write.read_dir().unwrap().next().is_none());
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
    assert_eq!(
        fixture.act_err(json!({"action":"move","paths":[fixture.w("dest/a.txt")],"destination":fixture.root.join("w2").to_str().unwrap()})),
        (403, "file_move_cross_root")
    );
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
    assert_eq!(job["completed"][0]["trash"]["path"], file.to_str().unwrap());
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
fn limits_expiry_cancel_and_scope_binding_are_explicit() {
    let fixture = Fixture::with_limits(WriteLimits {
        max_jobs: 2,
        max_job_bytes: 8,
        max_chunk_bytes: 4,
        expiry: Duration::from_millis(50),
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
    let error = fixture
        .act(json!({"action":"upload","destination":fixture.w(""),"name":"three","size":8}))
        .err()
        .unwrap();
    assert_eq!(
        (error.status, error.code, error.details["limit"].as_u64()),
        (429, "file_jobs_limit", Some(2))
    );
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
    let error = fixture.act(json!({"action":"delete","paths":(0..257).map(|i| fixture.w(&format!("f{i}"))).collect::<Vec<_>>()})).err().unwrap();
    assert_eq!((error.status, error.code), (413, "file_items_limit"));
    fixture.writer.registry().expire_all();
    let error = fixture
        .upload(third["id"].as_str().unwrap(), 4, b"efgh")
        .err()
        .unwrap();
    assert_eq!((error.status, error.code), (410, "file_job_expired"));
    assert!(
        fixture
            .write
            .join(UPLOAD_DIR)
            .read_dir()
            .unwrap()
            .next()
            .is_none(),
        "expired staging removed"
    );
    assert_eq!(
        fixture.writer.jobs(&fixture.scope())["jobs"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        fixture.upload(&"0".repeat(32), 0, b"").err().unwrap().code,
        "file_job_unknown"
    );
    // Roots must be explicit and non-overlapping; limits must be positive.
    assert_eq!(
        WriteService::open(vec![], WriteLimits::default(), None)
            .err()
            .unwrap()
            .code,
        "file_write_roots_required"
    );
    assert_eq!(
        WriteService::open(
            vec![fixture.write.clone(), fixture.write.join("sub")],
            WriteLimits::default(),
            None
        )
        .err()
        .unwrap()
        .code,
        "file_write_roots_overlap"
    );
    assert_eq!(
        WriteService::open(
            vec![fixture.write.clone()],
            WriteLimits {
                max_jobs: 0,
                ..Default::default()
            },
            None
        )
        .err()
        .unwrap()
        .code,
        "file_write_limits_invalid"
    );
    // The trash lives in the private state directory, never inside a write root.
    assert_eq!(
        WriteService::open(
            vec![fixture.write.clone()],
            WriteLimits::default(),
            Some(fixture.write.join("sub"))
        )
        .err()
        .unwrap()
        .code,
        "file_trash_overlap"
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
    assert!(!fixture.writer.writable(&fixture.root.join("readonly")));
    assert_eq!(fixture.writer.capabilities()["chunk_bytes"], 4);
}
