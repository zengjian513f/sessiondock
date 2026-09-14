#![cfg(unix)]

use super::*;
use crate::lifecycle::{model::Failure, store::LifecycleStore};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Fixture {
    directory: TempDir,
    config: Config,
    work: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for name in ["host", "work", "other", "bin", "ledger"] {
            fs::create_dir(root.join(name)).unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        // Ordinary tests only validate these private files, never execute them.
        for name in ["host", "adapter"] {
            fs::write(
                root.join("bin").join(name),
                b"synthetic-executable-placeholder",
            )
            .unwrap();
            fs::set_permissions(
                root.join("bin").join(name),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        let config = Config {
            schema: 1,
            host_binary: root.join("bin/host"),
            host_dir: root.join("host"),
            adapters: vec![Adapter {
                id: "shell-v1".into(),
                source: Source::Codex,
                executable: root.join("bin/adapter"),
                args: vec![],
                env: BTreeMap::new(),
            }],
            profiles: vec![],
        };
        let work = root.join("work");
        Self {
            directory,
            config,
            work,
        }
    }
    fn spec(&self) -> LaunchSpec {
        LaunchSpec::new(
            Source::Codex,
            "shell-v1".into(),
            &self.directory.path().join("work"),
        )
        .unwrap()
    }
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"host_binary":self.config.host_binary,"host_dir":self.config.host_dir,
            "adapters":[{"id":"shell-v1","source":"codex",
                "executable":self.config.adapters[0].executable,"args":[],"env":{}}]})
    }
    fn config_file(&self, bytes: &[u8]) -> PathBuf {
        let path = self.directory.path().join("launcher.json");
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
    fn authority(&self) -> (LifecycleStore, StartAuthority) {
        let mut store = LifecycleStore::initialize(&self.directory.path().join("ledger")).unwrap();
        let prepared = store
            .create("synthetic-launch-request", &self.spec())
            .unwrap()
            .prepared
            .unwrap();
        let authority = store.begin_start(prepared).unwrap();
        (store, authority)
    }
}

#[test]
fn explicit_allowlist_accepts_only_its_source_and_adapter() {
    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    assert_eq!(launcher.host_dir(), fixture.config.host_dir);
    assert!(launcher.validate_spec(&fixture.spec()).is_ok());
    let child = fixture.work.join("nested");
    fs::create_dir(&child).unwrap();
    assert!(
        launcher
            .validate_spec(&LaunchSpec::new(Source::Codex, "shell-v1".into(), &child).unwrap())
            .is_ok()
    );
    for (source, adapter) in [(Source::Claude, "shell-v1"), (Source::Codex, "unknown-v1")] {
        let spec = LaunchSpec::new(
            source,
            adapter.into(),
            &fixture.directory.path().join("other"),
        )
        .unwrap();
        assert!(launcher.validate_spec(&spec).is_err());
    }
    let outside = LaunchSpec::new(
        Source::Codex,
        "shell-v1".into(),
        &fixture.directory.path().join("other"),
    )
    .unwrap();
    assert!(launcher.validate_spec(&outside).is_ok());
}

#[test]
fn configuration_rejects_only_unusable_process_inputs() {
    let fixture = Fixture::new();
    let mutate = |change: fn(&mut Config)| {
        let mut config = fixture.config.clone();
        change(&mut config);
        assert!(Launcher::new(config).is_err());
    };
    for change in [
        (|c: &mut Config| c.adapters.clear()) as fn(&mut Config),
        |c| c.adapters.push(c.adapters[0].clone()),
        |c| c.adapters[0].args = vec!["contains\0nul".into()],
        |c| {
            c.adapters[0].env.insert("BAD=KEY".into(), "x".into());
        },
        |c| {
            c.adapters[0]
                .env
                .insert("OK".into(), "contains\0nul".into());
        },
    ] {
        mutate(change);
    }
}

#[test]
fn operator_config_accepts_large_arguments_and_environment() {
    let fixture = Fixture::new();
    let mut config = fixture.profiles();
    let profile = config.profiles[1].clone();
    config.profiles = (0..20)
        .map(|index| {
            let mut profile = profile.clone();
            profile.id = format!("codex-{index}-v1");
            profile.args = vec!["x".repeat(5000); 70];
            profile.env = (0..80)
                .map(|index| (format!("CUSTOM_{index}"), "v".repeat(9000)))
                .collect();
            profile.env_remove = (0..20).map(|index| format!("REMOVE_{index}")).collect();
            profile
        })
        .collect();
    assert!(Launcher::new(config).is_ok());

    // This is a trusted on-disk operator file, not a host JSON control line.
    let json = format!("{}{}", " ".repeat(128 * 1024), fixture.json());
    assert!(read_config(&fixture.config_file(json.as_bytes())).is_ok());
}

