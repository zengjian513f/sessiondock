//! SessionDock configuration. Paths come from explicit environment variables;
//! missing optional directories leave the capability disabled.

use std::{env, io, net::SocketAddr, path::PathBuf};

use crate::sessions::SessionRoots;

/// Development-only configuration. No implicit CLI home or production state paths.
pub struct Config {
    pub bind: SocketAddr,
    pub web_dir: PathBuf,
    pub roots: SessionRoots,
    /// Names file; defaults beside the configured Codex sessions root.
    pub codex_index: Option<PathBuf>,
    /// Opt-in isolated terminal transport. No implicit host discovery.
    pub ptyhost_dir: Option<PathBuf>,
    /// SessionDock-owned preferences and grants.
    pub state_dir: Option<PathBuf>,
    /// Delivery state directory. Configuration itself does not open the ledger.
    pub delivery_dir: Option<PathBuf>,
    /// Creation receipts directory.
    pub lifecycle_dir: Option<PathBuf>,
    /// Server-owned adapter/launcher JSON, not browser input.
    pub launcher_config: Option<PathBuf>,
    /// Legacy file-root values retained for configuration compatibility; they
    /// do not authorize or confine authenticated file reads.
    pub file_roots: Vec<PathBuf>,
    /// Legacy write-root values may enable the write service for configuration
    /// compatibility; they do not authorize or confine target paths.
    pub file_write_roots: Vec<PathBuf>,
    /// Write budgets (jobs, bytes, chunk, expiry); production defaults.
    pub file_write_limits: crate::files::WriteLimits,
    /// Directory for browser diagnostics JSONL.
    /// Unset keeps the audit capability disabled and its route `501`.
    pub audit_dir: Option<PathBuf>,
    /// Audit budgets; production defaults unless tests lower them explicitly.
    pub audit_limits: crate::audit::Limits,
    /// Directory for the session recycle bin. Unset keeps the
    /// `trash` capability disabled and the delete/trash routes `501`.
    pub trash_dir: Option<PathBuf>,
    /// Process table to scan; `/proc` by default and a synthetic tree in tests.
    pub proc_root: PathBuf,
    /// Optional override for `~/.grok/active_sessions.json`.
    pub grok_active: Option<PathBuf>,
    /// Second listener for Hub traffic. Honoured only together
    /// with the token file, the id file and the peer networks; any subset of
    /// the four is a startup error. Never a substitute for the loopback bind.
    pub node_bind: Option<SocketAddr>,
    /// Shared hub credential (`--node-token-file`): 32–256 chars of
    /// `[A-Za-z0-9._~+/=-]`, read once at startup, never printed.
    pub node_token_file: Option<PathBuf>,
    /// Persistent node identity (`--node-id-file`): 32 hex, minted `O_EXCL`
    /// 0600 on first start; an existing file is only read.
    pub node_id_file: Option<PathBuf>,
    /// Source networks the node listener accepts (strict CIDR list); every
    /// other peer is 403 before the token is looked at.
    pub node_peers: Vec<PeerNetwork>,
    /// Bug-report bundles: an
    /// configured bundle directory. Set together with the repository or not
    /// at all; unset keeps
    /// `capabilities.bug_report` false and `POST /api/bug-report` 501.
    pub bug_report_dir: Option<PathBuf>,
    /// The repository a bug-report worker investigates:
    /// the worker's cwd and the parent of
    /// `sessiondock_attachments/`.
    pub bug_report_repo: Option<PathBuf>,
    /// Exact public authorities an authenticating reverse proxy forwards
    /// (`Host $http_host`); the Host gate accepts them beside loopback and
    /// keys Origin on them. Lower-cased `host` or `host:port`; unset keeps
    /// the gate loopback-only.
    pub public_hosts: Vec<String>,
    /// The name the page, `<title>`, `/api/meta` and `/api/nodes` show for
    /// this machine: `SESSIONDOCK_HOSTNAME`
    /// when set, else the system host name, else `SessionDock`.
    pub hostname: String,
    /// Persistent search-text cache (`docs/read-model.md` "搜索"). Unset keeps
    /// the cache in memory only and disables the warm-up.
    pub search_cache_dir: Option<PathBuf>,
    /// Byte cap of the on-disk search-text cache, least recently used entries
    /// evicted first (`SESSIONDOCK_SEARCH_CACHE_BYTES`, default 1 GiB).
    pub search_cache_bytes: u64,
    /// Byte cap of the resident case-folded copies of cached bodies that the
    /// search prefilter reads, least recently used evicted first
    /// (`SESSIONDOCK_SEARCH_FOLD_BYTES`, default 128 MiB).
    pub search_fold_bytes: u64,
    /// Parse slots the search-text producer may use at once, shared by all
    /// searches and the warm-up; independent of the read worker pool
    /// (`SESSIONDOCK_SEARCH_WORKERS`, default `clamp(cpus/2, 2, 8)`).
    pub search_workers: usize,
    /// Seconds between low-priority background passes that refresh the
    /// search-text cache; 0 disables warm-up (`SESSIONDOCK_SEARCH_WARMUP`,
    /// default 300).
    pub search_warmup_secs: u64,
    /// Pool, page, runtime and cache budgets:
    /// `SESSIONDOCK_READ_WORKERS` blocking readers (default `clamp(cores/2, 8, 32)`;
    /// probes and response permits derive from it), `SESSIONDOCK_HISTORY_PAGE_EVENTS`
    /// events per history page (default 2000), `SESSIONDOCK_ASYNC_WORKERS`
    /// reactor threads (default `clamp(cores/8, 4, 16)`), `SESSIONDOCK_CACHE_ENTRIES`
    /// view/AST LRU entries (default 16), `SESSIONDOCK_VIEW_CACHE_MB` retained
    /// view bytes (default 128), `SESSIONDOCK_AST_CACHE_MB` retained decoded
    /// ASTs (default 64, 0 disables). See docs/performance.md and docs/read-model.md.
    pub pools: Pools,
}

