//! Configuration of the `sessiondock-hub` binary (batch 40 H4). Separate from
//! the node's `Config`: the hub owns no session root, host, ledger or state
//! directory — only the registry file, its cache, the allowed node networks,
//! the frontend snapshot and an optional diagnostics directory. Loopback only,
//! like the node; missing required settings fail closed.

use std::{
    env, io,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
};

use crate::hub::{
    Network,
    registry::{DEFAULT_NETWORKS, parse_networks},
};

/// Environment of `sessiondock-hub` (`SESSIONDOCK_HUB_*` plus the shared
/// `SESSIONDOCK_WEB_DIR` / `SESSIONDOCK_AUDIT_DIR`).
#[derive(Clone, Debug)]
pub struct HubConfig {
    /// `SESSIONDOCK_HUB_BIND`, default `127.0.0.1:8742`; loopback only.
    pub bind: SocketAddr,
    /// `SESSIONDOCK_HUB_NODES`: the registry file (`hub-nodes.json`, 0600).
    pub nodes_file: PathBuf,
    /// `SESSIONDOCK_HUB_CACHE_DIR`: offline session snapshots; default
    /// `hub-cache` next to the registry file (Python's layout).
    pub cache_dir: PathBuf,
    /// `SESSIONDOCK_HUB_NETWORKS`: CIDR list a node may be registered from.
    pub networks: Vec<Network>,
    /// The text of `networks` as configured, for `--check-config`.
    pub networks_text: String,
    /// `SESSIONDOCK_WEB_DIR`, default `legacy-web`; served in hub mode.
    pub web_dir: PathBuf,
    /// `SESSIONDOCK_AUDIT_DIR`: where `hub.node.*.changed` records go; unset
    /// keeps the hub silent about display/order changes.
    pub audit_dir: Option<PathBuf>,
    /// `SESSIONDOCK_PUBLIC_HOSTS`: authorities accepted besides loopback when
    /// the hub page sits behind the authenticated reverse proxy (same rule and
    /// parser as the node, `config::parse_public_hosts`).
    pub public_hosts: Vec<String>,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8742".parse().expect("constant socket address"),
            nodes_file: PathBuf::new(),
            cache_dir: PathBuf::new(),
            networks: parse_networks(DEFAULT_NETWORKS).expect("constant networks"),
            networks_text: DEFAULT_NETWORKS.to_string(),
            web_dir: "legacy-web".into(),
            audit_dir: None,
            public_hosts: Vec::new(),
        }
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

