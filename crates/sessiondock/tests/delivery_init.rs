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

#[test]
fn lifecycle_initialization_creates_absolute_and_relative_directories_without_starting_web() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("receipts");
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
    let relative = run(
        root.path(),
        &[
            "--initialize-lifecycle".as_ref(),
            "relative-receipts".as_ref(),
        ],
    );
    assert!(
        relative.status.success(),
        "{}",
        String::from_utf8_lossy(&relative.stderr)
    );
    assert!(
        root.path()
            .join("relative-receipts/lifecycle-ledger.json")
            .is_file()
    );
    drop(sessiondock::lifecycle::store::LifecycleStore::open(&directory).unwrap());
}

#[test]
fn delivery_initialization_creates_directories_and_never_reinitializes_existing_ledger() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("delivery-ledger");
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
    let missing = root.path().join("missing");
    let created = run(
        root.path(),
        &["--initialize-delivery".as_ref(), missing.as_os_str()],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(missing.join("delivery-ledger.json").is_file());
    let relative = run(
        root.path(),
        &[
            "--initialize-delivery".as_ref(),
            "relative-delivery".as_ref(),
        ],
    );
    assert!(
        relative.status.success(),
        "{}",
        String::from_utf8_lossy(&relative.stderr)
    );
    assert!(
        root.path()
            .join("relative-delivery/delivery-ledger.json")
            .is_file()
    );
    let engine = sessiondock::delivery::engine::DeliveryEngine::open(&directory).unwrap();
    drop(engine);
}
