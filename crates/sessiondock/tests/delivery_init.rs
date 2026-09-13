//! Explicit local initialization only; never launches a model CLI or Web server.
use std::{
    fs,
    process::{Command, Output},
};

fn run(directory: &std::path::Path, args: &[&std::ffi::OsStr]) -> Output {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    Command::new(env!("CARGO_BIN_EXE_sessiondock"))
        .env_clear()
        .env(
            "SESSIONDOCK_BIND",
            occupied.local_addr().unwrap().to_string(),
        )
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_and_invalid_options_never_initialize_or_listen() {
    let root = tempfile::tempdir().unwrap();
    let help = run(root.path(), &["--help".as_ref()]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--initialize-delivery"));
    assert!(String::from_utf8_lossy(&help.stdout).contains("--initialize-lifecycle"));
    for args in [
        vec!["--initialize-delivery".as_ref()],
        vec!["--initialize-lifecycle".as_ref()],
        vec!["--unknown".as_ref()],
    ] {
        assert!(!run(root.path(), &args).status.success());
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn lifecycle_initialization_requires_explicit_private_directory_and_never_starts_web() {
    use std::os::unix::fs::DirBuilderExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("receipts");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    let args = ["--initialize-lifecycle".as_ref(), directory.as_os_str()];
    let output = run(root.path(), &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = directory.join("lifecycle-ledger.json");
    let bytes = fs::read(&path).unwrap();
    assert!(!run(root.path(), &args).status.success());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(
        !run(
            root.path(),
            &["--initialize-lifecycle".as_ref(), "receipts".as_ref()]
        )
        .status
        .success()
    );
    let missing = root.path().join("missing");
    assert!(
        !run(
            root.path(),
            &["--initialize-lifecycle".as_ref(), missing.as_os_str()]
        )
        .status
        .success()
    );
    assert!(!missing.exists());
    drop(sessiondock::lifecycle::store::LifecycleStore::open(&directory).unwrap());
}

#[cfg(unix)]
#[test]
fn explicit_initialization_exits_without_web_and_never_reinitializes_existing_ledger() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private-ledger");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    // run() occupies the explicitly configured Web port; init must still succeed.
    let args = ["--initialize-delivery".as_ref(), directory.as_os_str()];
    let output = run(root.path(), &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ledger = directory.join("delivery-ledger.json");
    let original = fs::read(&ledger).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert!(value.is_object());
    assert!(!run(root.path(), &args).status.success());
    assert_eq!(fs::read(&ledger).unwrap(), original);
    // A missing directory is never created as a side effect of initialization.
    let missing = root.path().join("missing");
    assert!(
        !run(
            root.path(),
            &["--initialize-delivery".as_ref(), missing.as_os_str()]
        )
        .status
        .success()
    );
    assert!(!missing.exists());
    assert!(
        !run(
            root.path(),
            &["--initialize-delivery".as_ref(), "private-ledger".as_ref()]
        )
        .status
        .success()
    );
    let engine = sessiondock::delivery::engine::DeliveryEngine::open(&directory).unwrap();
    drop(engine);
}