impl HubConfig {
    /// Read and validate the environment; nothing is opened or written.
    pub fn from_env() -> io::Result<Self> {
        let mut config = Self::default();
        if let Some(bind) = env::var_os("SESSIONDOCK_HUB_BIND") {
            config.bind = bind
                .to_str()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| invalid("invalid SESSIONDOCK_HUB_BIND"))?;
        }
        if let Some(hosts) = env::var_os("SESSIONDOCK_PUBLIC_HOSTS") {
            config.public_hosts = crate::config::parse_public_hosts(&hosts)?;
        }
        let nodes = env::var_os("SESSIONDOCK_HUB_NODES")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid("SESSIONDOCK_HUB_NODES must name the hub registry file"))?;
        config.nodes_file = PathBuf::from(nodes);
        config.cache_dir = match env::var_os("SESSIONDOCK_HUB_CACHE_DIR") {
            Some(value) if value.is_empty() => {
                return Err(invalid("SESSIONDOCK_HUB_CACHE_DIR must not be empty"));
            }
            Some(value) => PathBuf::from(value),
            None => config
                .nodes_file
                .parent()
                .map(|parent| parent.join("hub-cache"))
                .unwrap_or_default(),
        };
        if let Some(value) = env::var_os("SESSIONDOCK_HUB_NETWORKS") {
            let text = value
                .to_str()
                .ok_or_else(|| invalid("invalid SESSIONDOCK_HUB_NETWORKS"))?
                .to_string();
            config.networks = parse_networks(&text)
                .map_err(|error| invalid(format!("invalid SESSIONDOCK_HUB_NETWORKS: {error}")))?;
            config.networks_text = text;
        }
        if let Some(path) = env::var_os("SESSIONDOCK_WEB_DIR") {
            config.web_dir = path.into();
        }
        config.audit_dir = env::var_os("SESSIONDOCK_AUDIT_DIR").map(PathBuf::from);
        config.validate()?;
        Ok(config)
    }

    /// The same checks startup performs (`--check-config` runs only these).
    pub fn validate(&self) -> io::Result<()> {
        if !self.bind.ip().is_loopback() {
            return Err(invalid(
                "the hub has no authentication of its own; SESSIONDOCK_HUB_BIND must be a loopback address",
            ));
        }
        if !self.web_dir.is_dir() {
            return Err(invalid("SESSIONDOCK_WEB_DIR must be an existing directory"));
        }
        let nodes = &self.nodes_file;
        if !nodes.is_absolute() || nodes.components().any(|part| part == Component::ParentDir) {
            return Err(invalid(
                "SESSIONDOCK_HUB_NODES must be an absolute path without relative jumps",
            ));
        }
        match std::fs::symlink_metadata(nodes) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    return Err(invalid(
                        "SESSIONDOCK_HUB_NODES must be a regular file, not a link or directory",
                    ));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o077 != 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "SESSIONDOCK_HUB_NODES holds node credentials and must not be group/other readable",
                        ));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !nodes.parent().is_some_and(Path::is_dir) {
                    return Err(invalid(
                        "SESSIONDOCK_HUB_NODES parent directory must exist (the registry is written there on first registration)",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
        if !self.cache_dir.is_absolute()
            || self
                .cache_dir
                .components()
                .any(|part| part == Component::ParentDir)
        {
            return Err(invalid(
                "SESSIONDOCK_HUB_CACHE_DIR must be an absolute path without relative jumps",
            ));
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&self.cache_dir)
            && !metadata.is_dir()
        {
            return Err(invalid("SESSIONDOCK_HUB_CACHE_DIR must be a directory"));
        }
        if let Some(audit) = &self.audit_dir {
            crate::audit::validate_directory(audit)?;
        }
        // Registry, cache and audit are private hub data; the frontend
        // snapshot is public. None may contain another.
        let web = std::path::absolute(&self.web_dir)?;
        let private: Vec<(&str, PathBuf)> = [
            ("registry", self.nodes_file.clone()),
            ("cache", self.cache_dir.clone()),
        ]
        .into_iter()
        .chain(self.audit_dir.iter().map(|dir| ("audit", dir.clone())))
        .collect();
        for (name, path) in &private {
            if path.starts_with(&web) || web.starts_with(path) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("the hub {name} path must stay outside the frontend directory"),
                ));
            }
        }
        for (index, (left_name, left)) in private.iter().enumerate() {
            for (right_name, right) in &private[index + 1..] {
                if left.starts_with(right) || right.starts_with(left) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("the hub {left_name} and {right_name} paths must be separate"),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(dir: &Path) -> HubConfig {
        HubConfig {
            nodes_file: dir.join("hub-nodes.json"),
            cache_dir: dir.join("hub-cache"),
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            ..HubConfig::default()
        }
    }

    #[test]
    fn loopback_bind_and_private_registry_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = base(dir.path());
        config.validate().unwrap();
        let public = HubConfig {
            bind: "0.0.0.0:8742".parse().unwrap(),
            ..config.clone()
        };
        assert!(public.validate().is_err());
        let relative = HubConfig {
            nodes_file: PathBuf::from("hub-nodes.json"),
            ..config.clone()
        };
        assert!(relative.validate().is_err());
        std::fs::write(&config.nodes_file, "[]").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config.nodes_file, std::fs::Permissions::from_mode(0o644))
                .unwrap();
            assert_eq!(
                config.validate().unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
            std::fs::set_permissions(&config.nodes_file, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        config.validate().unwrap();
        let missing_parent = HubConfig {
            nodes_file: dir.path().join("absent/hub-nodes.json"),
            ..config.clone()
        };
        assert!(missing_parent.validate().is_err());
    }

    #[test]
    fn cache_and_audit_stay_apart_from_the_frontend() {
        let dir = tempfile::tempdir().unwrap();
        let config = base(dir.path());
        let inside_web = HubConfig {
            cache_dir: config.web_dir.join("hub-cache"),
            ..config.clone()
        };
        assert!(inside_web.validate().is_err());
        let audit = dir.path().join("audit");
        std::fs::create_dir(&audit).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&audit, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let with_audit = HubConfig {
            audit_dir: Some(audit.clone()),
            ..config.clone()
        };
        with_audit.validate().unwrap();
        let shared = HubConfig {
            cache_dir: audit.clone(),
            audit_dir: Some(audit),
            ..config
        };
        assert!(shared.validate().is_err());
    }
}
