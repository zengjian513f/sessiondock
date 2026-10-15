#[cfg(target_os = "linux")]
mod bandwidth;
#[cfg(target_os = "linux")]
mod events;
#[cfg(target_os = "linux")]
mod gpu;
#[cfg(target_os = "linux")]
mod io_events;
#[cfg(target_os = "linux")]
mod memory;
#[path = "../../runtime_log.rs"]
mod runtime_log;
#[cfg(target_os = "linux")]
mod server;
#[cfg(target_os = "linux")]
mod usage;
#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--io-probe-helper") {
        return io_events::helper_main();
    }
    let result = server::run();
    runtime_log::emit(
        if result.is_ok() { "info" } else { "error" },
        "resource.stopped",
        serde_json::json!({"error_kind": result.as_ref().err().map(|e| e.kind().to_string()),"delivery":runtime_log::snapshot()}),
    );
    runtime_log::shutdown();
    result
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("resource-agent currently supports Linux only");
    std::process::exit(1);
}
