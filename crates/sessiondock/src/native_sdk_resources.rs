//! Read-only identities of dedicated SDK runtimes. Never starts a native host.
use crate::file_version::FileVersion;
use std::{
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

#[derive(Clone, serde::Deserialize)]
struct Identity {
    host_key: Option<String>,
    #[serde(default)]
    sid: Option<String>,
}
// Cache only tiny identities, never full Live/transcript values. Eviction merely
// re-reads a checkpoint; missing files are not retained.
static IDENTITIES: LazyLock<Mutex<indexmap::IndexMap<PathBuf, (FileVersion, Identity)>>> =
    LazyLock::new(Default::default);

pub(crate) fn checkpoint_host(state: &Path, source: &str, uid: &str, sid: &str) -> Option<String> {
    let key: String = uid.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let path: PathBuf = state
        .join("sdk-adapters")
        .join(source)
        .join(format!("{key}.json"));
    let mut cache = IDENTITIES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(version) = FileVersion::read(&path) else {
        cache.shift_remove(&path);
        return None;
    };
    let identity = if let Some((prior, identity)) = cache.get(&path)
        && *prior == version
    {
        identity.clone()
    } else {
        cache.shift_remove(&path);
        let identity: Identity =
            serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(&path).ok()?))
                .ok()?;
        cache.insert(path, (version, identity.clone()));
        while cache.len() > 1024 {
            cache.shift_remove_index(0);
        }
        identity
    };
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