#[test]
fn unknown_fields_are_ignored_and_executables_remain_checked() {
    let fixture = Fixture::new();
    let mut json = fixture.json();
    json["unknown_top_level"] = serde_json::json!(["relative", "/missing"]);
    json["adapters"][0]["unknown_adapter_field"] = serde_json::json!(["ignored"]);
    let launcher =
        Launcher::new(read_config(&fixture.config_file(json.to_string().as_bytes())).unwrap())
            .unwrap();
    let child = fixture.work.join("nested");
    fs::create_dir(&child).unwrap();
    let text = format!("{}/", fixture.work.display());
    assert_eq!(
        launcher.complete_directories(&text, 24).unwrap(),
        vec![format!("{text}nested/")]
    );
    // A symlinked executable resolves to the real file.
    let alias = fixture.directory.path().join("host-alias");
    symlink(&fixture.config.host_binary, &alias).unwrap();
    let mut config = fixture.config.clone();
    config.host_binary = alias;
    assert!(Launcher::new(config).is_ok());
    fs::set_permissions(&fixture.config.host_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Launcher::new(fixture.config.clone()).is_ok());
    fs::set_permissions(
        &fixture.config.adapters[0].executable,
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        Launcher::new(fixture.config.clone()).err(),
        Some(Error::UnsafePermissions)
    );
}

#[test]
fn cwd_validation_follows_python_resolve_without_a_configured_root() {
    let fixture = Fixture::new();
    for path in [
        fixture.directory.path().join("work/../work"),
        PathBuf::from(format!("{}/work//", fixture.directory.path().display())),
    ] {
        let spec = LaunchSpec::new(Source::Codex, "shell-v1".into(), &path).unwrap();
        assert!(
            Launcher::new(fixture.config.clone())
                .unwrap()
                .validate_spec(&spec)
                .is_ok()
        );
    }
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    let spec: LaunchSpec =
        serde_json::from_value(serde_json::json!({"source":"codex","adapter_id":"shell-v1",
        "cwd":fixture.directory.path().join("missing"),"launch":{"kind":"fixed"}}))
        .unwrap();
    assert!(launcher.validate_spec(&spec).is_err());
}

#[test]
fn executable_aliases_execute_the_checked_target_after_alias_retargeting() {
    let fixture = Fixture::new();
    let alias = fixture.directory.path().join("adapter-alias");
    symlink(&fixture.config.adapters[0].executable, &alias).unwrap();
    let mut config = fixture.config.clone();
    config.adapters[0].executable = alias.clone();
    let launcher = Launcher::new(config).unwrap();
    let record = fixture.record(&fixture.spec());
    fs::remove_file(&alias).unwrap();
    symlink(&fixture.config.host_binary, &alias).unwrap();
    launcher.validate_spec(record.spec()).unwrap();
    assert_eq!(
        launcher.argv(&record).unwrap()[0],
        fixture.config.adapters[0].executable
    );
    // Replacing the resolved target itself is still caught by the held handle.
    fs::remove_file(&fixture.config.adapters[0].executable).unwrap();
    symlink(
        &fixture.config.host_binary,
        &fixture.config.adapters[0].executable,
    )
    .unwrap();
    assert!(matches!(
        launcher.validate_spec(record.spec()),
        Err(Error::UnsafePath | Error::Changed)
    ));
}

#[test]
fn profile_executables_and_cwds_resolve_symlinked_ancestors() {
    let fixture = Fixture::new();
    let mut config = fixture.profiles();
    let alias = fixture.directory.path().join("parent-alias");
    symlink(fixture.directory.path(), &alias).unwrap();
    config.profiles[1].executable = alias.join("bin/codex");
    let spec = LaunchSpec::profile_new(
        Source::Codex,
        "codex-cli-v1".into(),
        &alias.join("work/codex-area/nested"),
    )
    .unwrap();
    let launcher = Launcher::new(config).unwrap();
    launcher.validate_spec(&spec).unwrap();
    let record = fixture.record(&spec);
    assert_eq!(
        launcher.argv(&record).unwrap()[0],
        fixture.directory.path().join("bin/codex")
    );
}

