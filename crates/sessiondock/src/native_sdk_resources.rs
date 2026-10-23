//! Read-only identities of dedicated SDK runtimes. Never starts a native host.
use std::path::{Path, PathBuf};

pub(crate) fn checkpoint_host(state: &Path, source: &str, uid: &str, sid: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Identity {
        host_key: Option<String>,
        #[serde(default)]
        sid: Option<String>,
    }
    let key: String = uid.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let path: PathBuf = state
        .join("sdk-adapters")
        .join(source)
        .join(format!("{key}.json"));
    // Skip transcript fields while decoding, rather than materializing them
    // for every catalog refresh. No directory creation or recovery side effects.
    let identity: Identity =
        serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(path).ok()?)).ok()?;
    if identity.sid.as_deref().is_some_and(|saved| saved != sid) {
        return None;
    }
    identity.host_key
}

pub(crate) async fn hosted_process(
    proc_root: &Path,
    directory: &Path,
    key: &str,
) -> Option<process_links::Process> {
    let client = sdkhost_client::Client::attach(directory, key).await.ok()?;
    let before = client.status().await.ok()?;
    if before.exit.is_some() {
        return None;
    }
    let (process, _) = process_links::linux::identity(proc_root, before.pid)?;
    let after = client.status().await.ok()?;
    (after.exit.is_none() && after.pid == process.pid).then_some(process)
}
