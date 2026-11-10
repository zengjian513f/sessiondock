//! Private disk-backed cache for CLI and SDK model catalogs under
//! `<SESSIONDOCK_STATE_DIR>/models/`. Never touches any CLI's native home.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::lifecycle::launcher::{CliProfile, current_executable};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn hash_file_meta(hasher: &mut Sha256, path: &Path) {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    hasher.update(resolved.to_string_lossy().as_bytes());
    hasher.update([0]);
    if let Ok(meta) = fs::metadata(&resolved) {
        hasher.update(meta.len().to_le_bytes());
        if let Ok(modified) = meta.modified()
            && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
        {
            hasher.update(duration.as_nanos().to_le_bytes());
        }
    } else {
        hasher.update(b"missing");
    }
    hasher.update([0]);
}

pub fn profile_fingerprint(profile: &CliProfile) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"profile:v1\0");
    hasher.update(profile.id.as_bytes());
    hasher.update([0]);
    hasher.update(profile.source.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(profile.executable.to_string_lossy().as_bytes());
    hasher.update([0]);
    if let Ok(resolved) = current_executable(&profile.executable) {
        hash_file_meta(&mut hasher, &resolved);
    } else {
        hash_file_meta(&mut hasher, &profile.executable);
    }
    for arg in &profile.args {
        hasher.update(arg.as_bytes());
        hasher.update([0]);
        let candidate = Path::new(arg);
        if candidate.is_absolute() && candidate.is_file() {
            hash_file_meta(&mut hasher, candidate);
        }
    }
    hasher.update([0xff]);
    for (k, v) in &profile.env {
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update([0]);
    }
    hasher.update([0xfe]);
    for k in &profile.env_remove {
        hasher.update(k.as_bytes());
        hasher.update([0]);
    }
    hex(&hasher.finalize())
}

pub fn endpoint_fingerprint(label: &str, executable: Option<&Path>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"endpoint:v1\0");
    hasher.update(label.as_bytes());
    hasher.update([0]);
    if let Some(exe) = executable {
        if let Ok(resolved) = current_executable(exe) {
            hash_file_meta(&mut hasher, &resolved);
        } else {
            hash_file_meta(&mut hasher, exe);
        }
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub fn has_models(value: &Value) -> bool {
    let non_empty = |v: &Value| v.as_array().is_some_and(|rows| !rows.is_empty());
    non_empty(&value["models"])
        || non_empty(&value["data"])
        || non_empty(&value["raw"]["data"])
        || non_empty(&value["models"]["availableModels"])
        || value["configOptions"]
            .as_array()
            .is_some_and(|opts| opts.iter().any(|o| o["id"] == "model" && non_empty(&o["options"])))
}

pub fn load(path: &Path, fingerprint: &str) -> Option<(u64, Value)> {
    let bytes = fs::read(path).ok()?;
    let mut saved: Value = serde_json::from_slice(&bytes).ok()?;
    if saved["fingerprint"].as_str() != Some(fingerprint) {
        return None;
    }
    let updated_at_ms = saved["updated_at_ms"].as_u64().unwrap_or(0);
    let value = saved.get_mut("value").map(Value::take)?;
    if value.is_null() {
        return None;
    }
    Some((updated_at_ms, value))
}

pub fn save(path: &Path, fingerprint: &str, value: &Value) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(parent)?;
    let mut nonce = [0u8; 8];
    let _ = getrandom::fill(&mut nonce);
    let suffix = u64::from_le_bytes(nonce);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "models.json".into());
    let temp = parent.join(format!(".{name}.{suffix:016x}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let payload = json!({
        "fingerprint": fingerprint,
        "updated_at_ms": now_ms(),
        "value": value,
    });
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec(&payload)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