#[test]
fn launch_rechecks_executable_and_current_cwd() {
    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    fs::write(&fixture.config.host_binary, b"changed").unwrap();
    assert_eq!(launcher.validate_spec(&fixture.spec()), Err(Error::Changed));

    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    fs::rename(
        &fixture.work,
        fixture.directory.path().join("previous-work"),
    )
    .unwrap();
    fs::create_dir(&fixture.work).unwrap();
    assert!(launcher.validate_spec(&fixture.spec()).is_ok());

    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    let nested = fixture.work.join("nested");
    fs::create_dir(&nested).unwrap();
    let spec = LaunchSpec::new(Source::Codex, "shell-v1".into(), &nested).unwrap();
    fs::remove_dir(&nested).unwrap();
    symlink(fixture.directory.path().join("other"), &nested).unwrap();
    assert_eq!(launcher.validate_spec(&spec), Err(Error::UnsafePath));
}

#[test]
fn occupied_endpoint_returns_the_original_authority_without_spawning() {
    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    let (mut store, authority) = fixture.authority();
    let id = authority.record().record_id().to_owned();
    fs::write(
        fixture
            .config
            .host_dir
            .join(format!("{}.json", authority.record().host_name())),
        b"preserve",
    )
    .unwrap();
    let failed = match launcher.launch(authority) {
        Err(failed) => failed,
        Ok(_) => panic!("occupied endpoint must not spawn"),
    };
    assert_eq!(failed.error, Error::EndpointOccupied);
    assert_eq!(failed.authority.record().record_id(), id);
    assert_eq!(
        store
            .mark_failed(failed.authority, Failure::LaunchRejected)
            .unwrap()
            .state(),
        State::Failed
    );
}

#[test]
fn ordinary_config_accepts_compatible_json_and_never_echoes_input() {
    let fixture = Fixture::new();
    let raw = fixture.json().to_string();
    let path = fixture.config_file(raw.as_bytes());
    assert!(Launcher::new(read_config(&path).unwrap()).is_ok());
    for compatible in [
        raw.replace(
            "\"env\":{}",
            "\"env\":{\"PRIVATE_ENV\":\"one\",\"PRIVATE_ENV\":\"two\"}",
        ),
        raw.replacen('{', "{\"secret-extra\":\"PRIVATE_SECRET\",", 1),
        raw.replacen(
            '{',
            &format!("{{\"padding\":\"{}\",", "x".repeat(64 * 1024)),
            1,
        ),
    ] {
        let path = fixture.config_file(compatible.as_bytes());
        assert!(read_config(&path).is_ok());
    }
    for invalid in [
        raw.replace(
            "\"id\":\"shell-v1\"",
            "\"id\":\"shell-v1\",\"id\":\"shell-v2\"",
        ),
        " ".repeat(64 * 1024 + 1),
    ] {
        let path = fixture.config_file(invalid.as_bytes());
        let error = read_config(&path).err().unwrap();
        let rendered = format!("{error:?} {error}");
        assert!(
            !rendered.contains("PRIVATE")
                && !rendered.contains(fixture.directory.path().to_str().unwrap())
        );
    }
    let path = fixture.config_file(fixture.json().to_string().as_bytes());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_config(&path).is_ok());
    let hardlink = fixture.directory.path().join("hardlink.json");
    fs::hard_link(&path, &hardlink).unwrap();
    assert!(read_config(&path).is_ok());
    let alias = fixture.directory.path().join("config-alias.json");
    symlink(&path, &alias).unwrap();
    assert!(read_config(&alias).is_ok());
}

