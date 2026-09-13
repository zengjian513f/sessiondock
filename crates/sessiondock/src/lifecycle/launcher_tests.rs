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
            cwd_roots: vec![root.join("work")],
            adapters: vec![Adapter {
                id: "shell-v1".into(),
                source: Source::Codex,
                executable: root.join("bin/adapter"),
                args: vec![],
                env: BTreeMap::new(),
            }],
            profiles: vec![],
            bug_report_profiles: Default::default(),
        };
        Self { directory, config }
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
            "cwd_roots":self.config.cwd_roots,"adapters":[{"id":"shell-v1","source":"codex",
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
fn explicit_allowlist_accepts_only_its_source_adapter_and_cwd() {
    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    assert_eq!(launcher.host_dir(), fixture.config.host_dir);
    assert!(launcher.validate_spec(&fixture.spec()).is_ok());
    let child = fixture.config.cwd_roots[0].join("nested");
    fs::create_dir(&child).unwrap();
    assert!(
        launcher
            .validate_spec(&LaunchSpec::new(Source::Codex, "shell-v1".into(), &child).unwrap())
            .is_ok()
    );
    for (source, adapter, cwd) in [
        (
            Source::Claude,
            "shell-v1",
            fixture.config.cwd_roots[0].clone(),
        ),
        (
            Source::Codex,
            "unknown-v1",
            fixture.config.cwd_roots[0].clone(),
        ),
        (
            Source::Codex,
            "shell-v1",
            fixture.directory.path().join("other"),
        ),
    ] {
        let spec = LaunchSpec::new(source, adapter.into(), &cwd).unwrap();
        assert!(launcher.validate_spec(&spec).is_err());
    }
}

#[test]
fn configuration_budgets_and_versioned_adapter_keys_fail_closed() {
    let fixture = Fixture::new();
    let mutate = |change: fn(&mut Config)| {
        let mut config = fixture.config.clone();
        change(&mut config);
        assert!(Launcher::new(config).is_err());
    };
    for change in [
        (|c: &mut Config| c.cwd_roots.clear()) as fn(&mut Config),
        |c| c.cwd_roots = vec![c.cwd_roots[0].clone(); 17],
        |c| c.adapters.clear(),
        |c| c.adapters = vec![c.adapters[0].clone(); 17],
        |c| c.adapters.push(c.adapters[0].clone()),
        |c| c.adapters[0].id = "unversioned".into(),
        |c| c.adapters[0].id = "shell-v0".into(),
        |c| c.adapters[0].id = "shell-v01".into(),
        |c| c.adapters[0].args = vec![String::new(); 65],
        |c| c.adapters[0].args = vec!["x".repeat(4097)],
        |c| c.adapters[0].args = vec!["contains\0nul".into()],
        |c| {
            c.adapters[0].env.insert("BAD=KEY".into(), "x".into());
        },
        |c| {
            c.adapters[0].env.insert("1BAD".into(), "x".into());
        },
        |c| {
            c.adapters[0].env.insert("OK".into(), "x".repeat(8193));
        },
        |c| {
            c.adapters[0]
                .env
                .insert("OK".into(), "contains\0nul".into());
        },
        |c| c.adapters[0].args = vec!["x".repeat(4096); 17],
    ] {
        mutate(change);
    }
}

#[test]
fn constructor_rejects_symlinks_overlap_broad_roots_and_permissions() {
    let fixture = Fixture::new();
    let mut config = fixture.config.clone();
    config.cwd_roots.push(config.cwd_roots[0].clone());
    assert!(Launcher::new(config).is_err());
    let mut config = fixture.config.clone();
    config.cwd_roots = vec![fixture.directory.path().to_owned()];
    assert!(Launcher::new(config).is_err());
    let mut config = fixture.config.clone();
    config.cwd_roots = vec![PathBuf::from("/")];
    assert!(Launcher::new(config).is_err());
    let alias = fixture.directory.path().join("alias");
    symlink(&fixture.config.cwd_roots[0], &alias).unwrap();
    let mut config = fixture.config.clone();
    config.cwd_roots = vec![alias];
    assert_eq!(Launcher::new(config).err(), Some(Error::UnsafePath));
    // A symlinked executable resolves to the real file (Python `shutil.which`).
    let alias = fixture.directory.path().join("host-alias");
    symlink(&fixture.config.host_binary, &alias).unwrap();
    let mut config = fixture.config.clone();
    config.host_binary = alias;
    assert!(Launcher::new(config).is_ok());
    fs::set_permissions(&fixture.config.host_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        Launcher::new(fixture.config.clone()).err(),
        Some(Error::UnsafePermissions)
    );
    fs::set_permissions(&fixture.config.host_dir, fs::Permissions::from_mode(0o700)).unwrap();
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
fn relative_noncanonical_and_deserialized_cwd_cannot_bypass_checks() {
    let fixture = Fixture::new();
    for path in [
        PathBuf::from("relative"),
        fixture.directory.path().join("work/../work"),
        PathBuf::from(format!("{}/work//", fixture.directory.path().display())),
    ] {
        let mut config = fixture.config.clone();
        config.cwd_roots = vec![path];
        assert!(Launcher::new(config).is_err());
    }
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    let spec: LaunchSpec =
        serde_json::from_value(serde_json::json!({"source":"codex","adapter_id":"shell-v1",
        "cwd":fixture.directory.path().join("missing"),"launch":{"kind":"fixed"}}))
        .unwrap();
    assert!(launcher.validate_spec(&spec).is_err());
}

#[test]
fn launch_rechecks_executable_directory_and_cwd_identity() {
    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    fs::write(&fixture.config.host_binary, b"changed").unwrap();
    assert_eq!(launcher.validate_spec(&fixture.spec()), Err(Error::Changed));

    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    fs::rename(
        &fixture.config.cwd_roots[0],
        fixture.directory.path().join("previous-work"),
    )
    .unwrap();
    fs::create_dir(&fixture.config.cwd_roots[0]).unwrap();
    assert_eq!(launcher.validate_spec(&fixture.spec()), Err(Error::Changed));

    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.config.clone()).unwrap();
    let nested = fixture.config.cwd_roots[0].join("nested");
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
fn explicit_private_config_is_bounded_strict_and_never_echoes_input() {
    let fixture = Fixture::new();
    let raw = fixture.json().to_string();
    let path = fixture.config_file(raw.as_bytes());
    assert!(Launcher::new(read_config(&path).unwrap()).is_ok());
    for raw in [
        raw.replace(
            "\"env\":{}",
            "\"env\":{\"PRIVATE_ENV\":\"one\",\"PRIVATE_ENV\":\"two\"}",
        ),
        raw.replace(
            "\"id\":\"shell-v1\"",
            "\"id\":\"shell-v1\",\"id\":\"shell-v2\"",
        ),
        raw.replacen('{', "{\"secret-extra\":\"PRIVATE_SECRET\",", 1),
        " ".repeat(MAX_CONFIG_BYTES + 1),
    ] {
        let path = fixture.config_file(raw.as_bytes());
        let error = read_config(&path).err().unwrap();
        let rendered = format!("{error:?} {error}");
        assert!(
            !rendered.contains("PRIVATE")
                && !rendered.contains(fixture.directory.path().to_str().unwrap())
        );
    }
    let path = fixture.config_file(fixture.json().to_string().as_bytes());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(read_config(&path).err(), Some(Error::UnsafePermissions));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let hardlink = fixture.directory.path().join("hardlink.json");
    fs::hard_link(&path, &hardlink).unwrap();
    assert_eq!(read_config(&path).err(), Some(Error::UnsafePermissions));
    let alias = fixture.directory.path().join("config-alias.json");
    symlink(&path, &alias).unwrap();
    assert_eq!(read_config(&alias).err(), Some(Error::UnsafePath));
}

/// Explicit opt-in integration: both paths must be supplied, no repository,
/// HOME or PATH discovery, and no paid CLI may be used for the shell path.
#[test]
#[ignore = "set SESSIONDOCK_TEST_PTYHOST_BINARY and AGENTHUB_TEST_FREE_SHELL_BINARY explicitly"]
fn explicit_free_shell_starts_with_receipt_identity_and_survives_launcher_drop() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut fixture = Fixture::new();
    fixture.config.host_binary = PathBuf::from(
        std::env::var_os("SESSIONDOCK_TEST_PTYHOST_BINARY").expect("explicit test host binary"),
    );
    fixture.config.adapters[0].executable = PathBuf::from(
        std::env::var_os("AGENTHUB_TEST_FREE_SHELL_BINARY").expect("explicit free shell binary"),
    );
    fixture.config.adapters[0].args = vec!["-c".into(),
        "printf 'LAUNCH_ENV:%s:%s:%s\\n' \"${EXPLICIT_VALUE-unset}\" \"${HOME-unset}\" \"${TERM-unset}\"; while IFS= read -r line; do case \"$line\" in quit) exit 0;; *) printf 'RECEIVED:%s\\n' \"$line\";; esac; done".into()];
    fixture.config.adapters[0]
        .env
        .insert("EXPLICIT_VALUE".into(), "configured".into());
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
                .contains("LAUNCH_ENV:configured:unset:xterm-256color")
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
    /// synthetic claude/codex CLI profiles with narrower cwd roots.
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
                cwd_roots: vec![root.join("work/claude-area")],
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
                cwd_roots: vec![root.join("work/codex-area")],
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
                cwd_roots: vec![root.join("work")],
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
fn profile_schema_placeholders_environment_and_roots_fail_closed() {
    let fixture = Fixture::new();
    let base = fixture.profiles();
    assert!(Launcher::new(base.clone()).is_ok());
    let mutate = |change: fn(&mut Config)| {
        let mut config = base.clone();
        change(&mut config);
        assert!(Launcher::new(config).is_err());
    };
    for change in [
        // Versioning: profiles require schema 2; adapters remain optional.
        (|c: &mut Config| c.schema = 1) as fn(&mut Config),
        |c| c.schema = 3,
        |c| {
            c.adapters.clear();
            c.profiles.clear();
        },
        |c| c.profiles.push(c.profiles[0].clone()),
        |c| c.profiles[0].id = c.adapters[0].id.clone(),
        |c| c.profiles[0].id = "claude-cli".into(),
        |c| c.profiles = vec![c.profiles[0].clone(); 17],
        // Executable identity and template injection.
        |c| c.profiles[0].executable = PathBuf::from("/nonexistent/synthetic/claude"),
        |c| c.profiles[0].executable = PathBuf::from("bin/claude"),
        |c| c.profiles[0].new_args = vec!["--session-id".into()],
        |c| c.profiles[0].new_args = vec!["--session-id={session_id}".into()],
        |c| c.profiles[0].new_args = vec!["{session_id}".into(), "{session_id}".into()],
        |c| c.profiles[0].new_args = vec!["{session_id}".into(), "{sid}".into()],
        |c| c.profiles[0].resume_args = vec!["--resume".into()],
        |c| c.profiles[0].resume_args = vec!["--resume={sid}".into()],
        |c| c.profiles[0].resume_args = vec!["{sid}".into(), "{sid}".into()],
        |c| c.profiles[0].resume_args = vec!["{session_id}".into()],
        |c| c.profiles[0].resume_args = vec!["--resume".into(), "{sid} ".into()],
        |c| c.profiles[0].resume_args = vec!["--resume".into(), "{unknown}".into()],
        |c| c.profiles[0].args = vec!["{sid}".into()],
        |c| c.profiles[0].args = vec!["prefix{session_id}".into()],
        |c| c.profiles[1].new_args = vec!["--session-id".into(), "{session_id}".into()],
        |c| c.profiles[2].new_args = vec!["{session_id}".into()],
        |c| c.profiles[0].args = vec![String::new(); 63],
        |c| c.profiles[0].args = vec!["contains\0nul".into()],
        // Environment allowlist and always-denied identity variables.
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
        |c| {
            c.profiles[0].env.insert("TMUX".into(), "/tmp/x".into());
        },
        |c| {
            c.profiles[0]
                .env
                .insert("LD_PRELOAD".into(), "/x.so".into());
        },
        |c| {
            c.profiles[0]
                .env
                .insert("SYNTHETIC_PRIVATE".into(), "x".into());
        },
        |c| c.profiles[0].env_remove = vec!["HOME".into()],
        |c| c.profiles[0].env_remove = vec!["BAD=NAME".into()],
        |c| c.profiles[0].env_remove = vec!["TERM".into(), "TERM".into()],
        |c| c.profiles[0].env_remove = vec!["X".into(); 17],
        // Explicit cwd roots: nonempty, unique, inside a global root.
        |c| c.profiles[0].cwd_roots.clear(),
        |c| c.profiles[0].cwd_roots = vec![c.profiles[0].cwd_roots[0].clone(); 2],
        |c| c.profiles[0].cwd_roots = vec![c.host_dir.clone()],
        |c| c.profiles[0].cwd_roots = vec![c.host_dir.parent().unwrap().join("other")],
        |c| c.profiles[0].cwd_roots = vec![PathBuf::from("/")],
        |c| c.profiles[0].cwd_roots = vec![c.cwd_roots[0].join("missing")],
    ] {
        mutate(change);
    }
    let alias = fixture.directory.path().join("work/claude-alias");
    symlink(fixture.directory.path().join("work/claude-area"), &alias).unwrap();
    let mut config = base.clone();
    config.profiles[0].cwd_roots = vec![alias];
    assert_eq!(Launcher::new(config).err(), Some(Error::UnsafePath));
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
    // JSON: unknown fields, schema-1 profiles and duplicate keys fail closed.
    let mut json = serde_json::json!({"schema":2,"host_binary":base.host_binary,"host_dir":base.host_dir,
        "cwd_roots":base.cwd_roots,"adapters":[],"profiles":[{"id":"codex-cli-v1","source":"codex",
        "executable":base.profiles[1].executable,"args":[],"new_args":[],"resume_args":["resume","{sid}"],
        "env":{},"env_remove":[],"cwd_roots":[base.profiles[1].cwd_roots[0]]}]});
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
            worker: false,
        }]
    );
    json["profiles"][0]["shell"] = serde_json::json!("/bin/sh -c");
    assert_eq!(
        read_config(&fixture.config_file(json.to_string().as_bytes())).err(),
        Some(Error::InvalidConfig)
    );
    json["profiles"][0].as_object_mut().unwrap().remove("shell");
    json["schema"] = serde_json::json!(1);
    assert_eq!(
        read_config(&fixture.config_file(json.to_string().as_bytes())).err(),
        Some(Error::InvalidConfig)
    );
    json.as_object_mut().unwrap().remove("schema");
    assert_eq!(
        read_config(&fixture.config_file(json.to_string().as_bytes())).err(),
        Some(Error::InvalidConfig)
    );
    // Legacy schema-1 shape (no schema field, adapters only) still loads.
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
            worker: false,
        },
        Entry {
            id: "claude-cli-v1".into(),
            source: Source::Claude,
            profile: true,
            resume: true,
            worker: false,
        },
        Entry {
            id: "codex-cli-v1".into(),
            source: Source::Codex,
            profile: true,
            resume: true,
            worker: false,
        },
        Entry {
            id: "grok-cli-v1".into(),
            source: Source::Grok,
            profile: true,
            resume: false,
            worker: false,
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
    // Grok profile: pending new session is fine, resume unsupported here.
    launcher
        .validate_spec(&LaunchSpec::profile_new(Source::Grok, "grok-cli-v1".into(), &work).unwrap())
        .unwrap();
    assert_eq!(
        launcher
            .validate_spec(
                &LaunchSpec::resume(
                    Source::Grok,
                    "grok-cli-v1".into(),
                    &work,
                    SID.into(),
                    "grok:0123456789abcdef".into()
                )
                .unwrap()
            )
            .err(),
        Some(Error::InvalidSpec)
    );
    // Source/ID disagreement and cwd outside the profile's narrower roots.
    assert_eq!(
        launcher
            .validate_spec(
                &LaunchSpec::profile_new(Source::Codex, "claude-cli-v1".into(), &claude).unwrap()
            )
            .err(),
        Some(Error::AdapterUnavailable)
    );
    assert_eq!(
        launcher
            .validate_spec(
                &LaunchSpec::profile_new(Source::Claude, "claude-cli-v1".into(), &codex).unwrap()
            )
            .err(),
        Some(Error::InvalidSpec)
    );
    assert_eq!(
        launcher
            .validate_spec(
                &LaunchSpec::profile_new(Source::Claude, "claude-cli-v1".into(), &work).unwrap()
            )
            .err(),
        Some(Error::InvalidSpec)
    );
    assert_eq!(
        launcher
            .validate_spec(
                &LaunchSpec::profile_new(Source::Claude, "missing-v1".into(), &claude).unwrap()
            )
            .err(),
        Some(Error::AdapterUnavailable)
    );
    // A symlinked cwd inside the profile root still fails fresh ancestry checks.
    let alias = root.join("work/claude-area/alias");
    symlink(root.join("work/codex-area"), &alias).unwrap();
    let escaped: LaunchSpec = serde_json::from_value(
        serde_json::json!({"source":"claude","adapter_id":"claude-cli-v1",
        "cwd":alias,"launch":{"kind":"new_assigned"}}),
    )
    .unwrap();
    assert!(launcher.validate_spec(&escaped).is_err());
}

