//! Synthetic explicit-root image handles. These bytes need not be images: format
//! validation belongs to media, while files only grants and preserves the read.
use super::*;
use crate::files::MAX_RAW_BYTES;
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    service: FileService,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        Self {
            service: FileService::open(vec![root.clone()]).unwrap(),
            root,
            _temp: temp,
        }
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
    fn scope<'a>(&'a self, messages: &'a [Value]) -> FileScope<'a> {
        FileScope {
            uid: "codex:synthetic-owner",
            agent: None,
            cwd: self.root.to_str().unwrap(),
            messages,
        }
    }
}

#[test]
#[cfg(not(windows))]
fn media_paths_use_python_percent_decoding_and_file_url_paths() {
    for (raw, expected) in [
        ("an%20image.png", "an image.png"),
        ("file:///tmp/an%2520image.png", "/tmp/an%20image.png"),
        ("file://host/tmp/a.png?download=1", "/tmp/a.png"),
        ("file:///tmp/image%23L2", "/tmp/image"),
        ("file:///tmp/%GG", "/tmp/%GG"),
        ("file:///tmp/%FF", "/tmp/�"),
        ("file:///tmp/%0A.png", "/tmp/\n.png"),
        ("~/image.png", "~/image.png"),
    ] {
        assert_eq!(normalize_media_ref(raw).unwrap(), expected);
    }
    assert!(normalize_media_ref("").is_err());
}

#[test]
fn exact_native_refs_are_inserted_without_text_tokenization_or_ambient_authority() {
    let fixture = Fixture::new();
    fixture.write("an image.png", b"synthetic");
    fixture.write("image.png", b"not mentioned");
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&[]), &["an image.png"])
        .unwrap();
    assert_eq!(scoped.identity(), ("codex:synthetic-owner", None));
    let mut image = scoped.image("an image.png").unwrap();
    assert_eq!(image.size(), 9);
    assert_eq!(image.remaining(), 9);
    assert_eq!(image.mime_hint(), Some("image/png"));
    let mut bytes = Vec::new();
    image.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"synthetic");
    assert_eq!(image.remaining(), 0);
    image.verify().unwrap();
    assert_eq!(
        scoped.image("image.png").err().unwrap().code,
        "file_not_referenced"
    );
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("external.png");
    fs::write(&path, b"outside").unwrap();
    let raw = path.to_str().unwrap();
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&[]), &[raw])
        .unwrap();
    assert_eq!(scoped.image(raw).unwrap().size(), 7);
}

#[test]
fn complete_selected_agent_messages_supply_scope_without_cloning_or_merging_parent_refs() {
    let fixture = Fixture::new();
    fixture.write("child.png", b"child");
    fixture.write("parent.png", b"parent");
    fixture.write("command.png", b"command");
    fixture.write("result.png", b"tool result");
    fixture.write("argument.png", b"tool argument");
    let messages = [
        json!({"role":"assistant·subagent","text":"`child.png`"}),
        json!({"role":"command·subagent","text":"`command.png`"}),
        json!({"role":"tool_result·subagent","text":"![image](result.png)"}),
        json!({"role":"tool·subagent","args":{"path":"argument.png"}}),
        json!({"role":"native_metadata","text":"parent.png"}),
    ];
    let scoped = fixture
        .service
        .scoped_media_iter(
            "claude:owner",
            Some("exact-child"),
            fixture.root.to_str().unwrap(),
            messages.iter(),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(scoped.identity(), ("claude:owner", Some("exact-child")));
    assert!(scoped.image("child.png").is_ok());
    assert!(scoped.image("command.png").is_ok());
    assert!(scoped.image("result.png").is_ok());
    assert!(scoped.image("argument.png").is_ok());
    assert_eq!(
        scoped.image("parent.png").err().unwrap().code,
        "file_not_referenced"
    );
    let parent = [json!({"role":"assistant","text":"parent.png"})];
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&parent), &[])
        .unwrap();
    assert_eq!(
        scoped.image("child.png").err().unwrap().code,
        "file_not_referenced"
    );
    assert!(scoped.image("parent.png").is_ok());
}

#[test]
fn media_basename_uses_cwd_even_when_other_directories_have_same_name() {
    let fixture = Fixture::new();
    fixture.write("same.png", b"cwd");
    let first = fixture.write("first/same.png", b"first");
    let second = fixture.write("second/same.png", b"second");
    let messages = [
        json!({"role":"assistant","text":format!("`{}`\n`{}`\n`same.png`",first.display(),second.display())}),
    ];
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&messages), &[])
        .unwrap();
    assert!(scoped.image("same.png").is_ok());
    assert!(scoped.image(first.to_str().unwrap()).is_ok());
    assert!(scoped.image(second.to_str().unwrap()).is_ok());
}