/// Explicit opt-in integration: both paths must be supplied, no repository,
/// HOME or PATH discovery, and no paid CLI may be used for the shell path.
#[test]
#[ignore = "set SESSIONDOCK_TEST_PTYHOST_BINARY and SESSIONDOCK_TEST_FREE_SHELL_BINARY explicitly"]
fn explicit_free_shell_starts_with_receipt_identity_and_survives_launcher_drop() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut fixture = Fixture::new();
    fixture.config.host_binary = PathBuf::from(
        std::env::var_os("SESSIONDOCK_TEST_PTYHOST_BINARY").expect("explicit test host binary"),
    );
    fixture.config.adapters[0].executable = PathBuf::from(
        std::env::var_os("SESSIONDOCK_TEST_FREE_SHELL_BINARY").expect("explicit free shell binary"),
    );
    fixture.config.adapters[0].args = vec!["-c".into(),
        "printf 'LAUNCH_ENV:%s:%s:%s\\n' \"${EXPLICIT_VALUE-unset}\" \"${HOME-unset}\" \"${TERM-unset}\"; while IFS= read -r line; do case \"$line\" in quit) exit 0;; *) printf 'RECEIVED:%s\\n' \"$line\";; esac; done".into()];
    fixture.config.adapters[0]
        .env
        .insert("EXPLICIT_VALUE".into(), "configured".into());
    fixture.config.adapters[0]
        .env
        .insert("HOME".into(), "/synthetic/shell-home".into());
    fixture.config.adapters[0]
        .env
        .insert("TERM".into(), "xterm-256color".into());
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    let (mut store, authority) = fixture.authority();
    let record = authority.record().clone();
    let started = match launcher.launch(authority) {
        Ok(started) => started,
        Err(error) => panic!("{}", error.error),
    };
    let Started {
        authority,
        mut child,
    } = started;
    let endpoint = fixture
        .config
        .host_dir
        .join(format!("{}.sock", record.host_name()));
    let request = |inner: serde_json::Value| -> serde_json::Value {
        let mut stream = UnixStream::connect(&endpoint).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let envelope = serde_json::json!({"op":"launch_guard_v1","expected_source":"codex",
            "expected_launch_id":record.launch_id(),"expected_instance_id":record.instance_id(),"request":inner});
        writeln!(stream, "{envelope}").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !endpoint.exists() {
            assert!(child.try_wait().unwrap().is_none());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let info = request(serde_json::json!({"op":"info"}));
        assert_eq!(info["launch_guard"]["instance_id"], record.instance_id());
        assert_eq!(info["launch_guard"]["launch_id"], record.launch_id());
        assert_eq!(info["info"]["cwd"], record.spec().cwd().to_str().unwrap());
        assert!(info["info"]["meta"].get("sid").is_none());
        assert!(info["info"]["meta"].get("uid").is_none());
        loop {
            let capture = request(serde_json::json!({"op":"capture","styled":false}));
            if capture["text"]
                .as_str()
                .unwrap()
                .contains("LAUNCH_ENV:configured:/synthetic/shell-home:xterm-256color")
            {
                break;
            }
            assert!(Instant::now() < deadline);
        }
        store.mark_running(authority).unwrap();
        drop(launcher); // Releasing launcher/coordinator ownership never kills a host.
        assert!(child.try_wait().unwrap().is_none());
        assert_eq!(
            request(serde_json::json!({"op":"send","text":"quit\n"}))["ok"],
            true
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }));
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill(); // Only the held exact test child, never a discovered PID.
        let _ = child.wait();
    }
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}

// ------------------------------------------------------------------ CLI profiles
const SID: &str = "0123abcd-4567-4ef0-8123-456789abcdef";
const CODEX_UID: &str = "codex:0123456789abcdef";
const CLAUDE_UID: &str = "claude:0123456789abcdef";

