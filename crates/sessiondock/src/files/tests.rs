use super::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    service: FileService,
    messages: Vec<Value>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let service = FileService::open(vec![root.clone()]).unwrap();
        Self {
            _temp: temp,
            root,
            service,
            messages: vec![],
        }
    }
    fn write(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }
    fn mention(&mut self, text: impl Into<String>) {
        self.messages
            .push(json!({"role":"assistant","text":text.into()}));
    }
    fn scope(&self) -> FileScope<'_> {
        FileScope {
            uid: "claude:synthetic",
            agent: None,
            cwd: self.root.to_str().unwrap(),
            messages: &self.messages,
        }
    }
    fn target(&self, reference: &str) -> ResolvedTarget {
        self.service.target(&self.scope(), reference, None).unwrap()
    }
    fn read(&self, reference: &str, options: ReadOptions) -> FileResponse {
        self.service.read(self.target(reference), &options).unwrap()
    }
}
fn bytes(response: FileResponse) -> Vec<u8> {
    match response.body {
        FileBody::Empty => vec![],
        FileBody::Bytes(data) => data,
        FileBody::Reader(mut reader) => {
            let mut data = Vec::new();
            reader.read_to_end(&mut data).unwrap();
            data
        }
    }
}

#[test]
fn only_explicit_existing_roots_and_semantic_mentions_grant_access() {
    let mut fixture = Fixture::new();
    let secret = fixture.write("unmentioned.txt", b"synthetic private content");
    assert_eq!(
        fixture
            .service
            .target(&fixture.scope(), secret.to_str().unwrap(), None)
            .err()
            .unwrap()
            .code,
        "file_not_referenced"
    );
    fixture
        .messages
        .push(json!({"role":"native_metadata","text":secret.to_str().unwrap()}));
    assert_eq!(
        fixture
            .service
            .target(&fixture.scope(), secret.to_str().unwrap(), None)
            .err()
            .unwrap()
            .status,
        404
    );
    fixture.mention("`unmentioned.txt`");
    assert_eq!(
        bytes(fixture.read("unmentioned.txt", ReadOptions::default())),
        b"synthetic private content"
    );
    assert!(FileService::open(vec![]).is_err());
    assert!(FileService::open(vec![fixture.root.join("missing")]).is_err());
    assert!(!fixture.root.join("missing").exists());
    assert!(FileService::open(vec![fixture.root.clone(), fixture.root.clone()]).is_err());
    #[cfg(unix)]
    assert_eq!(
        FileService::open(vec![PathBuf::from("/")])
            .err()
            .unwrap()
            .code,
        "file_root_too_broad"
    );
}

#[test]
fn selected_agent_and_branch_do_not_share_reference_authority() {
    let mut fixture = Fixture::new();
    fixture.write("parent.txt", b"parent");
    fixture.write("agent.txt", b"agent");
    fixture.mention("`parent.txt`");
    let agent_messages = vec![json!({"role":"assistant","text":"`agent.txt`"})];
    let scope = FileScope {
        uid: "claude:synthetic",
        agent: Some("child"),
        cwd: fixture.root.to_str().unwrap(),
        messages: &agent_messages,
    };
    assert_eq!(
        fixture
            .service
            .target(&scope, "parent.txt", None)
            .err()
            .unwrap()
            .status,
        404
    );
    assert_eq!(
        fixture
            .service
            .target(&fixture.scope(), "agent.txt", None)
            .err()
            .unwrap()
            .status,
        404
    );
    assert_eq!(
        fixture
            .service
            .target(&scope, "agent.txt", None)
            .unwrap()
            .kind(),
        "file"
    );
}