#[test]
fn unicode_and_drive_paths_keep_complete_evidence_beyond_the_display_cap() {
    let fixture = Fixture::new();
    fixture.write("图0.png", b"unicode");
    fixture.write("🖼.png", b"unicode symbol");
    fixture.write("第一目录/same.png", b"one");
    fixture.write("第二目录/same.png", b"two");
    let mut text = String::from("看 图0.png 🖼.png 第一目录/same.png same.png");
    for number in 0..16 {
        text.push_str(&format!(" filler{number}.png"));
    }
    text.push_str(" 第二目录/same.png");
    let messages = [json!({"role":"assistant","text":text})];
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&messages), &[])
        .unwrap();
    assert!(scoped.image("图0.png").is_ok());
    assert!(scoped.image("🖼.png").is_ok());
    assert!(scoped.image("第一目录/same.png").is_ok());
    assert_eq!(scoped.image("same.png").err().unwrap().status, 404);

    // Lexical coverage is platform-independent; no foreign drive is opened.
    let messages =
        [json!({"role":"assistant","text":r"paths C:\第一目录\same.png D:\第二目录\same.png"})];
    let index = references::ReferenceIndex::media(&messages, &[]).unwrap();
    assert!(
        index
            .refs
            .contains(&normalize_media_ref(r"C:\第一目录\same.png").unwrap())
    );
    assert!(
        index
            .refs
            .contains(&normalize_media_ref(r"D:\第二目录\same.png").unwrap())
    );
    assert_eq!(index.basenames["same.png"].len(), 2);

    let messages = [
        json!({"role":"assistant","text":(0..=250_000).map(|n|format!("picture{n}.png")).collect::<Vec<_>>().join(" ")}),
        json!({"role":"assistant","text":"late.png"}),
    ];
    let index = references::ReferenceIndex::media(&messages, &["typed.png"]).unwrap();
    assert!(index.complete);
    assert!(index.refs.contains("late.png"));
    assert!(index.refs.contains("typed.png"));
    assert!(
        references::ReferenceIndex::new(&messages)
            .unwrap()
            .refs
            .contains("late.png")
    );
}

#[test]
fn markdown_bare_file_urls_and_typed_refs_share_identical_media_normalization() {
    let fixture = Fixture::new();
    let spaced = fixture.write("an image.png", b"space");
    let percent = fixture.write("an%20image.png", b"percent");
    let hash = fixture.write("image#L2", b"hash");
    let uri_path = |path: &std::path::Path| {
        let text = path.to_str().unwrap();
        #[cfg(windows)]
        let text = format!("/{}", text.trim_start_matches("\\\\?\\").replace('\\', "/"));
        text.to_string()
            .replace('%', "%25")
            .replace(' ', "%20")
            .replace('#', "%23")
    };
    let uri = format!("file://{}", uri_path(&spaced));
    let percent_uri = format!("file://{}", uri_path(&percent));
    let hash_uri = format!("file://{}", uri_path(&hash));
    for text in [format!("![image](<{uri}>)"), format!("saved image: {uri}")] {
        let messages = [json!({"role":"assistant","text":text})];
        let scoped = fixture
            .service
            .scoped_media(&fixture.scope(&messages), &[])
            .unwrap();
        assert!(scoped.image(&uri).is_ok());
        assert!(scoped.image(spaced.to_str().unwrap()).is_ok());
    }
    let messages = [
        json!({"role":"assistant","text":format!("![percent]({percent_uri}) ![hash]({hash_uri})")}),
    ];
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&messages), &[])
        .unwrap();
    assert!(scoped.image(&percent_uri).is_ok());
    assert!(scoped.image(percent.to_str().unwrap()).is_err()); // Plain paths are unquoted too.
    assert!(scoped.image(&hash_uri).is_err()); // The URL is decoded before dropping its fragment.
    // The ordinary file API still does not expand URL references.
    assert!(
        fixture
            .service
            .target(&fixture.scope(&messages), &percent_uri, None)
            .is_err()
    );
}

#[test]
fn image_bytes_are_not_guessed_by_extension_and_size_is_bounded_before_reading() {
    let fixture = Fixture::new();
    fixture.write("extensionless", b"format is checked elsewhere");
    fs::create_dir(fixture.root.join("directory.png")).unwrap();
    let maximum = fixture.write("maximum.bin", b"");
    fs::OpenOptions::new()
        .write(true)
        .open(&maximum)
        .unwrap()
        .set_len(MAX_RAW_BYTES)
        .unwrap();
    let large = fixture.write("large.png", b"");
    fs::OpenOptions::new()
        .write(true)
        .open(&large)
        .unwrap()
        .set_len(MAX_RAW_BYTES + 1)
        .unwrap();
    let scoped = fixture
        .service
        .scoped_media(
            &fixture.scope(&[]),
            &["extensionless", "directory.png", "maximum.bin", "large.png"],
        )
        .unwrap();
    assert_eq!(scoped.image("extensionless").unwrap().mime_hint(), None);
    assert_eq!(scoped.image("maximum.bin").unwrap().size(), MAX_RAW_BYTES);
    assert_eq!(
        scoped.image("large.png").err().unwrap().code,
        "file_image_budget"
    );
    assert_eq!(
        scoped.image("directory.png").err().unwrap().code,
        "file_image_required"
    );
}

