//! Node identity and credential files (`federation.identity`, `server.py`
//! `--node-token-file`). Loaders and checks only; the node listener wires them.

#[cfg(all(test, unix))]
mod tests;

use std::{
    fmt, fs,
    io::{self, Write},
    path::Path,
};

use subtle::ConstantTimeEq;

/// Wire protocol version both sides must state (`federation.PROTOCOL`).
pub const PROTOCOL: u32 = 1;

/// A node id is exactly 32 lowercase hex digits (`secrets.token_hex(16)`).
pub fn is_node_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// A node token is 32–256 characters of `[A-Za-z0-9._~+/=-]`.
pub fn is_token(value: &str) -> bool {
    (32..=256).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-')
        })
}

/// Read the persistent node id, minting one on first use. The file is created
/// with `O_EXCL` 0600 so two concurrent first starts cannot disagree; an
/// existing file is only read and must hold a valid id.
pub fn node_id(path: &Path) -> io::Result<String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let value = match options.open(path) {
        Ok(mut file) => {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|_| io::Error::other("system randomness unavailable"))?;
            let value: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            file.write_all(format!("{value}\n").as_bytes())?;
            file.sync_all()?;
            value
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            fs::read_to_string(path)?.trim().to_string()
        }
        Err(error) => return Err(error),
    };
    if !is_node_id(&value) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid node identity file",
        ));
    }
    Ok(value)
}

/// The shared hub credential a node expects in `X-SessionDock-Node-Token`.
/// Never printed: `Debug` redacts it.
#[derive(Clone, PartialEq, Eq)]
pub struct NodeToken(String);

impl NodeToken {
    /// Validate token text (already stripped of surrounding whitespace).
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        if is_token(value) {
            Ok(Self(value.to_string()))
        } else {
            Err("node token must contain 32–256 characters of [A-Za-z0-9._~+/=-]")
        }
    }

    /// Read `--node-token-file`: whole file, surrounding whitespace stripped.
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(path)?;
        Self::parse(text.trim())
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Constant-time comparison (`secrets.compare_digest`); a length mismatch
    /// is a mismatch.
    pub fn verify(&self, presented: &str) -> bool {
        let expected = self.0.as_bytes();
        let presented = presented.as_bytes();
        expected.len() == presented.len() && bool::from(expected.ct_eq(presented))
    }
}

impl fmt::Debug for NodeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NodeToken(<redacted>)")
    }
}