#[test]
fn relative_absolute_unicode_markdown_tool_paths_and_line_suffixes() {
    let mut fixture = Fixture::new();
    let file = fixture.write("output/中文 report.py", b"print('synthetic')");
    fixture.mention("[`源码`](<output/中文 report.py:12>)");
    assert_eq!(fixture.target("output/中文 report.py:12").path(), file);
    fixture.mention(format!("`{}#L12C3`", file.display()));
    assert_eq!(
        fixture.target(&format!("{}#L12C3", file.display())).path(),
        file
    );
    let output = fixture.write("output/curve(final).png", b"synthetic-image");
    fixture.mention("[图](output/curve(final).png \"caption\")");
    assert_eq!(fixture.target("output/curve(final).png").path(), output);
    fixture
        .messages
        .push(json!({"role":"tool","args":{"nested":{"path":file.to_str().unwrap()}}}));
    fixture.mention("`中文 report.py`");
    assert_eq!(fixture.target("中文 report.py").path(), file);
    fixture.write("plain.py", b"pass");
    fixture.mention("源码plain.py:12，目录./output。");
    assert_eq!(fixture.target("plain.py:12").kind(), "file");
    assert_eq!(fixture.target("./output").kind(), "directory");
}

#[test]
fn basename_directory_fallback_never_recurses_or_guesses_collisions() {
    let mut fixture = Fixture::new();
    let file = fixture.write("checkout/README.md", b"one");
    fixture.write("checkout/nested/deep.txt", b"unmentioned child");
    fixture.mention("`./checkout` contains `README.md` and `deep.txt`");
    assert_eq!(fixture.target("README.md").path(), file);
    assert_eq!(
        fixture
            .service
            .target(&fixture.scope(), "deep.txt", None)
            .err()
            .unwrap()
            .status,
        404
    );
    fixture.write("README.md", b"two");
    assert_eq!(
        fixture
            .service
            .target(&fixture.scope(), "README.md", None)
            .err()
            .unwrap()
            .code,
        "file_ambiguous"
    );
    fixture.mention("`checkout/README.md`");
    assert_eq!(fixture.target("checkout/README.md").path(), file);
}

#[test]
fn basename_resolves_authorized_recorded_directory_without_granting_outside_cwd() {
    let mut fixture = Fixture::new();
    let path = fixture.write("artifact.txt", b"explicitly shared synthetic output");
    fixture.mention(format!(
        "`{}` contains `artifact.txt`",
        fixture.root.display()
    ));
    let scope = FileScope {
        uid: "claude:synthetic",
        agent: None,
        cwd: fixture.root.parent().unwrap().to_str().unwrap(),
        messages: &fixture.messages,
    };
    assert_eq!(
        fixture
            .service
            .target(&scope, "artifact.txt", None)
            .unwrap()
            .path(),
        path
    );
    fixture.mention(format!("`{}`", path.display()));
    let no_cwd = FileScope {
        uid: "claude:synthetic",
        agent: None,
        cwd: "",
        messages: &fixture.messages,
    };
    assert_eq!(
        fixture
            .service
            .target(&no_cwd, "artifact.txt", None)
            .unwrap()
            .path(),
        path
    );
}

#[test]
fn resolve_batch_reports_errors_and_never_probes_outside_roots() {
    let mut fixture = Fixture::new();
    let outside = Fixture::new();
    let external = outside.write("outside.txt", b"must not be read");
    let good = fixture.write("good.txt", b"good");
    fixture.mention(format!("`good.txt` `missing.txt` `{}`", external.display()));
    let value = fixture
        .service
        .resolve_many(
            &fixture.scope(),
            &[
                "good.txt".into(),
                "good.txt".into(),
                "missing.txt".into(),
                external.to_str().unwrap().into(),
            ],
        )
        .unwrap();
    assert_eq!(value["targets"].as_array().unwrap().len(), 1);
    assert_eq!(value["resolved"]["good.txt"], good.to_str().unwrap());
    assert_eq!(value["errors"].as_array().unwrap().len(), 2);
    assert_eq!(value["errors"][1]["code"], "file_outside_roots");
    assert_eq!(value["incomplete"], true);
    assert!(
        fixture
            .service
            .resolve_many(&fixture.scope(), &vec!["x".into(); 257])
            .is_err()
    );
    assert!(
        fixture
            .service
            .resolve_many(&fixture.scope(), &["x".repeat(4097)])
            .is_err()
    );
}