#[test]
fn bare_image_names_resolve_against_cwd_only_without_a_directory_sweep() {
    // A bare name is joined with the session cwd; a
    // mentioned directory is a `resolve-files` rule, not a media rule, so the
    // child of `pictures/` is not found through its parent's mention.
    let fixture = Fixture::new();
    let image = fixture.write("pictures/a.png", b"first");
    let directory = image.parent().unwrap().to_str().unwrap();
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&[]), &[directory, "a.png"])
        .unwrap();
    assert_eq!(scoped.image("a.png").err().unwrap().code, "file_not_found");
    fixture.write("a.png", b"cwd");
    let mut cwd_image = scoped.image("a.png").unwrap();
    let mut bytes = Vec::new();
    cwd_image.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"cwd");
    assert!(scoped.image("a.png").is_ok());
}

#[test]
fn file_versions_are_stable_for_unchanged_handles_but_detect_replacement_and_edits() {
    let fixture = Fixture::new();
    let path = fixture.write("a.png", b"first");
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&[]), &["a.png"])
        .unwrap();
    let old = scoped.image("a.png").unwrap();
    let same = scoped.image("a.png").unwrap();
    assert!(old.version() == same.version());
    fs::rename(&path, fixture.root.join("old.png")).unwrap();
    fixture.write("a.png", b"other");
    assert_eq!(old.verify().err().unwrap().code, "file_changed");
    let new = scoped.image("a.png").unwrap();
    assert!(old.version() != new.version());
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .write_all(b"longer")
        .unwrap();
    assert_eq!(new.verify().err().unwrap().code, "file_changed");
    assert!(new.version() != scoped.image("a.png").unwrap().version());
}

#[test]
fn changes_after_open_and_after_eof_are_errors_not_successful_reads() {
    let fixture = Fixture::new();
    let path = fixture.write("a.png", b"first");
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&[]), &["a.png"])
        .unwrap();
    let mut image = scoped.image("a.png").unwrap();
    let mut bytes = Vec::new();
    image.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"first");
    fs::write(&path, b"changed").unwrap();
    assert!(image.read(&mut [0]).is_err());
    assert!(image.read(&mut []).is_err());
    let mut image = scoped.image("a.png").unwrap();
    fs::rename(&path, fixture.root.join("old.png")).unwrap();
    fixture.write("a.png", b"new");
    assert!(image.read(&mut [0; 16]).is_err());
}

#[cfg(unix)]
#[test]
fn symlinks_special_files_and_replaced_parent_never_bypass_image_checks_but_hardlinks_read() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let fixture = Fixture::new();
    let original = fixture.write("original.png", b"original");
    symlink(&original, fixture.root.join("link.png")).unwrap();
    fs::hard_link(&original, fixture.root.join("hard.png")).unwrap();
    let _socket = UnixListener::bind(fixture.root.join("socket.png")).unwrap();
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&[]), &["link.png", "hard.png", "socket.png"])
        .unwrap();
    assert_eq!(scoped.image("link.png").unwrap().size(), 8);
    assert_eq!(
        scoped.image("socket.png").err().unwrap().code,
        "file_special_forbidden"
    );
    // Bug-report attachments are hard links into the project tree:
    // a multiply linked regular file inside the root is readable as an image.
    let mut hard = scoped.image("hard.png").unwrap();
    let mut bytes = Vec::new();
    hard.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
    fixture.write("dir/a.png", b"same leaf");
    let scoped = fixture
        .service
        .scoped_media(&fixture.scope(&[]), &["dir/a.png"])
        .unwrap();
    let old = scoped.image("dir/a.png").unwrap();
    fs::rename(fixture.root.join("dir"), fixture.root.join("old-dir")).unwrap();
    fs::create_dir(fixture.root.join("dir")).unwrap();
    fs::rename(
        fixture.root.join("old-dir/a.png"),
        fixture.root.join("dir/a.png"),
    )
    .unwrap();
    assert_eq!(old.verify().err().unwrap().code, "file_changed");
    assert!(old.version() != scoped.image("dir/a.png").unwrap().version());
}

#[test]
fn invalid_unselected_native_refs_are_not_grants_or_scope_wide_image_failures() {
    let fixture = Fixture::new();
    fixture.write("good.png", b"good");
    let scoped = fixture
        .service
        .scoped_media(
            &fixture.scope(&[]),
            &["good.png", "file://remote/private.png", "~/private.png"],
        )
        .unwrap();
    assert!(scoped.image("good.png").is_ok());
    assert_eq!(
        scoped
            .image("file://remote/private.png")
            .err()
            .unwrap()
            .code,
        "file_not_found"
    );
    assert_eq!(
        scoped.image("~/private.png").err().unwrap().code,
        "file_not_found"
    );
}