/// The kernel's host name (Linux `/proc/sys/kernel/hostname`, other Unix
/// `gethostname(2)`, else the `HOSTNAME` variable, else Windows'
/// `COMPUTERNAME`); `SessionDock` when none is available.
pub fn system_hostname() -> String {
    let read = || {
        #[cfg(target_os = "linux")]
        if let Ok(name) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
            return Some(name);
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            // launchd starts services without HOSTNAME; ask the kernel.
            let mut buffer = [0u8; 256];
            // SAFETY: the kernel writes at most `buffer.len()` bytes.
            let rc = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
            if rc == 0 {
                let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
                if let Ok(name) = std::str::from_utf8(&buffer[..end])
                    && !name.trim().is_empty()
                {
                    return Some(name.to_owned());
                }
            }
        }
        env::var("HOSTNAME")
            .ok()
            .or_else(|| env::var("COMPUTERNAME").ok())
    };
    read()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "SessionDock".to_owned())
}

/// Admission and cache budgets. The read pool is the only one sized directly
/// (`SESSIONDOCK_READ_WORKERS`); the derived pools keep the ratios of the
/// original fixed sizes (4 readers : 2 probes : 8 responses). Requests queue
/// until a permit is available or their task is cancelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pools {
    /// Shared blocking readers for lists, messages, pages, files, media and
    /// metadata writes (`SESSIONDOCK_READ_WORKERS`, at least 1; default
    /// `clamp(available_parallelism / 2, 8, 32)`). Searches never take one.
    pub read_workers: usize,
    /// Events per history page (`SESSIONDOCK_HISTORY_PAGE_EVENTS`, at least 1;
    /// default 2000).
    pub history_page_events: usize,
    /// Async runtime worker threads (`SESSIONDOCK_ASYNC_WORKERS`, at least 1;
    /// default `clamp(available_parallelism / 8, 4, 16)`): the HTTP reactor
    /// is not CPU-bound, and every thread is another malloc arena.
    pub async_workers: usize,
    /// Parsed-file / view LRU entries (`SESSIONDOCK_CACHE_ENTRIES`, default
    /// 16); zero disables retained entries.
    pub cache_entries: usize,
    /// Serialized message bytes plus resident embedded images the view LRU
    /// keeps (`SESSIONDOCK_VIEW_CACHE_MB`, default 128). Resident
    /// memory is roughly 1.2–1.7× this figure (docs/read-model.md).
    pub view_cache_mb: usize,
    /// Estimated resident bytes of decoded ASTs kept for append reuse
    /// (`SESSIONDOCK_AST_CACHE_MB`, default 64; 0 disables reuse).
    pub ast_cache_mb: usize,
}

impl Pools {
    pub const MIN_READ_WORKERS: usize = 1;

    /// `clamp(available_parallelism / 2, 8, 32)`.
    pub fn default_read_workers() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get() / 2)
            .unwrap_or(8)
            .clamp(8, 32)
    }
    /// `clamp(available_parallelism / 8, 4, 16)`.
    pub fn default_async_workers() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get() / 8)
            .unwrap_or(4)
            .clamp(4, 16)
    }
    /// The in-process cache budgets these knobs describe.
    pub fn caches(&self) -> crate::sessions::budgets::Caches {
        crate::sessions::budgets::Caches {
            view_entries: self.cache_entries,
            view_bytes: self.view_cache_mb.saturating_mul(1024 * 1024),
            ast_entries: self.cache_entries / 2,
            ast_bytes: self.ast_cache_mb.saturating_mul(1024 * 1024),
        }
    }
    /// Concurrent managed-runtime observations (`/proc` scans): half the readers.
    pub fn runtime_probes(&self) -> usize {
        (self.read_workers / 2).max(2)
    }
    /// Response permits held through the body for history/media pages, file
    /// writes and lifecycle responses: twice the readers.
    pub fn responses(&self) -> usize {
        self.read_workers.saturating_mul(2).max(8)
    }
}