#[test]
fn directory_navigation_stops_at_anchor_root_and_needs_fresh_directory_mention() {
    let mut fixture = Fixture::new();
    let child = fixture.write("nested/child.txt", b"child");
    fixture.mention("`./nested` `nested/child.txt`");
    let scope = fixture.scope();
    let at_root = fixture
        .service
        .target(&scope, "./nested", fixture.root.to_str())
        .unwrap();
    let listing = fixture
        .service
        .list(&at_root, &ListOptions::default())
        .unwrap();
    assert_eq!(listing["parent"], Value::Null);
    assert_eq!(listing["root"], fixture.root.to_str().unwrap());
    assert_eq!(listing["writable"], false);
    assert_eq!(
        fixture
            .service
            .target(&scope, "./nested", fixture.root.parent().unwrap().to_str())
            .err()
            .unwrap()
            .status,
        403
    );
    assert_eq!(
        fixture
            .service
            .target(&scope, "nested/child.txt", child.to_str())
            .err()
            .unwrap()
            .code,
        "file_directory_anchor_required"
    );
    assert_eq!(
        fixture
            .service
            .target(&scope, "./nested", Some("../"))
            .err()
            .unwrap()
            .status,
        400
    );
    let second = Fixture::new();
    let service = FileService::open(vec![fixture.root.clone(), second.root.clone()]).unwrap();
    assert_eq!(
        service
            .target(&scope, "./nested", second.root.to_str())
            .err()
            .unwrap()
            .code,
        "file_outside_anchor_root"
    );
}

#[test]
fn path_validation_has_no_home_url_or_cross_platform_fallback() {
    let mut fixture = Fixture::new();
    for raw in [
        "~/secret.txt",
        "https://example.invalid/file.txt",
        "C:\\Windows\\secret.txt",
        "..\\outside.txt",
    ] {
        fixture.mention(format!("`{raw}`"));
        assert!(fixture.service.target(&fixture.scope(), raw, None).is_err());
    }
    for raw in ["", "a\0b", "a\nb"] {
        assert!(clean_ref(raw).is_err());
    }
    fixture.write("note.txt", b"note");
    fixture.mention("`note.txt`");
    let scope = FileScope {
        uid: "claude:synthetic",
        agent: None,
        cwd: "",
        messages: &fixture.messages,
    };
    assert_eq!(
        fixture
            .service
            .target(&scope, "note.txt", None)
            .err()
            .unwrap()
            .code,
        "file_cwd_unavailable"
    );
}

#[test]
fn listing_pagination_hidden_sort_and_limit_preserve_known_schema() {
    let mut fixture = Fixture::new();
    fixture.write("folder/b.txt", b"x");
    fixture.write("small.txt", b"x");
    fixture.write("large.txt", b"xxxxxxxxxx");
    fixture.write(".hidden", b"h");
    fixture.mention(format!("`{}`", fixture.root.display()));
    let target = fixture.target(fixture.root.to_str().unwrap());
    let mut options = ListOptions {
        limit: 2,
        sort: "size".into(),
        order: "desc".into(),
        hidden: false,
        ..ListOptions::default()
    };
    let first = fixture.service.list(&target, &options).unwrap();
    assert_eq!(first["entries"][0]["kind"], "directory");
    assert_eq!(first["entries"][1]["name"], "large.txt");
    assert_eq!(first["total"], 3);
    assert_eq!(first["next_offset"], 2);
    options.offset = 2;
    let second = fixture.service.list(&target, &options).unwrap();
    assert_eq!(second["entries"][0]["name"], "small.txt");
    assert!(second["next_offset"].is_null());
    options.offset = 0;
    options.hidden = true;
    assert_eq!(fixture.service.list(&target, &options).unwrap()["total"], 4);
    options.limit = 501;
    assert!(fixture.service.list(&target, &options).is_err());
}