#[test]
fn directory_completion_is_bounded_inside_roots_and_skips_symlinks() {
    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.profiles()).unwrap();
    let root = fixture.directory.path();
    let work = root.join("work");
    let work_text = work.to_str().unwrap().to_owned();
    fs::create_dir(work.join(".hidden")).unwrap();
    fs::create_dir(work.join("Beta")).unwrap();
    fs::write(work.join("codex-file"), b"not a directory").unwrap();
    symlink(root.join("other"), work.join("escape-link")).unwrap();
    symlink(work.join("Beta"), work.join("inside-link")).unwrap();
    let complete = |text: &str| launcher.complete_directories(text, 24).unwrap();
    // Empty or partial root text suggests the roots themselves, nothing else.
    assert_eq!(complete(""), vec![format!("{work_text}/")]);
    assert_eq!(complete("/"), vec![format!("{work_text}/")]);
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
        vec![format!("{work_text}/.hidden/")]
    );
    assert_eq!(
        launcher
            .complete_directories(&format!("{work_text}/"), 1)
            .unwrap(),
        vec![format!("{work_text}/Beta/")]
    );
    // Symlinks (even those pointing inside), files, parents outside every
    // root, traversal and relative forms yield nothing and reveal nothing.
    for text in [
        format!("{work_text}/escape"),
        format!("{work_text}/inside"),
        format!("{work_text}/escape-link/"),
        format!("{work_text}/inside-link/"),
        format!("{work_text}/../"),
        format!("{work_text}/./"),
        format!("{work_text}//"),
        format!("{work_text}/codex-area/../"),
        format!("{work_text}/codex-area/.."),
        format!("{work_text}/missing/"),
        root.join("other").to_str().unwrap().to_owned() + "/",
        root.join("host").to_str().unwrap().to_owned() + "/",
        "/etc/".into(),
        "relative/path".into(),
        "~/".into(),
    ] {
        assert!(complete(&text).is_empty(), "{text}");
    }
    assert_eq!(
        launcher.complete_directories(&"x".repeat(4097), 24).err(),
        Some(Error::InvalidSpec)
    );
    assert_eq!(
        launcher.complete_directories("a\nb", 24).err(),
        Some(Error::InvalidSpec)
    );
    // The launcher-level completion covers the global roots; profile roots
    // narrow only the launch itself.
    assert!(complete(&format!("{work_text}/claude-area/")).is_empty());
}