impl Fixture {
    /// Schema-2 configuration: the legacy free-shell adapter (grok) plus
    /// synthetic Claude/Codex/Grok CLI profiles with legacy cwd-root fields.
    fn profiles(&self) -> Config {
        let root = self.directory.path();
        for name in ["claude", "codex", "grok"] {
            fs::write(root.join("bin").join(name), b"synthetic-cli-placeholder").unwrap();
            fs::set_permissions(
                root.join("bin").join(name),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        for name in [
            "work/claude-area",
            "work/codex-area",
            "work/codex-area/nested",
        ] {
            let _ = fs::create_dir(root.join(name));
        }
        let mut config = self.config.clone();
        config.schema = 2;
        config.adapters[0].source = Source::Grok;
        config.profiles = vec![
            CliProfile {
                id: "claude-cli-v1".into(),
                source: Source::Claude,
                executable: root.join("bin/claude"),
                args: vec![
                    "--settings".into(),
                    "/synthetic/bridge-settings.json".into(),
                ],
                new_args: vec!["--session-id".into(), "{session_id}".into()],
                resume_args: vec!["--resume".into(), "{sid}".into()],
                env: BTreeMap::from([("HOME".to_string(), "/synthetic/home".to_string())]),
                env_remove: vec!["TERM".into()],
            },
            CliProfile {
                id: "codex-cli-v1".into(),
                source: Source::Codex,
                executable: root.join("bin/codex"),
                args: vec![
                    "--enable".into(),
                    "default_mode_request_user_input".into(),
                    "-c".into(),
                    "suppress_unstable_features_warning=true".into(),
                ],
                new_args: vec![],
                resume_args: vec!["resume".into(), "{sid}".into()],
                env: BTreeMap::new(),
                env_remove: vec![],
            },
            CliProfile {
                id: "grok-cli-v1".into(),
                source: Source::Grok,
                executable: root.join("bin/grok"),
                args: vec![],
                new_args: vec![],
                resume_args: vec![],
                env: BTreeMap::new(),
                env_remove: vec![],
            },
        ];
        config
    }
    fn record(&self, spec: &LaunchSpec) -> super::super::model::Record {
        let mut store = LifecycleStore::initialize(&self.directory.path().join("ledger")).unwrap();
        store
            .create("synthetic-profile-request", spec)
            .unwrap()
            .record
    }
}
fn strings(argv: &[OsString]) -> Vec<String> {
    argv.iter()
        .map(|arg| arg.to_str().unwrap().to_owned())
        .collect()
}

#[test]
fn profile_configuration_rejects_only_unusable_process_inputs() {
    let fixture = Fixture::new();
    let base = fixture.profiles();
    assert!(Launcher::new(base.clone()).is_ok());
    let mutate = |change: fn(&mut Config)| {
        let mut config = base.clone();
        change(&mut config);
        assert!(Launcher::new(config).is_err());
    };
    for change in [
        (|c: &mut Config| {
            c.adapters.clear();
            c.profiles.clear();
        }) as fn(&mut Config),
        |c| c.profiles.push(c.profiles[0].clone()),
        |c| c.profiles[0].id = c.adapters[0].id.clone(),
        |c| c.profiles[0].executable = PathBuf::from("/nonexistent/synthetic/claude"),
        |c| c.profiles[0].executable = PathBuf::from("bin/claude"),
        |c| c.profiles[0].args = vec!["contains\0nul".into()],
        |c| {
            c.profiles[0]
                .env
                .insert("CLAUDE_CODE_SESSION_ID".into(), SID.into());
        },
        |c| {
            c.profiles[1]
                .env
                .insert("CODEX_COMPANION_SESSION_ID".into(), SID.into());
        },
        |c| {
            c.profiles[0]
                .env
                .insert("GROK_SESSION_ID".into(), SID.into());
        },
        |c| c.profiles[0].env_remove = vec!["BAD=NAME".into()],
    ] {
        mutate(change);
    }

    let mut compatible = base.clone();
    compatible.schema = 99;
    compatible.profiles[0].new_args = vec!["literal-{session_id}".into()];
    compatible.profiles[0].resume_args.clear();
    compatible.profiles[0].env_remove = vec!["TERM".into(), "TERM".into(), "HOME".into()];
    compatible.profiles[0]
        .env
        .insert("TMUX".into(), "/tmp/x".into());
    assert!(Launcher::new(compatible).is_ok());

    fs::set_permissions(
        &base.profiles[0].executable,
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        Launcher::new(base.clone()).err(),
        Some(Error::UnsafePermissions)
    );
    fs::set_permissions(
        &base.profiles[0].executable,
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    // Unknown fields and schema values are ignored.
    let mut json = serde_json::json!({"schema":2,"host_binary":base.host_binary,"host_dir":base.host_dir,
        "unknown_top_level":["relative","/missing"],"adapters":[],"profiles":[{"id":"codex-cli-v1","source":"codex",
        "executable":base.profiles[1].executable,"args":[],"new_args":[],"resume_args":["resume","{sid}"],
        "env":{},"env_remove":[],"unknown_profile_field":["ignored"]}]});
    let launcher =
        Launcher::new(read_config(&fixture.config_file(json.to_string().as_bytes())).unwrap())
            .unwrap();
    assert_eq!(
        launcher.entries(),
        &[Entry {
            id: "codex-cli-v1".into(),
            source: Source::Codex,
            profile: true,
            resume: true,
        }]
    );
    json["profiles"][0]["shell"] = serde_json::json!("/bin/sh -c");
    assert!(read_config(&fixture.config_file(json.to_string().as_bytes())).is_ok());
    json["schema"] = serde_json::json!(1);
    assert!(read_config(&fixture.config_file(json.to_string().as_bytes())).is_ok());
    json.as_object_mut().unwrap().remove("schema");
    assert!(read_config(&fixture.config_file(fixture.json().to_string().as_bytes())).is_ok());
}

#[test]
fn argv_metadata_and_kind_rules_follow_the_fixed_per_source_contract() {
    let fixture = Fixture::new();
    let config = fixture.profiles();
    let launcher = Launcher::new(config.clone()).unwrap();
    let root = fixture.directory.path();
    let claude = root.join("work/claude-area");
    let codex = root.join("work/codex-area/nested");
    let expected_entries = vec![
        Entry {
            id: "shell-v1".into(),
            source: Source::Grok,
            profile: false,
            resume: false,
        },
        Entry {
            id: "claude-cli-v1".into(),
            source: Source::Claude,
            profile: true,
            resume: true,
        },
        Entry {
            id: "codex-cli-v1".into(),
            source: Source::Codex,
            profile: true,
            resume: true,
        },
        Entry {
            id: "grok-cli-v1".into(),
            source: Source::Grok,
            profile: true,
            resume: true,
        },
    ];
    assert_eq!(launcher.entries(), expected_entries.as_slice());
    assert_eq!(entries(&config), expected_entries);

    // Claude new: server UUID as a whole `--session-id` argument and metadata sid.
    let spec = LaunchSpec::profile_new(Source::Claude, "claude-cli-v1".into(), &claude).unwrap();
    launcher.validate_spec(&spec).unwrap();
    let record = fixture.record(&spec);
    let sid = record.session_id().unwrap().to_owned();
    assert_eq!(
        strings(&launcher.argv(&record).unwrap()),
        vec![
            root.join("bin/claude").to_str().unwrap().to_owned(),
            "--settings".into(),
            "/synthetic/bridge-settings.json".into(),
            "--session-id".into(),
            sid.clone(),
        ]
    );
    let metadata = Launcher::metadata(&record);
    assert_eq!(metadata["sid"], sid);
    assert!(metadata.get("uid").is_none());
    assert_eq!(metadata["source"], "claude");
    assert_eq!(metadata["launch_id"], record.launch_id());
    assert_eq!(metadata["instance_id"], record.instance_id());

    // Codex new: fixed prefix only, no upfront SID anywhere.
    fs::remove_dir_all(root.join("ledger")).unwrap();
    fs::create_dir(root.join("ledger")).unwrap();
    fs::set_permissions(root.join("ledger"), fs::Permissions::from_mode(0o700)).unwrap();
    let spec = LaunchSpec::profile_new(Source::Codex, "codex-cli-v1".into(), &codex).unwrap();
    launcher.validate_spec(&spec).unwrap();
    let record = fixture.record(&spec);
    assert!(record.session_id().is_none());
    assert_eq!(
        strings(&launcher.argv(&record).unwrap()),
        vec![
            root.join("bin/codex").to_str().unwrap().to_owned(),
            "--enable".into(),
            "default_mode_request_user_input".into(),
            "-c".into(),
            "suppress_unstable_features_warning=true".into(),
        ]
    );
    let metadata = Launcher::metadata(&record);
    assert!(metadata.get("sid").is_none() && metadata.get("uid").is_none());
    assert_eq!(metadata["launch_id"], record.launch_id());

    // Codex resume: `resume <sid>` after the fixed prefix, sid+uid metadata.
    fs::remove_dir_all(root.join("ledger")).unwrap();
    fs::create_dir(root.join("ledger")).unwrap();
    fs::set_permissions(root.join("ledger"), fs::Permissions::from_mode(0o700)).unwrap();
    let spec = LaunchSpec::resume(
        Source::Codex,
        "codex-cli-v1".into(),
        &codex,
        SID.into(),
        CODEX_UID.into(),
    )
    .unwrap();
    launcher.validate_spec(&spec).unwrap();
    let record = fixture.record(&spec);
    let argv = strings(&launcher.argv(&record).unwrap());
    assert_eq!(&argv[5..], ["resume", SID]);
    assert_eq!(argv.len(), 7);
    let metadata = Launcher::metadata(&record);
    assert_eq!(metadata["sid"], SID);
    assert_eq!(metadata["uid"], CODEX_UID);

    // Claude resume: `--resume <sid>` after `--settings`.
    fs::remove_dir_all(root.join("ledger")).unwrap();
    fs::create_dir(root.join("ledger")).unwrap();
    fs::set_permissions(root.join("ledger"), fs::Permissions::from_mode(0o700)).unwrap();
    let spec = LaunchSpec::resume(
        Source::Claude,
        "claude-cli-v1".into(),
        &claude,
        SID.into(),
        CLAUDE_UID.into(),
    )
    .unwrap();
    launcher.validate_spec(&spec).unwrap();
    let record = fixture.record(&spec);
    let argv = strings(&launcher.argv(&record).unwrap());
    assert_eq!(
        &argv[1..],
        [
            "--settings",
            "/synthetic/bridge-settings.json",
            "--resume",
            SID
        ]
    );

    // Legacy adapter: fixed argv, Fixed kind only; profiles never take Fixed.
    let work = root.join("work");
    let fixed = LaunchSpec::new(Source::Grok, "shell-v1".into(), &work).unwrap();
    launcher.validate_spec(&fixed).unwrap();
    assert_eq!(
        launcher
            .validate_spec(
                &LaunchSpec::profile_new(Source::Grok, "shell-v1".into(), &work).unwrap()
            )
            .err(),
        Some(Error::InvalidSpec)
    );
    assert_eq!(
        launcher
            .validate_spec(&LaunchSpec::new(Source::Grok, "grok-cli-v1".into(), &work).unwrap())
            .err(),
        Some(Error::InvalidSpec)
    );
    // Grok receives a new SID and supports the same `--resume` form as Python.
    let grok_new = LaunchSpec::profile_new(Source::Grok, "grok-cli-v1".into(), &work).unwrap();
    launcher.validate_spec(&grok_new).unwrap();
    fs::remove_dir_all(root.join("ledger")).unwrap();
    fs::create_dir(root.join("ledger")).unwrap();
    fs::set_permissions(root.join("ledger"), fs::Permissions::from_mode(0o700)).unwrap();
    let grok_record = fixture.record(&grok_new);
    let grok_sid = grok_record.session_id().unwrap();
    assert_eq!(
        strings(&launcher.argv(&grok_record).unwrap()),
        [
            root.join("bin/grok").to_string_lossy().into_owned(),
            "--session-id".into(),
            grok_sid.into()
        ]
    );
    let grok_resume = LaunchSpec::resume(
        Source::Grok,
        "grok-cli-v1".into(),
        &work,
        SID.into(),
        "grok:0123456789abcdef".into(),
    )
    .unwrap();
    launcher.validate_spec(&grok_resume).unwrap();

    // Source/ID disagreement remains invalid; cwd is unrestricted.
    assert_eq!(
        launcher
            .validate_spec(
                &LaunchSpec::profile_new(Source::Codex, "claude-cli-v1".into(), &claude).unwrap()
            )
            .err(),
        Some(Error::AdapterUnavailable)
    );
    launcher
        .validate_spec(
            &LaunchSpec::profile_new(Source::Claude, "claude-cli-v1".into(), &codex).unwrap(),
        )
        .unwrap();
    launcher
        .validate_spec(
            &LaunchSpec::profile_new(Source::Claude, "claude-cli-v1".into(), &work).unwrap(),
        )
        .unwrap();
    assert_eq!(
        launcher
            .validate_spec(
                &LaunchSpec::profile_new(Source::Claude, "missing-v1".into(), &claude).unwrap()
            )
            .err(),
        Some(Error::AdapterUnavailable)
    );
    // Constructors resolve a symlinked cwd before it is persisted.
    let alias = root.join("work/claude-area/alias");
    symlink(root.join("work/codex-area"), &alias).unwrap();
    let linked = LaunchSpec::profile_new(Source::Claude, "claude-cli-v1".into(), &alias).unwrap();
    assert_eq!(linked.cwd(), root.join("work/codex-area"));
    launcher.validate_spec(&linked).unwrap();
}

#[test]
fn directory_completion_matches_python() {
    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.profiles()).unwrap();
    let root = fixture.directory.path();
    let work = root.join("work");
    let work_text = work.to_str().unwrap().to_owned();
    fs::create_dir(work.join(".hidden")).unwrap();
    fs::create_dir(work.join("Beta")).unwrap();
    fs::create_dir(work.join("Beta/child")).unwrap();
    fs::create_dir(root.join("other/child")).unwrap();
    fs::write(work.join("codex-file"), b"not a directory").unwrap();
    symlink(root.join("other"), work.join("escape-link")).unwrap();
    symlink(work.join("Beta"), work.join("inside-link")).unwrap();
    let complete = |text: &str| launcher.complete_directories(text, 24).unwrap();
    assert!(complete("").is_empty());
    assert_eq!(
        complete(&work_text[..work_text.len() - 2]),
        vec![format!("{work_text}/")]
    );
    assert_eq!(complete(&work_text), vec![format!("{work_text}/")]);
    assert_eq!(
        complete(&format!("{work_text}/")),
        vec![
            format!("{work_text}/Beta/"),
            format!("{work_text}/claude-area/"),
            format!("{work_text}/codex-area/"),
            format!("{work_text}/escape-link/"),
            format!("{work_text}/inside-link/"),
        ]
    );
    assert_eq!(
        complete(&format!("{work_text}/co")),
        vec![format!("{work_text}/codex-area/")]
    );
    assert_eq!(
        complete(&format!("{work_text}/codex-area/")),
        vec![format!("{work_text}/codex-area/nested/")]
    );
    assert_eq!(
        complete(&format!("{work_text}/.")),
        vec![format!("{work_text}/work/")]
    );
    assert_eq!(
        launcher
            .complete_directories(&format!("{work_text}/"), 1)
            .unwrap(),
        vec![format!("{work_text}/Beta/")]
    );
    assert_eq!(
        complete(&format!("{work_text}/inside")),
        vec![format!("{work_text}/inside-link/")]
    );
    assert_eq!(
        complete(&format!("{work_text}/inside-link/")),
        vec![format!("{work_text}/inside-link/child/")]
    );
    for suffix in ["./", "/", "codex-area/../"] {
        let text = format!("{work_text}/{suffix}");
        assert!(complete(&text).contains(&format!("{text}Beta/")), "{text}");
    }
    assert_eq!(
        complete(&format!("{work_text}/escape-link/")),
        vec![format!("{work_text}/escape-link/child/")]
    );
    assert_eq!(
        complete(&(root.join("other").to_string_lossy().into_owned() + "/")),
        vec![format!("{}/other/child/", root.display())]
    );
    for text in [format!("{work_text}/missing/"), "relative/path".into()] {
        assert!(complete(&text).is_empty(), "{text}");
    }
    assert_eq!(
        launcher.complete_directories(&"x".repeat(4097), 24).err(),
        Some(Error::InvalidSpec)
    );
    assert!(
        launcher
            .complete_directories("a\nb", 24)
            .unwrap()
            .is_empty()
    );
    assert!(complete(&format!("{work_text}/claude-area/")).is_empty());
}

#[test]
fn a_leftover_bug_report_profiles_table_is_ignored() {
    // An earlier build shipped a per-source worker profile table with a fixed
    // cheapest-model policy; both are gone. Deployed launcher files that
    // still carry the key load like any file with an unknown key, and the
    // worker selects the source's one CLI exactly like `term/create`.
    let fixture = Fixture::new();
    let plain = read_config(&fixture.config_file(fixture.json().to_string().as_bytes())).unwrap();
    let mut json = fixture.json();
    json["bug_report_profiles"] = serde_json::json!({"codex": "shell-v1", "claude": "missing-v1"});
    let config = read_config(&fixture.config_file(json.to_string().as_bytes())).unwrap();
    assert_eq!(entries(&config), entries(&plain));
    let launcher = Launcher::new(config).unwrap();
    assert_eq!(
        launcher.entries(),
        &[Entry {
            id: "shell-v1".into(),
            source: Source::Codex,
            profile: false,
            resume: false,
        }]
    );
}

#[test]
fn existing_tab_named_working_directory_can_be_launched_and_completed() {
    let fixture = Fixture::new();
    let cwd = fixture.work.join("project\tname");
    fs::create_dir(&cwd).unwrap();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    let spec = LaunchSpec::new(Source::Codex, "shell-v1".into(), &cwd).unwrap();
    launcher.validate_spec(&spec).unwrap();
    let mut store = LifecycleStore::initialize(&fixture.directory.path().join("ledger")).unwrap();
    store.create("tab-cwd-request", &spec).unwrap();
    drop(store);
    let mut reopened = LifecycleStore::open(&fixture.directory.path().join("ledger")).unwrap();
    assert_eq!(reopened.list(0, 1).unwrap()[0].spec().cwd(), cwd);
    let prefix = format!("{}/project\t", fixture.work.display());
    assert_eq!(
        launcher.complete_directories(&prefix, 50).unwrap(),
        vec![format!("{}/", cwd.display())]
    );
}