#[test]
fn text_html_svg_binary_media_pdf_and_unicode_download_policy() {
    let mut fixture = Fixture::new();
    for (name, content) in [
        (
            "report.html",
            b"<script>window.synthetic=1</script>".as_slice(),
        ),
        ("unsafe.svg", b"<svg onload='bad()'/>"),
        ("image.png", b"synthetic image"),
        ("bytes.bin", b"\0\xff"),
        ("valid.pdf", b"%PDF-1.4\nsynthetic"),
        ("invalid.pdf", b"not PDF"),
        ("中文.txt", b"unicode filename"),
    ] {
        fixture.write(name, content);
        fixture.mention(format!("`{name}`"));
    }
    for name in ["report.html", "unsafe.svg"] {
        let info = fixture.service.describe(&fixture.target(name)).unwrap();
        assert_eq!(info["preview"], "text");
        let response = fixture.read(name, ReadOptions::default());
        assert_eq!(
            response.headers["Content-Type"],
            "text/plain; charset=utf-8"
        );
        assert!(response.headers["Content-Security-Policy"].contains("sandbox"));
    }
    assert_eq!(
        fixture
            .service
            .describe(&fixture.target("bytes.bin"))
            .unwrap()["preview"],
        "unsupported"
    );
    assert!(
        fixture.read("bytes.bin", ReadOptions::default()).headers["Content-Disposition"]
            .starts_with("attachment")
    );
    assert_eq!(
        fixture.read("image.png", ReadOptions::default()).headers["Content-Type"],
        "image/png"
    );
    assert_eq!(
        fixture
            .read(
                "valid.pdf",
                ReadOptions {
                    preview: true,
                    ..ReadOptions::default()
                }
            )
            .headers["Content-Security-Policy"],
        "frame-ancestors 'self'"
    );
    assert_eq!(
        fixture
            .service
            .describe(&fixture.target("invalid.pdf"))
            .err()
            .unwrap()
            .code,
        "file_invalid_pdf"
    );
    assert_eq!(
        bytes(fixture.read(
            "invalid.pdf",
            ReadOptions {
                download: true,
                ..ReadOptions::default()
            }
        )),
        b"not PDF"
    );
    let download = fixture.read(
        "中文.txt",
        ReadOptions {
            download: true,
            ..ReadOptions::default()
        },
    );
    assert!(
        download.headers["Content-Disposition"].contains("filename*=UTF-8''%E4%B8%AD%E6%96%87.txt")
    );
    assert_eq!(download.headers["Cache-Control"], "no-store");
    assert_eq!(download.headers["X-Content-Type-Options"], "nosniff");
}

