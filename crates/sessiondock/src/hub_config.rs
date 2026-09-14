//! Configuration of the `sessiondock-hub` binary. Separate from
//! the node's `Config`: the hub owns no session root, host, ledger or state
//! directory — only the registry file, its cache, the allowed node networks,
//! the frontend snapshot and an optional diagnostics directory. Loopback only,
//! like the node.

use std::{env, io, net::SocketAddr, path::PathBuf};

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
    /// `SESSIONDOCK_HUB_NODES`: the registry file (`hub-nodes.json`).
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
        config.nodes_file = env::var_os("SESSIONDOCK_HUB_NODES")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local/share/sessiondock/hub-nodes.json"))
            })
            .ok_or_else(|| invalid("cannot find the home directory for the hub registry"))?;
        config.cache_dir =
            match env::var_os("SESSIONDOCK_HUB_CACHE_DIR").filter(|value| !value.is_empty()) {
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(dir: &std::path::Path) -> HubConfig {
        HubConfig {
            nodes_file: dir.join("hub-nodes.json"),
            cache_dir: dir.join("hub-cache"),
            web_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../legacy-web"),
            ..HubConfig::default()
        }
    }

    #[test]
    fn loopback_bind_and_web_directory_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let config = base(dir.path());
        config.validate().unwrap();
        let public = HubConfig {
            bind: "0.0.0.0:8742".parse().unwrap(),
            ..config.clone()
        };
        assert!(public.validate().is_err());
        let missing_web = HubConfig {
            web_dir: dir.path().join("missing-web"),
            ..config
        };
        assert!(missing_web.validate().is_err());
    }

    #[test]
    fn registry_cache_and_audit_paths_are_not_prevalidated() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = base(dir.path());
        config.nodes_file = PathBuf::from("relative/../hub-nodes.json");
        config.cache_dir = config.web_dir.join("hub-cache");
        config.audit_dir = Some(config.cache_dir.clone());
        config.validate().unwrap();

        config.nodes_file = dir.path().join("absent/parent/hub-nodes.json");
        config.validate().unwrap();

        config.nodes_file = dir.path().join("existing.json");
        std::fs::write(&config.nodes_file, "[]").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config.nodes_file, std::fs::Permissions::from_mode(0o644))
                .unwrap();
            let link = dir.path().join("linked-registry.json");
            std::os::unix::fs::symlink(&config.nodes_file, &link).unwrap();
            config.nodes_file = link;
        }
        config.validate().unwrap();

        config.cache_dir = dir.path().join("cache-is-a-file");
        std::fs::write(&config.cache_dir, "ordinary startup path").unwrap();
        config.validate().unwrap();
    }
}