impl Default for Pools {
    fn default() -> Self {
        Self {
            read_workers: Self::default_read_workers(),
            history_page_events: 2000,
            async_workers: Self::default_async_workers(),
            cache_entries: 16,
            view_cache_mb: 128,
            ast_cache_mb: 64,
        }
    }
}

/// One entry of `SESSIONDOCK_NODE_PEERS`: the strict network plus its text as
/// configured, echoed by `--check-config`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerNetwork {
    pub text: String,
    pub network: crate::hub::Network,
}

impl std::str::FromStr for PeerNetwork {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let text = value.trim().to_string();
        let network = text.parse()?;
        Ok(Self { text, network })
    }
}

impl std::fmt::Display for PeerNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8741".parse().expect("constant socket address"),
            web_dir: "legacy-web".into(),
            roots: SessionRoots {
                claude: None,
                codex: None,
                grok: None,
            },
            codex_index: None,
            ptyhost_dir: None,
            state_dir: None,
            delivery_dir: None,
            lifecycle_dir: None,
            launcher_config: None,
            file_roots: Vec::new(),
            file_write_roots: Vec::new(),
            file_write_limits: Default::default(),
            audit_dir: None,
            audit_limits: Default::default(),
            trash_dir: None,
            proc_root: "/proc".into(),
            grok_active: None,
            node_bind: None,
            node_token_file: None,
            node_id_file: None,
            node_peers: Vec::new(),
            bug_report_dir: None,
            bug_report_repo: None,
            public_hosts: Vec::new(),
            hostname: system_hostname(),
            search_cache_dir: None,
            search_cache_bytes: 1024 * 1024 * 1024,
            search_fold_bytes: crate::search::cache::FOLD_BYTES,
            search_workers: default_search_workers(),
            search_warmup_secs: 300,
            pools: Pools::default(),
        }
    }
}

/// `SESSIONDOCK_PUBLIC_HOSTS`: the exact `host[:port]` authorities (lower-cased)
/// a reverse proxy forwards besides loopback. Shared by the node and the hub;
/// an empty or malformed list is a startup error rather than "no public host".
pub fn parse_public_hosts(hosts: &std::ffi::OsStr) -> io::Result<Vec<String>> {
    let hosts = hosts.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "SESSIONDOCK_PUBLIC_HOSTS must be a comma-separated host[:port] list",
        )
    })?;
    let mut parsed = Vec::new();
    for host in hosts.split(',').map(str::trim).filter(|h| !h.is_empty()) {
        if host.parse::<axum::http::uri::Authority>().is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("SESSIONDOCK_PUBLIC_HOSTS: invalid authority {host:?}"),
            ));
        }
        parsed.push(host.to_ascii_lowercase());
    }
    if parsed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SESSIONDOCK_PUBLIC_HOSTS must be a comma-separated host[:port] list",
        ));
    }
    Ok(parsed)
}

/// Half the CPUs, at least 2 and at most 8: a cold search parses in
/// parallel without taking the machine from the CLIs it observes.
pub fn default_search_workers() -> usize {
    std::thread::available_parallelism()
        .map(|cpus| cpus.get() / 2)
        .unwrap_or(2)
        .clamp(2, 8)
}