#[test]
fn single_ranges_head_zero_file_and_bounded_stream_reader() {
    let mut fixture = Fixture::new();
    fixture.write("video.mp4", b"0123456789");
    fixture.write("empty.txt", b"");
    fixture.mention("`video.mp4` `empty.txt`");
    for (range, expected) in [
        ("bytes=2-5", b"2345".as_slice()),
        ("bytes=7-", b"789"),
        ("bytes=-3", b"789"),
        ("bytes=8-500", b"89"),
        ("bytes=-500", b"0123456789"),
    ] {
        let response = fixture.read(
            "video.mp4",
            ReadOptions {
                preview: true,
                range: Some(range.into()),
                ..ReadOptions::default()
            },
        );
        assert_eq!(response.status, 206);
        assert_eq!(
            response.headers["Content-Length"],
            expected.len().to_string()
        );
        assert_eq!(bytes(response), expected);
    }
    for range in [
        "bytes=",
        "bytes=-",
        "bytes=-0",
        "bytes=10-",
        "bytes=7-2",
        "bytes=0-1,3-4",
        "bytes=+0-1",
        "bytes=0 -1",
        "bytes=18446744073709551616-",
    ] {
        let response = fixture.read(
            "video.mp4",
            ReadOptions {
                preview: true,
                range: Some(range.into()),
                ..ReadOptions::default()
            },
        );
        assert_eq!(response.status, 416, "{range}");
        assert_eq!(response.headers["Content-Range"], "bytes */10");
        assert!(bytes(response).is_empty());
    }
    let head = fixture.read(
        "video.mp4",
        ReadOptions {
            preview: true,
            head: true,
            range: Some("bytes=2-5".into()),
            ..ReadOptions::default()
        },
    );
    assert_eq!(head.status, 200);
    assert!(!head.headers.contains_key("Content-Range"));
    assert_eq!(head.headers["Content-Length"], "10");
    assert!(bytes(head).is_empty());
    let unknown_unit = fixture.read(
        "video.mp4",
        ReadOptions {
            preview: true,
            range: Some("items=0-1".into()),
            ..ReadOptions::default()
        },
    );
    assert_eq!(unknown_unit.status, 200);
    assert_eq!(bytes(unknown_unit), b"0123456789");
    assert_eq!(
        fixture.read("empty.txt", ReadOptions::default()).headers["Content-Length"],
        "0"
    );
    assert_eq!(
        fixture
            .read(
                "empty.txt",
                ReadOptions {
                    range: Some("bytes=0-".into()),
                    ..ReadOptions::default()
                }
            )
            .status,
        416
    );
    let large = fixture.write("large.bin", &vec![b'x'; STREAM_CHUNK_BYTES * 2 + 1]);
    fixture.mention("`large.bin`");
    let FileBody::Reader(mut reader) = fixture
        .read(
            "large.bin",
            ReadOptions {
                download: true,
                ..ReadOptions::default()
            },
        )
        .body
    else {
        panic!("not streaming")
    };
    let mut buffer = vec![0; STREAM_CHUNK_BYTES * 3];
    assert_eq!(reader.read(&mut buffer).unwrap(), STREAM_CHUNK_BYTES);
    fs::OpenOptions::new()
        .write(true)
        .open(large)
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    assert!(reader.read(&mut buffer).is_err());
}

#[test]
fn preview_truncation_raw_and_download_budgets_are_explicit() {
    let mut fixture = Fixture::new();
    let path = fixture.write("large.txt", &vec![b'a'; MAX_PREVIEW_BYTES + 8]);
    fixture.mention("`large.txt`");
    let info = fixture
        .service
        .describe(&fixture.target("large.txt"))
        .unwrap();
    assert_eq!(info["truncated"], true);
    assert_eq!(info["text"].as_str().unwrap().len(), MAX_PREVIEW_BYTES);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_RAW_BYTES + 1)
        .unwrap();
    assert_eq!(
        fixture
            .service
            .read(fixture.target("large.txt"), &ReadOptions::default())
            .err()
            .unwrap()
            .code,
        "file_raw_budget"
    );
    assert_eq!(
        fixture
            .read(
                "large.txt",
                ReadOptions {
                    download: true,
                    head: true,
                    ..ReadOptions::default()
                }
            )
            .headers["Content-Length"],
        (MAX_RAW_BYTES + 1).to_string()
    );
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(MAX_STREAM_BYTES + 1)
        .unwrap();
    assert_eq!(
        fixture
            .service
            .read(
                fixture.target("large.txt"),
                &ReadOptions {
                    download: true,
                    ..ReadOptions::default()
                }
            )
            .err()
            .unwrap()
            .code,
        "file_stream_budget"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_special_files_and_permission_denial_are_never_followed_but_hardlinks_read() {
    use std::os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    };
    let mut fixture = Fixture::new();
    let outside = Fixture::new();
    let secret = outside.write("secret.txt", b"secret");
    symlink(&secret, fixture.root.join("link.txt")).unwrap();
    symlink(&outside.root, fixture.root.join("linked-dir")).unwrap();
    // A hard link inside the root is an ordinary file for reading: Python's
    // bug-report attachments and file manager link files into project trees.
    let shared = fixture.write("shared.txt", b"shared");
    fs::hard_link(&shared, fixture.root.join("hard.txt")).unwrap();
    let socket = UnixListener::bind(fixture.root.join("socket")).unwrap();
    let denied = fixture.write("denied.txt", b"no read");
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o000)).unwrap();
    fixture.mention(
        "`link.txt` `./linked-dir` `linked-dir/secret.txt` `hard.txt` `socket` `denied.txt`",
    );
    for raw in [
        "link.txt",
        "./linked-dir",
        "linked-dir/secret.txt",
        "socket",
        "denied.txt",
    ] {
        assert_eq!(
            fixture
                .service
                .target(&fixture.scope(), raw, None)
                .err()
                .unwrap()
                .status,
            403,
            "{raw}"
        );
    }
    assert_eq!(
        bytes(fixture.read("hard.txt", ReadOptions::default())),
        b"shared"
    );
    fixture.mention(format!("`{}`", fixture.root.display()));
    let listing = fixture
        .service
        .list(
            &fixture.target(fixture.root.to_str().unwrap()),
            &ListOptions::default(),
        )
        .unwrap();
    assert_eq!(listing["incomplete"], true);
    let rows = listing["entries"].as_array().unwrap();
    assert!(
        rows.iter()
            .filter(|row| !["hard.txt", "shared.txt"].contains(&row["name"].as_str().unwrap()))
            .all(|row| row["kind"] == "unavailable")
    );
    assert!(
        rows.iter()
            .filter(|row| ["hard.txt", "shared.txt"].contains(&row["name"].as_str().unwrap()))
            .all(|row| row["kind"] == "file")
    );
    assert!(FileService::open(vec![fixture.root.join("linked-dir")]).is_err());
    fs::set_permissions(denied, fs::Permissions::from_mode(0o600)).unwrap();
    drop(socket);
}

