#[cfg(target_os = "linux")]
mod events;
#[cfg(target_os = "linux")]
mod gpu;
#[cfg(target_os = "linux")]
mod io_events;
#[cfg(target_os = "linux")]
mod memory;
#[cfg(target_os = "linux")]
mod server;
#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    server::run()
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("resource-agent currently supports Linux only");
    std::process::exit(1);
}