#[test]
fn bug_report_worker_profiles_are_catalogued_but_not_interactive() {
    let fixture = Fixture::new();
    let mut config = fixture.profiles();
    let mut worker = config.profiles[0].clone();
    worker.id = "claude-bug-report-v1".into();
    worker.args = vec![
        "--model".into(),
        "claude-haiku-4-5-20251001".into(),
        "--effort".into(),
        "low".into(),
    ];
    config.profiles.push(worker);
    config.bug_report_profiles.claude = Some("claude-bug-report-v1".into());
    let launcher = Launcher::new(config.clone()).unwrap();
    let claude: Vec<&Entry> = launcher
        .entries()
        .iter()
        .filter(|entry| entry.source == Source::Claude)
        .collect();
    assert_eq!(claude.len(), 2);
    assert!(claude[0].interactive() && !claude[0].worker);
    assert_eq!(claude[1].id, "claude-bug-report-v1");
    assert!(claude[1].worker && !claude[1].interactive());
    // Exactly one interactive, resume-capable Claude profile remains.
    assert_eq!(
        entries(&config)
            .iter()
            .filter(|entry| entry.source == Source::Claude && entry.resume && entry.interactive())
            .count(),
        1
    );
}

#[test]
fn bug_report_policy_accepts_a_login_shell_wrapper_argv() {
    // Batch 44 WP-F: the profile executable may be a login-shell wrapper
    // (`with-zshrc`) with the CLI's PATH name as `args[0]`; the policy scans
    // the arguments after the executable, so the extra leading name changes
    // nothing, and a wrapper without the model/effort pairs still fails.
    let wrapped = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    assert!(bug_report_policy(
        Source::Claude,
        &wrapped(&[
            "claude",
            "--settings",
            "/etc/example/claude-bridge-settings.json",
            "--model",
            "claude-haiku-4-5-20251001",
            "--effort",
            "low",
            "--session-id",
            "{session_id}",
        ])
    ));
    assert!(bug_report_policy(
        Source::Codex,
        &wrapped(&[
            "codex",
            "--enable",
            "default_mode_request_user_input",
            "-c",
            "suppress_unstable_features_warning=true",
            "-m",
            "gpt-5.6-luna",
            "-c",
            "model_reasoning_effort=\"low\"",
        ])
    ));
    assert!(bug_report_policy(
        Source::Grok,
        &wrapped(&["grok", "-m", "grok-4.6", "--reasoning-effort", "low"])
    ));
    assert!(!bug_report_policy(
        Source::Claude,
        &wrapped(&[
            "claude",
            "--settings",
            "/etc/example/claude-bridge-settings.json"
        ])
    ));
    // The catalogue view keeps the wrapper as argv[0] and the policy reads argv[1..].
    let fixture = Fixture::new();
    let mut config = fixture.profiles();
    let mut worker = config.profiles[0].clone();
    worker.id = "claude-bug-report-v1".into();
    worker.args = wrapped(&[
        "claude",
        "--model",
        "claude-haiku-4-5-20251001",
        "--effort",
        "low",
    ]);
    config.profiles.push(worker);
    config.bug_report_profiles.claude = Some("claude-bug-report-v1".into());
    let resolved = bug_report_profiles(&config);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].argv[1], "claude");
    assert!(resolved[0].policy_ok());
}