#[cfg(unix)]
#[test]
fn replaced_root_parent_or_file_rejects_existing_handle_before_any_bytes() {
    let mut fixture = Fixture::new();
    fixture.write("dir/child.txt", b"original");
    fixture.mention("`dir/child.txt`");
    let target = fixture.target("dir/child.txt");
    fs::rename(fixture.root.join("dir"), fixture.root.join("moved-dir")).unwrap();
    fixture.write("dir/child.txt", b"replacement");
    assert_eq!(
        fixture
            .service
            .read(target, &ReadOptions::default())
            .err()
            .unwrap()
            .code,
        "file_changed"
    );
    let target = fixture.target("dir/child.txt");
    fs::rename(
        fixture.root.join("dir/child.txt"),
        fixture.root.join("dir/moved.txt"),
    )
    .unwrap();
    fixture.write("dir/child.txt", b"replacement2");
    assert_eq!(
        fixture
            .service
            .read(target, &ReadOptions::default())
            .err()
            .unwrap()
            .code,
        "file_changed"
    );
    let root_parent = tempfile::tempdir().unwrap();
    let root = root_parent.path().canonicalize().unwrap().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file.txt"), b"old").unwrap();
    let service = FileService::open(vec![root.clone()]).unwrap();
    let messages = vec![json!({"role":"assistant","text":"`file.txt`"})];
    let scope = FileScope {
        uid: "claude:root",
        agent: None,
        cwd: root.to_str().unwrap(),
        messages: &messages,
    };
    let target = service.target(&scope, "file.txt", None).unwrap();
    fs::rename(&root, root_parent.path().join("old-root")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file.txt"), b"new").unwrap();
    assert_eq!(
        service
            .read(target, &ReadOptions::default())
            .err()
            .unwrap()
            .code,
        "file_changed"
    );
}

#[test]
fn unsupported_modes_are_not_empty_success_and_directory_budgets_are_honest() {
    assert_eq!(FileError::unsupported("jobs").status, 501);
    let mut fixture = Fixture::new();
    fixture.mention(format!("`{}`", fixture.root.display()));
    for index in 0..=MAX_DIRECTORY_ENTRIES {
        fs::write(fixture.root.join(format!("item-{index}")), b"").unwrap();
    }
    let error = fixture
        .service
        .list(
            &fixture.target(fixture.root.to_str().unwrap()),
            &ListOptions::default(),
        )
        .err()
        .unwrap();
    assert_eq!(error.code, "file_directory_budget");
}