impl Config {
    pub fn from_env() -> io::Result<Self> {
        let mut config = Self::default();
        if let Some(bind) = env::var_os("SESSIONDOCK_BIND") {
            config.bind = bind.to_str().and_then(|s| s.parse().ok()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid SESSIONDOCK_BIND")
            })?;
        }
        if let Some(path) = env::var_os("SESSIONDOCK_WEB_DIR") {
            config.web_dir = path.into();
        }
        fn root(name: &str, allow_missing: bool) -> io::Result<Option<PathBuf>> {
            match env::var_os(name) {
                None => Ok(None),
                Some(value) if value.is_empty() => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} must not be empty"),
                )),
                Some(value) => {
                    let configured = PathBuf::from(value);
                    let path = match configured.canonicalize() {
                        Ok(path) => path,
                        Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {
                            return Ok(Some(std::path::absolute(configured)?));
                        }
                        Err(error) => return Err(error),
                    };
                    if !path.is_dir() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("{name} must be a directory"),
                        ));
                    }
                    Ok(Some(path))
                }
            }
        }
        config.roots = SessionRoots {
            claude: root("SESSIONDOCK_CLAUDE_ROOT", true)?,
            codex: root("SESSIONDOCK_CODEX_ROOT", true)?,
            grok: root("SESSIONDOCK_GROK_ROOT", true)?,
        };
        config.ptyhost_dir = root("SESSIONDOCK_PTYHOST_DIR", false)?;
        config.state_dir = root("SESSIONDOCK_STATE_DIR", false)?;
        // Keep the original spelling for the store's no-follow checks. Unlike
        // the legacy root helper, this must not canonicalize and store an alias.
        config.delivery_dir = env::var_os("SESSIONDOCK_DELIVERY_DIR").map(PathBuf::from);
        config.lifecycle_dir = env::var_os("SESSIONDOCK_LIFECYCLE_DIR").map(PathBuf::from);
        config.launcher_config = env::var_os("SESSIONDOCK_LAUNCHER_CONFIG").map(PathBuf::from);
        config.audit_dir = env::var_os("SESSIONDOCK_AUDIT_DIR").map(PathBuf::from);
        config.trash_dir = env::var_os("SESSIONDOCK_TRASH_DIR").map(PathBuf::from);
        config.codex_index = env::var_os("SESSIONDOCK_CODEX_INDEX")
            .map(PathBuf::from)
            .or_else(|| {
                config
                    .roots
                    .codex
                    .as_deref()
                    .and_then(|root| root.parent())
                    .map(|home| home.join("session_index.jsonl"))
            });
        if let Some(path) = env::var_os("SESSIONDOCK_PROC_ROOT") {
            config.proc_root = PathBuf::from(path);
        }
        config.grok_active = env::var_os("SESSIONDOCK_GROK_ACTIVE").map(PathBuf::from);
        if let Some(bind) = env::var_os("SESSIONDOCK_NODE_BIND") {
            config.node_bind =
                Some(bind.to_str().and_then(|s| s.parse().ok()).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "invalid SESSIONDOCK_NODE_BIND")
                })?);
        }
        config.node_token_file = env::var_os("SESSIONDOCK_NODE_TOKEN_FILE").map(PathBuf::from);
        config.node_id_file = env::var_os("SESSIONDOCK_NODE_ID_FILE").map(PathBuf::from);
        config.bug_report_dir = env::var_os("SESSIONDOCK_BUG_REPORT_DIR").map(PathBuf::from);
        config.bug_report_repo = env::var_os("SESSIONDOCK_BUG_REPORT_REPO").map(PathBuf::from);
        config.search_cache_dir = env::var_os("SESSIONDOCK_SEARCH_CACHE_DIR").map(PathBuf::from);
        if let Some(bytes) = env::var_os("SESSIONDOCK_SEARCH_CACHE_BYTES") {
            config.search_cache_bytes = bytes
                .to_str()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_SEARCH_CACHE_BYTES must be an integer",
                    )
                })?;
        }
        if let Some(bytes) = env::var_os("SESSIONDOCK_SEARCH_FOLD_BYTES") {
            config.search_fold_bytes = bytes
                .to_str()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_SEARCH_FOLD_BYTES must be an integer",
                    )
                })?;
        }
        if let Some(workers) = env::var_os("SESSIONDOCK_SEARCH_WORKERS") {
            config.search_workers = workers
                .to_str()
                .and_then(|s| s.parse::<usize>().ok())
                .map(|workers| workers.max(1))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_SEARCH_WORKERS must be an integer",
                    )
                })?;
        }
        if let Some(seconds) = env::var_os("SESSIONDOCK_SEARCH_WARMUP") {
            config.search_warmup_secs = seconds
                .to_str()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_SEARCH_WARMUP must be an interval in seconds from 0 (off)",
                    )
                })?;
        }
        if let Some(hosts) = env::var_os("SESSIONDOCK_PUBLIC_HOSTS") {
            config.public_hosts = parse_public_hosts(&hosts)?;
        }
        if let Some(name) = env::var_os("SESSIONDOCK_HOSTNAME") {
            let name = name
                .to_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_HOSTNAME must be a non-empty display name",
                    )
                })?;
            config.hostname = name.to_owned();
        }
        if let Some(peers) = env::var_os("SESSIONDOCK_NODE_PEERS") {
            let peers = peers.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SESSIONDOCK_NODE_PEERS must be a comma-separated CIDR list",
                )
            })?;
            for network in peers.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                config.node_peers.push(network.parse().map_err(|reason| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("SESSIONDOCK_NODE_PEERS: {reason}"),
                    )
                })?);
            }
            if config.node_peers.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SESSIONDOCK_NODE_PEERS must be a comma-separated CIDR list",
                ));
            }
        }
        fn at_least(name: &str, value: &std::ffi::OsStr, low: usize) -> io::Result<usize> {
            value
                .to_str()
                .and_then(|text| text.trim().parse::<usize>().ok())
                .filter(|number| *number >= low)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{name} must be an integer of at least {low}"),
                    )
                })
        }
        // Pool budgets; the derived pools follow `Pools`.
        if let Some(value) = env::var_os("SESSIONDOCK_READ_WORKERS") {
            config.pools.read_workers =
                at_least("SESSIONDOCK_READ_WORKERS", &value, Pools::MIN_READ_WORKERS)?;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_HISTORY_PAGE_EVENTS") {
            config.pools.history_page_events =
                at_least("SESSIONDOCK_HISTORY_PAGE_EVENTS", &value, 1)?;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_ASYNC_WORKERS") {
            config.pools.async_workers = at_least("SESSIONDOCK_ASYNC_WORKERS", &value, 1)?;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_CACHE_ENTRIES") {
            config.pools.cache_entries = at_least("SESSIONDOCK_CACHE_ENTRIES", &value, 0)?;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_VIEW_CACHE_MB") {
            config.pools.view_cache_mb = at_least("SESSIONDOCK_VIEW_CACHE_MB", &value, 0)?;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_AST_CACHE_MB") {
            config.pools.ast_cache_mb = at_least("SESSIONDOCK_AST_CACHE_MB", &value, 0)?;
        }
        if let Some(paths) = env::var_os("SESSIONDOCK_FILE_ROOTS") {
            for path in env::split_paths(&paths) {
                if !path.as_os_str().is_empty() {
                    config.file_roots.push(path);
                }
            }
        }
        if let Some(paths) = env::var_os("SESSIONDOCK_FILE_WRITE_ROOTS") {
            for path in env::split_paths(&paths) {
                if !path.as_os_str().is_empty() {
                    config.file_write_roots.push(path);
                }
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> io::Result<()> {
        if let Some(audit) = &self.audit_dir {
            crate::audit::validate_directory(audit)?;
        }
        if let Some(trash) = &self.trash_dir {
            crate::trash::validate_directory(trash)?;
        }
        self.validate_node()?;
        self.validate_bug_report()?;
        Ok(())
    }

    /// The node listener is all-or-nothing: bind, token file, id file and peer
    /// networks together, else fail closed. Files are checked read-only here
    /// (`--check-config` runs this too and must not mint an id); startup mints
    /// a missing id through `hub::identity::node_id`.
    fn validate_node(&self) -> io::Result<()> {
        let configured = [
            self.node_bind.is_some(),
            self.node_token_file.is_some(),
            self.node_id_file.is_some(),
            !self.node_peers.is_empty(),
        ];
        if configured.iter().all(|set| !set) {
            return Ok(());
        }
        if !configured.iter().all(|set| *set) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_NODE_BIND, SESSIONDOCK_NODE_TOKEN_FILE, SESSIONDOCK_NODE_ID_FILE and SESSIONDOCK_NODE_PEERS must be set together (the node listener fails closed)",
            ));
        }
        let (Some(bind), Some(token), Some(id)) = (
            self.node_bind,
            self.node_token_file.as_ref(),
            self.node_id_file.as_ref(),
        ) else {
            unreachable!("checked above");
        };
        // Port 0 is a fresh socket each time (tests); a fixed address is one listener.
        if bind == self.bind && bind.port() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_NODE_BIND must differ from SESSIONDOCK_BIND",
            ));
        }
        crate::hub::NodeToken::load(token).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("SESSIONDOCK_NODE_TOKEN_FILE: {error}"),
            )
        })?;
        match std::fs::read_to_string(id) {
            Ok(value) => {
                if !crate::hub::identity::is_node_id(value.trim()) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "SESSIONDOCK_NODE_ID_FILE holds an invalid node identity",
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    /// Bug-report configuration is all-or-nothing: the bundle directory and
    /// repository are configured together.
    fn validate_bug_report(&self) -> io::Result<()> {
        let (dir, _repo) = match (&self.bug_report_dir, &self.bug_report_repo) {
            (None, None) => return Ok(()),
            (Some(dir), Some(repo)) => (dir, repo),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SESSIONDOCK_BUG_REPORT_DIR and SESSIONDOCK_BUG_REPORT_REPO must be set together (bug reports fail closed)",
                ));
            }
        };
        crate::bug_report::validate_directory(dir)?;
        Ok(())
    }

    pub fn validate_launcher(
        &self,
        launcher: &crate::lifecycle::launcher::Config,
    ) -> io::Result<()> {
        let invalid = || {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "launcher host directory must match the configured ptyhost directory",
            )
        };
        let expected = self
            .ptyhost_dir
            .as_ref()
            .ok_or_else(invalid)?
            .canonicalize()
            .map_err(|_| invalid())?;
        if launcher.host_dir.canonicalize().map_err(|_| invalid())? != expected {
            return Err(invalid());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn lifecycle_and_launcher_paths_do_not_create_cross_root_gates() {
        use std::os::unix::fs::DirBuilderExt;
        let root = tempfile::tempdir().unwrap();
        for name in [
            "web", "native", "host", "state", "delivery", "receipts", "files",
        ] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.path().join(name))
                .unwrap();
        }
        let private = root.path().join("launcher.json");
        std::fs::write(&private, b"{}").unwrap();
        let mut config = super::Config {
            web_dir: root.path().join("web"),
            roots: crate::sessions::SessionRoots {
                codex: Some(root.path().join("native")),
                ..Default::default()
            },
            ptyhost_dir: Some(root.path().join("host")),
            state_dir: Some(root.path().join("state")),
            delivery_dir: Some(root.path().join("delivery")),
            lifecycle_dir: Some(root.path().join("receipts")),
            launcher_config: Some(private.clone()),
            file_roots: vec![root.path().join("files")],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        for name in ["web", "native", "host", "state", "delivery", "files"] {
            config.lifecycle_dir = Some(root.path().join(name));
            assert!(config.validate().is_ok(), "{name}");
        }
        config.lifecycle_dir = Some(root.path().join("receipts"));
        for name in [
            "web", "native", "host", "state", "delivery", "receipts", "files",
        ] {
            let file = root.path().join(name).join("launcher.json");
            std::fs::write(&file, b"{}").unwrap();
            config.launcher_config = Some(file);
            assert!(config.validate().is_ok(), "{name}");
        }
        config.launcher_config = Some(private);
        config.lifecycle_dir = Some("relative".into());
        assert!(config.validate().is_ok());
    }
    use super::*;

    #[test]
    fn delivery_search_cache_launcher_and_index_paths_have_no_configuration_policy() {
        let (temp, mut config) = names_config();
        for path in [
            PathBuf::new(),
            PathBuf::from("relative/../delivery"),
            temp.path().join("missing"),
            temp.path().join("names/session_index.jsonl"),
            config.web_dir.clone(),
        ] {
            config.delivery_dir = Some(path.clone());
            config.search_cache_dir = Some(path.clone());
            config.launcher_config = Some(path.clone());
            config.codex_index = Some(path);
            config.validate().unwrap();
        }
    }

    #[test]
    fn pool_budgets_default_by_parallelism_and_parse_positive_env() {
        const CHILD: &str = "SESSIONDOCK_TEST_POOLS_CONFIG_CHILD";
        if let Some(expected) = env::var_os(CHILD) {
            match Config::from_env() {
                Ok(config) => {
                    assert_eq!(expected, "valid");
                    assert_eq!(
                        config.pools.read_workers,
                        env::var("SESSIONDOCK_READ_WORKERS")
                            .unwrap()
                            .parse::<usize>()
                            .unwrap()
                    );
                    assert_eq!(
                        config.pools.history_page_events,
                        env::var("SESSIONDOCK_HISTORY_PAGE_EVENTS")
                            .unwrap()
                            .parse::<usize>()
                            .unwrap()
                    );
                }
                Err(error) => assert_eq!(expected, "invalid", "{error}"),
            }
            return;
        }
        let defaults = Pools::default();
        let cores = std::thread::available_parallelism().unwrap().get();
        assert_eq!(defaults.read_workers, (cores / 2).clamp(8, 32));
        assert_eq!(defaults.history_page_events, 2000);
        // Fixed 4 : 2 : 8 became W : W/2 : 2W, with small-pool floors.
        for (workers, probes, responses) in [
            (1, 2, 8),
            (8, 4, 16),
            (16, 8, 32),
            (32, 16, 64),
            (64, 32, 128),
        ] {
            let pools = Pools {
                read_workers: workers,
                ..Pools::default()
            };
            assert_eq!(
                (pools.runtime_probes(), pools.responses()),
                (probes, responses),
                "{workers}"
            );
        }
        let (_temp, config) = names_config();
        for (workers, events, expected) in [
            ("3", "7", "valid"),
            ("0", "7", "invalid"),
            ("65", "7", "valid"),
            ("3", "0", "invalid"),
            ("3", "10001", "valid"),
            ("three", "7", "invalid"),
        ] {
            let mut command = std::process::Command::new(env::current_exe().unwrap());
            for (name, _) in env::vars_os() {
                if name.to_string_lossy().starts_with("SESSIONDOCK_") {
                    command.env_remove(name);
                }
            }
            let output = command
                .args([
                    "--exact",
                    "config::tests::pool_budgets_default_by_parallelism_and_parse_positive_env",
                    "--nocapture",
                ])
                .env(CHILD, expected)
                .env("SESSIONDOCK_WEB_DIR", &config.web_dir)
                .env("SESSIONDOCK_READ_WORKERS", workers)
                .env("SESSIONDOCK_HISTORY_PAGE_EVENTS", events)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{workers}/{events}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn names_config() -> (tempfile::TempDir, Config) {
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "web", "claude", "codex", "grok", "host", "state", "files", "names",
        ] {
            std::fs::create_dir(temp.path().join(name)).unwrap();
        }
        let index = temp.path().join("names/session_index.jsonl");
        std::fs::write(&index, b"").unwrap();
        let config = Config {
            web_dir: temp.path().join("web"),
            roots: SessionRoots {
                claude: Some(temp.path().join("claude")),
                codex: Some(temp.path().join("codex")),
                grok: Some(temp.path().join("grok")),
            },
            codex_index: Some(index),
            ptyhost_dir: Some(temp.path().join("host")),
            state_dir: Some(temp.path().join("state")),
            file_roots: vec![temp.path().join("files")],
            ..Config::default()
        };
        (temp, config)
    }

    #[test]
    fn codex_index_has_no_configuration_path_gate() {
        let (temp, mut config) = names_config();
        for path in [
            PathBuf::new(),
            PathBuf::from("session_index.jsonl"),
            temp.path().join("missing.jsonl"),
            temp.path().join("names"),
        ] {
            config.codex_index = Some(path);
            config.validate().unwrap();
        }
        config.codex_index = Some(temp.path().join("names/session_index.jsonl"));
        config.roots.codex = None;
        config.validate().unwrap();
    }

    #[test]
    fn external_codex_index_is_kept_exact_and_read_only() {
        let (_temp, config) = names_config();
        let index = config.codex_index.clone().unwrap();
        let before = std::fs::read(&index).unwrap();
        config.validate().unwrap();
        assert_eq!(config.codex_index.as_ref(), Some(&index));
        let store = crate::sessions::SessionStore::with_metadata_and_names(
            config.roots,
            None,
            config.codex_index,
        );
        assert!(store.list(false).is_ok());
        assert_eq!(std::fs::read(index).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn codex_index_aliases_are_left_for_the_names_reader_to_validate() {
        use std::os::unix::fs::symlink;
        let (temp, mut config) = names_config();
        let native_index = temp.path().join("codex/session_index.jsonl");
        std::fs::write(&native_index, b"").unwrap();
        let native_alias = temp.path().join("native-index-alias");
        symlink(native_index, &native_alias).unwrap();
        config.codex_index = Some(native_alias.clone());
        config.validate().unwrap();
        assert_eq!(config.codex_index.as_ref(), Some(&native_alias));

        let external_alias = temp.path().join("external-index-alias");
        symlink(
            temp.path().join("names/session_index.jsonl"),
            &external_alias,
        )
        .unwrap();
        let parent_alias = temp.path().join("names-parent-alias");
        symlink(temp.path().join("names"), &parent_alias).unwrap();
        for alias in [external_alias, parent_alias.join("session_index.jsonl")] {
            config.codex_index = Some(alias.clone());
            config.validate().unwrap();
            assert_eq!(config.codex_index.as_ref(), Some(&alias));
            let store = crate::sessions::SessionStore::with_metadata_and_names(
                config.roots.clone(),
                None,
                config.codex_index.clone(),
            );
            let document = store.list(false).unwrap();
            assert_eq!(document["sessions"], serde_json::json!([]));
        }
    }

    #[test]
    fn no_implicit_data_roots_and_ordinary_listener_addresses() {
        let mut config = Config::default();
        assert!(
            config.roots.claude.is_none()
                && config.roots.codex.is_none()
                && config.roots.grok.is_none()
        );
        assert!(config.validate().is_ok());
        assert!(config.ptyhost_dir.is_none());
        assert!(config.state_dir.is_none());
        assert!(config.delivery_dir.is_none());
        assert!(config.file_roots.is_empty());
        assert!(config.codex_index.is_none());
        config.bind = "0.0.0.0:8741".parse().unwrap();
        assert!(config.validate().is_ok());
        config.bind = "[::1]:8741".parse().unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn runtime_and_native_directories_have_no_cross_root_config_gate() {
        let temp = tempfile::tempdir().unwrap();
        let web = temp.path().join("web");
        let private = web.join("private");
        std::fs::create_dir_all(&private).unwrap();
        let mut config = Config {
            web_dir: web,
            ptyhost_dir: Some(private.clone()),
            ..Config::default()
        };
        assert!(config.validate().is_ok());
        config.ptyhost_dir = None;
        config.roots.codex = Some(private);
        assert!(config.validate().is_ok());
        config.roots.codex = None;
        config.web_dir = temp.path().join("missing-web");
        config.state_dir = Some(temp.path().to_path_buf());
        config.ptyhost_dir = Some(temp.path().join("web"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn file_service_activation_roots_allow_authenticated_operator_paths() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["web", "native", "files", "state", "host"] {
            std::fs::create_dir(temp.path().join(name)).unwrap();
        }
        let mut config = Config {
            web_dir: temp.path().join("web"),
            roots: SessionRoots {
                codex: Some(temp.path().join("native")),
                ..Default::default()
            },
            state_dir: Some(temp.path().join("state")),
            ptyhost_dir: Some(temp.path().join("host")),
            file_roots: vec![temp.path().join("files")],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        for name in ["web", "native", "state", "host"] {
            config.file_roots = vec![temp.path().join(name)];
            assert!(config.validate().is_ok(), "{name}");
        }
        config.file_roots = vec![temp.path().to_owned()];
        assert!(config.validate().is_ok());
        config.file_write_roots = vec![temp.path().join("native"), temp.path().to_owned()];
        assert!(config.validate().is_ok());
    }

    #[cfg(unix)]
    fn node_files(temp: &std::path::Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let token = temp.join("node-token");
        std::fs::write(&token, format!("{}\n", "t0ken-".repeat(8))).unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        (token, temp.join("node-id"))
    }

    #[cfg(unix)]
    #[test]
    fn node_listener_needs_all_four_settings_and_fails_closed_on_any_subset() {
        let temp = tempfile::tempdir().unwrap();
        let (token, id) = node_files(temp.path());
        let full = Config {
            node_bind: Some("127.0.0.1:8742".parse().unwrap()),
            node_token_file: Some(token.clone()),
            node_id_file: Some(id.clone()),
            node_peers: vec!["127.0.0.0/8".parse().unwrap()],
            ..Config::default()
        };
        assert!(full.validate().is_ok(), "{:?}", full.validate());
        assert!(
            !id.exists(),
            "validation is read-only: the id is minted at startup"
        );
        for missing in 0..4 {
            let mut config = Config {
                node_bind: full.node_bind,
                node_token_file: full.node_token_file.clone(),
                node_id_file: full.node_id_file.clone(),
                node_peers: full.node_peers.clone(),
                ..Config::default()
            };
            match missing {
                0 => config.node_bind = None,
                1 => config.node_token_file = None,
                2 => config.node_id_file = None,
                _ => config.node_peers.clear(),
            }
            let error = config.validate().unwrap_err();
            assert!(
                error.to_string().contains("set together"),
                "{missing}: {error}"
            );
        }
        let unset = Config::default();
        assert!(unset.validate().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn node_listener_accepts_ordinary_paths_and_rejects_bad_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let (token, id) = node_files(temp.path());
        let full = || Config {
            node_bind: Some("127.0.0.1:8742".parse().unwrap()),
            node_token_file: Some(token.clone()),
            node_id_file: Some(id.clone()),
            node_peers: vec!["127.0.0.0/8".parse().unwrap()],
            ..Config::default()
        };
        let mut config = full();
        config.node_bind = Some("0.0.0.0:8742".parse().unwrap());
        assert!(config.validate().is_ok());
        let mut config = full();
        config.node_bind = Some(config.bind);
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("differ")
        );
        // Token grammar errors are startup errors, not silent downgrades.
        let short = temp.path().join("short");
        std::fs::write(&short, "short\n").unwrap();
        let mut config = full();
        config.node_token_file = Some(short);
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("32–256")
        );
        // An existing id file must already hold a node id. A missing parent is
        // created by `identity::node_id` at startup.
        std::fs::write(&id, "not-an-id\n").unwrap();
        assert!(
            full()
                .validate()
                .unwrap_err()
                .to_string()
                .contains("invalid node identity")
        );
        std::fs::write(&id, format!("{}\n", "e".repeat(32))).unwrap();
        assert!(full().validate().is_ok());
        std::fs::remove_file(&id).unwrap();
        let mut config = full();
        config.node_id_file = Some(temp.path().join("missing-dir").join("node-id"));
        assert!(config.validate().is_ok());
        // Cross-root placement does not add an authorization rule.
        let native = temp.path().join("claude");
        std::fs::create_dir(&native).unwrap();
        let mut config = full();
        config.roots.claude = Some(native.clone());
        config.node_id_file = Some(native.join("node-id"));
        assert!(config.validate().is_ok());
        let mut config = full();
        config.web_dir = temp.path().to_path_buf();
        assert!(config.validate().is_ok());
    }
}
