//! Development-only configuration. Paths come from explicit environment
//! variables; missing optional directories leave the capability disabled.
//! Only loopback binds are allowed. Private roots must not overlap frontend,
//! native, host, or each other. Never discovers a CLI home or initializes a ledger.

use std::{
    env, io,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
};

use crate::sessions::SessionRoots;

/// Development-only configuration. No implicit CLI home or production state paths.
pub struct Config {
    pub bind: SocketAddr,
    pub web_dir: PathBuf,
    pub roots: SessionRoots,
    /// Explicit names file, separate from the Codex sessions root. Never infer its parent.
    pub codex_index: Option<PathBuf>,
    /// Opt-in isolated terminal transport. No implicit host discovery.
    pub ptyhost_dir: Option<PathBuf>,
    /// Independent AgentHub-owned preferences; never a native CLI data directory.
    pub state_dir: Option<PathBuf>,
    /// Explicit independent delivery directory. Configuration never initializes a ledger.
    pub delivery_dir: Option<PathBuf>,
    /// Independent creation receipts, never initialized by Web startup.
    pub lifecycle_dir: Option<PathBuf>,
    /// Explicit private server-owned adapter/launcher JSON, not browser input.
    pub launcher_config: Option<PathBuf>,
    /// Explicit upper bound for session-reference-scoped file reads.
    pub file_roots: Vec<PathBuf>,
    /// Explicit write roots for uploads/actions; each must equal or lie inside
    /// a read root. Read roots never become writable implicitly. Unset keeps
    /// `files_jobs` false and the write routes `501`.
    pub file_write_roots: Vec<PathBuf>,
    /// Write budgets (jobs, bytes, chunk, expiry); production defaults.
    pub file_write_limits: crate::files::WriteLimits,
    /// Explicit private directory for bounded browser diagnostics JSONL.
    /// Unset keeps the audit capability disabled and its route `501`.
    pub audit_dir: Option<PathBuf>,
    /// Audit budgets; production defaults unless tests lower them explicitly.
    pub audit_limits: crate::audit::Limits,
    /// Explicit private directory for the session recycle bin. Unset keeps the
    /// `trash` capability disabled and the delete/trash routes `501`.
    pub trash_dir: Option<PathBuf>,
    /// Explicit read-only process-table scan for external CLIs (Linux `/proc`,
    /// Python `live.py`). Off by default: `/api/live` then observes only
    /// managed host instances and the `live` capability stays false.
    pub proc_scan: bool,
    /// Process table to scan; a synthetic tree in tests (Python `PROC_FS`).
    pub proc_root: PathBuf,
    /// Explicit Grok active-sessions file (Python `GROK_ACTIVE`); never
    /// discovered from a home directory.
    pub grok_active: Option<PathBuf>,
    /// Second listener for Hub traffic (batch 38 H1). Honoured only together
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
    /// Bug-report bundles (batch 41, Python `bug_report.REPORT_ROOT`): an
    /// explicit private 0700 directory disjoint from every other path. Set
    /// together with the repository or not at all; unset keeps
    /// `capabilities.bug_report` false and `POST /api/bug-report` 501.
    pub bug_report_dir: Option<PathBuf>,
    /// The repository a bug-report worker investigates (Python
    /// `PROJECT_ROOT`): the worker's cwd and the parent of
    /// `agenthub_attachments/`. Must equal or lie inside a file write root so
    /// report attachments go through the write service.
    pub bug_report_repo: Option<PathBuf>,
    /// Exact public authorities an authenticating reverse proxy forwards
    /// (`Host $http_host`); the Host gate accepts them beside loopback and
    /// keys Origin on them. Lower-cased `host` or `host:port`; unset keeps
    /// the gate loopback-only.
    pub public_hosts: Vec<String>,
    /// The name the page, `<title>`, `/api/meta` and `/api/nodes` show for
    /// this machine (Python `socket.gethostname()`): `SESSIONDOCK_HOSTNAME`
    /// when set, else the system host name, else `SessionDock`.
    pub hostname: String,
    /// Persistent search-text cache (`docs/read-model.md` "搜索"): an explicit
    /// existing private 0700 directory of its own (the metadata store keeps
    /// `SESSIONDOCK_STATE_DIR` to itself). Unset keeps the cache in memory
    /// only and disables the warm-up. Never a native CLI directory.
    pub search_cache_dir: Option<PathBuf>,
    /// Byte cap of the on-disk search-text cache, least recently used entries
    /// evicted first (`SESSIONDOCK_SEARCH_CACHE_BYTES`, default 1 GiB).
    pub search_cache_bytes: u64,
    /// Parse slots the search-text producer may use at once, shared by all
    /// searches and the warm-up; independent of the read worker pool
    /// (`SESSIONDOCK_SEARCH_WORKERS`, default `clamp(cpus/2, 2, 8)`).
    pub search_workers: usize,
    /// Seconds between low-priority background passes that refresh the
    /// search-text cache; 0 disables warm-up (`SESSIONDOCK_SEARCH_WARMUP`,
    /// default 300).
    pub search_warmup_secs: u64,
    /// Pool, page, runtime and cache budgets (batch 44 WP-A):
    /// `SESSIONDOCK_READ_WORKERS` blocking readers (default `clamp(cores/2, 8, 32)`;
    /// probes and response permits derive from it), `SESSIONDOCK_ADMISSION_WAIT_MS`
    /// queue wait before a 503 (default 10000), `SESSIONDOCK_HISTORY_PAGE_EVENTS`
    /// events per history page (default 2000), `SESSIONDOCK_ASYNC_WORKERS`
    /// reactor threads (default `clamp(cores/8, 4, 16)`), `SESSIONDOCK_CACHE_ENTRIES`
    /// view/AST LRU entries (default 16), `SESSIONDOCK_VIEW_CACHE_MB` retained
    /// view bytes (default 128), `SESSIONDOCK_AST_CACHE_MB` retained decoded
    /// ASTs (default 64, 0 disables). See docs/performance.md and docs/read-model.md.
    pub pools: Pools,
}

/// The kernel's host name (Linux `/proc/sys/kernel/hostname`, else the
/// `HOSTNAME` variable, else Windows' `COMPUTERNAME`); `SessionDock` when
/// none is available.
pub fn system_hostname() -> String {
    let read = || {
        #[cfg(target_os = "linux")]
        if let Ok(name) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
            return Some(name);
        }
        env::var("HOSTNAME")
            .ok()
            .or_else(|| env::var("COMPUTERNAME").ok())
    };
    read()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty() && name.len() <= 253)
        .unwrap_or_else(|| "SessionDock".to_owned())
}

/// Bounded admission budgets. The read pool is the only one sized directly
/// (`SESSIONDOCK_READ_WORKERS`); the derived pools keep the ratios of the
/// original fixed sizes (4 readers : 2 probes : 8 responses). A request that
/// cannot be admitted within `wait` is 503 `*_busy`; a request cancelled while
/// waiting leaves the queue without ever holding a permit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pools {
    /// Shared blocking readers for lists, messages, pages, files, media and
    /// metadata writes (`SESSIONDOCK_READ_WORKERS`, 1–64; default
    /// `clamp(available_parallelism / 2, 8, 32)`). Searches never take one.
    pub read_workers: usize,
    /// Bounded admission wait for every pool (`SESSIONDOCK_ADMISSION_WAIT_MS`,
    /// 0–60000; default 10000). `0` restores immediate rejection.
    pub wait: std::time::Duration,
    /// Events per history page (`SESSIONDOCK_HISTORY_PAGE_EVENTS`, 1–10000;
    /// default 2000) under the unchanged 8 MiB / 128-image page budget.
    pub history_page_events: usize,
    /// Async runtime worker threads (`SESSIONDOCK_ASYNC_WORKERS`, 1–64;
    /// default `clamp(available_parallelism / 8, 4, 16)`): the HTTP reactor
    /// is not CPU-bound, and every thread is another malloc arena.
    pub async_workers: usize,
    /// Parsed-file / view LRU entries (`SESSIONDOCK_CACHE_ENTRIES`, 1–256;
    /// default 16); the AST cache uses half, at least one.
    pub cache_entries: usize,
    /// Serialized message bytes plus resident embedded images the view LRU
    /// keeps (`SESSIONDOCK_VIEW_CACHE_MB`, 16–8192; default 128). Resident
    /// memory is roughly 1.2–1.7× this figure (docs/read-model.md).
    pub view_cache_mb: usize,
    /// Estimated resident bytes of decoded ASTs kept for append reuse
    /// (`SESSIONDOCK_AST_CACHE_MB`, 0–8192; default 64; 0 disables reuse).
    pub ast_cache_mb: usize,
}

impl Pools {
    pub const MIN_READ_WORKERS: usize = 1;
    pub const MAX_READ_WORKERS: usize = 64;
    pub const MAX_WAIT_MS: u64 = 60_000;
    pub const MAX_HISTORY_PAGE_EVENTS: usize = 10_000;
    pub const MAX_ASYNC_WORKERS: usize = 64;
    pub const MAX_CACHE_ENTRIES: usize = 256;
    pub const MAX_CACHE_MB: usize = 8192;

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
            view_bytes: self.view_cache_mb * 1024 * 1024,
            ast_entries: (self.cache_entries / 2).max(1),
            ast_bytes: self.ast_cache_mb * 1024 * 1024,
        }
    }
    /// Concurrent managed-runtime observations (`/proc` scans): half the readers.
    pub fn runtime_probes(&self) -> usize {
        (self.read_workers / 2).clamp(2, 16)
    }
    /// Response permits held through the body for history/media pages, file
    /// writes and lifecycle responses: twice the readers.
    pub fn responses(&self) -> usize {
        (self.read_workers * 2).clamp(8, 64)
    }
}

impl Default for Pools {
    fn default() -> Self {
        Self {
            read_workers: Self::default_read_workers(),
            wait: std::time::Duration::from_secs(10),
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
            proc_scan: false,
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
        fn root(name: &str) -> io::Result<Option<PathBuf>> {
            match env::var_os(name) {
                None => Ok(None),
                Some(value) if value.is_empty() => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} must not be empty"),
                )),
                Some(value) => {
                    let path = PathBuf::from(value).canonicalize()?;
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
            claude: root("SESSIONDOCK_CLAUDE_ROOT")?,
            codex: root("SESSIONDOCK_CODEX_ROOT")?,
            grok: root("SESSIONDOCK_GROK_ROOT")?,
        };
        config.ptyhost_dir = root("SESSIONDOCK_PTYHOST_DIR")?;
        config.state_dir = root("SESSIONDOCK_STATE_DIR")?;
        // Keep the original spelling for the store's no-follow checks. Unlike
        // the legacy root helper, this must not canonicalize and store an alias.
        config.delivery_dir = env::var_os("SESSIONDOCK_DELIVERY_DIR").map(PathBuf::from);
        config.lifecycle_dir = env::var_os("SESSIONDOCK_LIFECYCLE_DIR").map(PathBuf::from);
        config.launcher_config = env::var_os("SESSIONDOCK_LAUNCHER_CONFIG").map(PathBuf::from);
        config.audit_dir = env::var_os("SESSIONDOCK_AUDIT_DIR").map(PathBuf::from);
        config.trash_dir = env::var_os("SESSIONDOCK_TRASH_DIR").map(PathBuf::from);
        if let Some(path) = env::var_os("SESSIONDOCK_CODEX_INDEX") {
            config.codex_index = Some(PathBuf::from(path));
        }
        config.proc_scan = match env::var_os("SESSIONDOCK_PROC_SCAN") {
            None => false,
            Some(value) if value == "1" => true,
            Some(value) if value == "0" => false,
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SESSIONDOCK_PROC_SCAN must be 1 or 0",
                ));
            }
        };
        if let Some(path) = root("SESSIONDOCK_PROC_ROOT")? {
            config.proc_root = path;
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
        config.search_cache_dir = root("SESSIONDOCK_SEARCH_CACHE_DIR")?;
        if let Some(bytes) = env::var_os("SESSIONDOCK_SEARCH_CACHE_BYTES") {
            config.search_cache_bytes = bytes
                .to_str()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|bytes| *bytes >= 1024 * 1024)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_SEARCH_CACHE_BYTES must be an integer of at least 1048576 bytes",
                    )
                })?;
        }
        if let Some(workers) = env::var_os("SESSIONDOCK_SEARCH_WORKERS") {
            config.search_workers = workers
                .to_str()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|workers| (1..=64).contains(workers))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_SEARCH_WORKERS must be an integer from 1 to 64",
                    )
                })?;
        }
        if let Some(seconds) = env::var_os("SESSIONDOCK_SEARCH_WARMUP") {
            config.search_warmup_secs = seconds
                .to_str()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|seconds| *seconds <= 86_400)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_SEARCH_WARMUP must be an interval in seconds from 0 (off) to 86400",
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
                .filter(|name| !name.is_empty() && name.len() <= 253)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_HOSTNAME must be a non-empty display name (at most 253 bytes)",
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
        fn bounded(name: &str, value: &std::ffi::OsStr, low: u64, high: u64) -> io::Result<u64> {
            value
                .to_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
                .filter(|number| (low..=high).contains(number))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{name} must be an integer in {low}..={high}"),
                    )
                })
        }
        // Batch 44 WP-A: pool budgets; the derived pools follow `Pools`.
        if let Some(value) = env::var_os("SESSIONDOCK_READ_WORKERS") {
            config.pools.read_workers = bounded(
                "SESSIONDOCK_READ_WORKERS",
                &value,
                Pools::MIN_READ_WORKERS as u64,
                Pools::MAX_READ_WORKERS as u64,
            )? as usize;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_ADMISSION_WAIT_MS") {
            config.pools.wait = std::time::Duration::from_millis(bounded(
                "SESSIONDOCK_ADMISSION_WAIT_MS",
                &value,
                0,
                Pools::MAX_WAIT_MS,
            )?);
        }
        if let Some(value) = env::var_os("SESSIONDOCK_HISTORY_PAGE_EVENTS") {
            config.pools.history_page_events = bounded(
                "SESSIONDOCK_HISTORY_PAGE_EVENTS",
                &value,
                1,
                Pools::MAX_HISTORY_PAGE_EVENTS as u64,
            )? as usize;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_ASYNC_WORKERS") {
            config.pools.async_workers = bounded(
                "SESSIONDOCK_ASYNC_WORKERS",
                &value,
                1,
                Pools::MAX_ASYNC_WORKERS as u64,
            )? as usize;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_CACHE_ENTRIES") {
            config.pools.cache_entries = bounded(
                "SESSIONDOCK_CACHE_ENTRIES",
                &value,
                1,
                Pools::MAX_CACHE_ENTRIES as u64,
            )? as usize;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_VIEW_CACHE_MB") {
            config.pools.view_cache_mb = bounded(
                "SESSIONDOCK_VIEW_CACHE_MB",
                &value,
                16,
                Pools::MAX_CACHE_MB as u64,
            )? as usize;
        }
        if let Some(value) = env::var_os("SESSIONDOCK_AST_CACHE_MB") {
            config.pools.ast_cache_mb = bounded(
                "SESSIONDOCK_AST_CACHE_MB",
                &value,
                0,
                Pools::MAX_CACHE_MB as u64,
            )? as usize;
        }
        if let Some(paths) = env::var_os("SESSIONDOCK_FILE_ROOTS") {
            if paths.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SESSIONDOCK_FILE_ROOTS must not be empty",
                ));
            }
            for path in env::split_paths(&paths) {
                if path.as_os_str().is_empty() || config.file_roots.len() >= 16 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_FILE_ROOTS needs 1..16 explicit directories",
                    ));
                }
                if !path.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "file roots must be directories",
                    ));
                }
                config.file_roots.push(path);
            }
        }
        if let Some(paths) = env::var_os("SESSIONDOCK_FILE_WRITE_ROOTS") {
            if paths.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SESSIONDOCK_FILE_WRITE_ROOTS must not be empty",
                ));
            }
            for path in env::split_paths(&paths) {
                if path.as_os_str().is_empty() || config.file_write_roots.len() >= 16 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_FILE_WRITE_ROOTS needs 1..16 explicit directories",
                    ));
                }
                if !path.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "file write roots must be directories",
                    ));
                }
                config.file_write_roots.push(path);
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> io::Result<()> {
        if let Some(directory) = &self.lifecycle_dir {
            let resolved = validate_delivery_directory(directory).map_err(|error| {
                io::Error::new(error.kind(), "lifecycle requires an explicit existing private absolute directory without links")
            })?;
            for boundary in [
                Some(&self.web_dir),
                self.roots.claude.as_ref(),
                self.roots.codex.as_ref(),
                self.roots.grok.as_ref(),
                self.ptyhost_dir.as_ref(),
                self.state_dir.as_ref(),
                self.delivery_dir.as_ref(),
                self.codex_index.as_ref(),
                self.launcher_config.as_ref(),
            ]
            .into_iter()
            .flatten()
            .chain(self.file_roots.iter())
            {
                if overlaps(&resolved, &absolute_components(boundary)?)
                    || boundary
                        .canonicalize()
                        .is_ok_and(|path| overlaps(&resolved, &path))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "lifecycle receipts must be separate from all frontend, native, host, state, delivery, launcher and file access paths",
                    ));
                }
            }
        }
        if let Some(path) = &self.launcher_config {
            if !path.is_absolute() || !path.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "launcher configuration requires an explicit absolute existing private JSON file",
                ));
            }
            let resolved = path.canonicalize().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "launcher configuration path cannot be resolved",
                )
            })?;
            for boundary in [
                Some(&self.web_dir),
                self.roots.claude.as_ref(),
                self.roots.codex.as_ref(),
                self.roots.grok.as_ref(),
                self.ptyhost_dir.as_ref(),
                self.state_dir.as_ref(),
                self.delivery_dir.as_ref(),
                self.lifecycle_dir.as_ref(),
                self.codex_index.as_ref(),
            ]
            .into_iter()
            .flatten()
            .chain(self.file_roots.iter())
            {
                if overlaps(&resolved, &absolute_components(boundary)?)
                    || boundary
                        .canonicalize()
                        .is_ok_and(|other| overlaps(&resolved, &other))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "launcher configuration must remain outside frontend, native, runtime and file access paths",
                    ));
                }
            }
        }
        if let Some(delivery) = &self.delivery_dir {
            let resolved = validate_delivery_directory(delivery)?;
            let boundaries = [
                Some(&self.web_dir),
                self.roots.claude.as_ref(),
                self.roots.codex.as_ref(),
                self.roots.grok.as_ref(),
                self.ptyhost_dir.as_ref(),
                self.state_dir.as_ref(),
                self.codex_index.as_ref(),
                self.lifecycle_dir.as_ref(),
                self.launcher_config.as_ref(),
            ];
            for boundary in boundaries
                .into_iter()
                .flatten()
                .chain(self.file_roots.iter())
            {
                let lexical = absolute_components(boundary)?;
                // Lexical comparison also rejects an absent configured child
                // beneath delivery; resolving existing aliases closes the
                // converse case where an outside alias points into delivery.
                if overlaps(&resolved, &lexical)
                    || boundary
                        .canonicalize()
                        .is_ok_and(|path| overlaps(&resolved, &path))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "delivery requires an independent directory outside frontend, native, host, metadata, Codex index and file access roots",
                    ));
                }
            }
        }
        if let Some(audit) = &self.audit_dir {
            let resolved = crate::audit::validate_directory(audit)?;
            for boundary in [
                Some(&self.web_dir),
                self.roots.claude.as_ref(),
                self.roots.codex.as_ref(),
                self.roots.grok.as_ref(),
                self.ptyhost_dir.as_ref(),
                self.state_dir.as_ref(),
                self.delivery_dir.as_ref(),
                self.lifecycle_dir.as_ref(),
                self.codex_index.as_ref(),
                self.launcher_config.as_ref(),
            ]
            .into_iter()
            .flatten()
            .chain(self.file_roots.iter())
            {
                if overlaps(&resolved, &absolute_components(boundary)?)
                    || boundary
                        .canonicalize()
                        .is_ok_and(|path| overlaps(&resolved, &path))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "audit requires an independent directory outside frontend, native, host, state, delivery, lifecycle, launcher, Codex index and file access paths",
                    ));
                }
            }
        }
        if let Some(trash) = &self.trash_dir {
            let resolved = crate::trash::validate_directory(trash)?;
            for boundary in [
                Some(&self.web_dir),
                self.roots.claude.as_ref(),
                self.roots.codex.as_ref(),
                self.roots.grok.as_ref(),
                self.ptyhost_dir.as_ref(),
                self.state_dir.as_ref(),
                self.delivery_dir.as_ref(),
                self.lifecycle_dir.as_ref(),
                self.codex_index.as_ref(),
                self.launcher_config.as_ref(),
                self.audit_dir.as_ref(),
            ]
            .into_iter()
            .flatten()
            .chain(self.file_roots.iter())
            {
                if overlaps(&resolved, &absolute_components(boundary)?)
                    || boundary
                        .canonicalize()
                        .is_ok_and(|path| overlaps(&resolved, &path))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "trash requires an independent directory outside frontend, native, host, state, delivery, lifecycle, launcher, audit, Codex index and file access paths",
                    ));
                }
            }
        }
        if let Some(cache) = &self.search_cache_dir {
            let resolved = cache.canonicalize()?;
            for boundary in [
                Some(&self.web_dir),
                self.roots.claude.as_ref(),
                self.roots.codex.as_ref(),
                self.roots.grok.as_ref(),
                self.ptyhost_dir.as_ref(),
                self.state_dir.as_ref(),
                self.delivery_dir.as_ref(),
                self.lifecycle_dir.as_ref(),
                self.codex_index.as_ref(),
                self.launcher_config.as_ref(),
                self.audit_dir.as_ref(),
                self.trash_dir.as_ref(),
            ]
            .into_iter()
            .flatten()
            .chain(self.file_roots.iter())
            {
                if boundary
                    .canonicalize()
                    .is_ok_and(|path| overlaps(&resolved, &path))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "search cache requires an independent directory outside frontend, native, host, state, delivery, lifecycle, launcher, audit, trash, Codex index and file access paths",
                    ));
                }
            }
        }
        if let Some(index) = &self.codex_index
            && (self.roots.codex.is_none() || !index.is_absolute() || !index.is_file())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_CODEX_INDEX requires an explicit existing absolute file and a Codex sessions root",
            ));
        }
        if let Some(index) = &self.codex_index {
            // Resolve aliases only for overlap comparisons. Keep the caller's
            // exact index path in Config so names' no-follow component walk
            // can reject symlinks/reparse points instead of hiding them.
            let resolved_index = index.canonicalize().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the explicit Codex name index cannot be resolved",
                )
            })?;
            let directories = [
                Some(&self.web_dir),
                self.roots.claude.as_ref(),
                self.roots.codex.as_ref(),
                self.roots.grok.as_ref(),
                self.ptyhost_dir.as_ref(),
                self.state_dir.as_ref(),
                self.delivery_dir.as_ref(),
                self.lifecycle_dir.as_ref(),
                self.launcher_config.as_ref(),
            ];
            if directories
                .into_iter()
                .flatten()
                .chain(self.file_roots.iter())
                .filter_map(|directory| directory.canonicalize().ok())
                .any(|directory| {
                    resolved_index.starts_with(&directory) || directory.starts_with(&resolved_index)
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Codex name index must be separate from frontend, native, host, metadata and file access roots",
                ));
            }
        }
        if !self.bind.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Rust migration server has no authentication; only loopback binds are allowed",
            ));
        }
        self.validate_proc_scan()?;
        self.validate_node()?;
        self.validate_bug_report()?;
        // Static files are snapshotted before serving. Runtime/native trees
        // must never become public assets, even with an explicit dev config.
        let private: Vec<_> = [
            self.state_dir.as_ref(),
            self.ptyhost_dir.as_ref(),
            self.roots.claude.as_ref(),
            self.roots.codex.as_ref(),
            self.roots.grok.as_ref(),
            self.codex_index.as_ref(),
            self.delivery_dir.as_ref(),
            self.lifecycle_dir.as_ref(),
            self.launcher_config.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|path| path.canonicalize().ok())
        .collect();
        let files: Vec<_> = self
            .file_roots
            .iter()
            .filter_map(|path| path.canonicalize().ok())
            .collect();
        if files.iter().any(|root| {
            private
                .iter()
                .any(|path| root.starts_with(path) || path.starts_with(root))
        }) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "file access roots must not include private metadata, host or native input directories",
            ));
        }
        if let Ok(web) = self.web_dir.canonicalize()
            && private
                .iter()
                .chain(files.iter())
                .any(|path| path.starts_with(&web) || web.starts_with(path))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "frontend and private runtime/native directories must not overlap",
            ));
        }
        if let Some(state) = self
            .state_dir
            .as_ref()
            .and_then(|path| path.canonicalize().ok())
            && private
                .iter()
                .skip(1)
                .any(|path| path.starts_with(&state) || state.starts_with(path))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "metadata requires an independent directory outside host and native inputs",
            ));
        }
        self.validate_file_write_roots(&private, &files)?;
        Ok(())
    }

    /// Write roots are explicit and independent of read roots: each must be an
    /// existing absolute directory equal to or inside a configured read root,
    /// disjoint from every private path, and not nested in another write root.
    fn validate_file_write_roots(&self, private: &[PathBuf], files: &[PathBuf]) -> io::Result<()> {
        if self.file_write_roots.is_empty() {
            return Ok(());
        }
        if self.file_roots.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_FILE_WRITE_ROOTS requires SESSIONDOCK_FILE_ROOTS: writes are scoped through read-side session references",
            ));
        }
        if self.file_write_roots.len() > 16 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_FILE_WRITE_ROOTS needs 1..16 explicit directories",
            ));
        }
        let mut resolved: Vec<PathBuf> = Vec::new();
        for root in &self.file_write_roots {
            if !root.is_absolute()
                || root.components().any(|part| part == Component::ParentDir)
                || root.parent().is_none()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "file write roots must be explicit absolute directories below the filesystem root without relative jumps",
                ));
            }
            let canonical = root.canonicalize().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "file write roots must be existing directories",
                )
            })?;
            if !canonical.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "file write roots must be directories",
                ));
            }
            let lexical = absolute_components(root)?;
            if !files
                .iter()
                .any(|read| canonical.starts_with(read) || lexical.starts_with(read))
                && !self.file_roots.iter().any(|read| {
                    absolute_components(read).is_ok_and(|read| lexical.starts_with(read))
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "each file write root must equal or lie inside an explicit read file root; read roots never become write roots implicitly",
                ));
            }
            let web = self.web_dir.canonicalize().ok();
            if private
                .iter()
                .chain(web.iter())
                .any(|path| overlaps(&canonical, path) || overlaps(&lexical, path))
                || [
                    self.delivery_dir.as_ref(),
                    self.lifecycle_dir.as_ref(),
                    self.launcher_config.as_ref(),
                    self.audit_dir.as_ref(),
                ]
                .into_iter()
                .flatten()
                .any(|path| {
                    absolute_components(path).is_ok_and(|path| overlaps(&lexical, &path))
                        || path
                            .canonicalize()
                            .is_ok_and(|path| overlaps(&canonical, &path))
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "file write roots must be disjoint from frontend, native, state, host, delivery, lifecycle, launcher and audit paths",
                ));
            }
            if resolved.iter().any(|other| overlaps(&canonical, other)) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "file write roots must not overlap or repeat",
                ));
            }
            resolved.push(canonical);
        }
        Ok(())
    }

    /// The scan is explicit: a process root or Grok active file without the
    /// switch is a configuration mistake, not something to ignore. The active
    /// file is a native CLI state file read as-is; it must stay outside every
    /// private, frontend and native directory (like the Codex name index).
    fn validate_proc_scan(&self) -> io::Result<()> {
        if !self.proc_scan {
            if self.proc_root != Path::new("/proc") || self.grok_active.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SESSIONDOCK_PROC_ROOT and SESSIONDOCK_GROK_ACTIVE require SESSIONDOCK_PROC_SCAN=1",
                ));
            }
            return Ok(());
        }
        // Other platforms have no process table to check; the scanner itself
        // answers `unsupported_platform` there.
        if cfg!(target_os = "linux") && (!self.proc_root.is_absolute() || !self.proc_root.is_dir())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_PROC_ROOT must be an existing absolute directory",
            ));
        }
        let Some(active) = &self.grok_active else {
            return Ok(());
        };
        if !active.is_absolute() || !active.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_GROK_ACTIVE requires an explicit existing absolute file",
            ));
        }
        let resolved = active.canonicalize().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the explicit Grok active-sessions file cannot be resolved",
            )
        })?;
        for boundary in [
            Some(&self.web_dir),
            self.roots.claude.as_ref(),
            self.roots.codex.as_ref(),
            self.roots.grok.as_ref(),
            self.ptyhost_dir.as_ref(),
            self.state_dir.as_ref(),
            self.delivery_dir.as_ref(),
            self.lifecycle_dir.as_ref(),
            self.launcher_config.as_ref(),
            self.audit_dir.as_ref(),
            self.trash_dir.as_ref(),
        ]
        .into_iter()
        .flatten()
        .chain(self.file_roots.iter())
        {
            if overlaps(&resolved, &absolute_components(boundary)?)
                || boundary
                    .canonicalize()
                    .is_ok_and(|path| overlaps(&resolved, &path))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Grok active-sessions file must stay outside frontend, native, host, state, delivery, lifecycle, launcher, audit, trash and file access paths",
                ));
            }
        }
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
        if bind.ip().is_unspecified() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_NODE_BIND must name one interface address, not a wildcard",
            ));
        }
        // Port 0 is a fresh socket each time (tests); a fixed address is one listener.
        if bind == self.bind && bind.port() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_NODE_BIND must differ from SESSIONDOCK_BIND",
            ));
        }
        if !token.is_absolute() || !token.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_NODE_TOKEN_FILE requires an explicit existing absolute file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(token)?.permissions().mode() & 0o077 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "SESSIONDOCK_NODE_TOKEN_FILE must not be readable by group or others",
                ));
            }
        }
        crate::hub::NodeToken::load(token).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("SESSIONDOCK_NODE_TOKEN_FILE: {error}"),
            )
        })?;
        if !id.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_NODE_ID_FILE requires an explicit absolute path",
            ));
        }
        let token_path = resolve_file(token)?;
        let id_path = resolve_file(id)?;
        if token_path == id_path {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_NODE_TOKEN_FILE and SESSIONDOCK_NODE_ID_FILE must be different files",
            ));
        }
        match std::fs::symlink_metadata(id) {
            Ok(metadata) if metadata.is_file() => {
                if !crate::hub::identity::is_node_id(std::fs::read_to_string(id)?.trim()) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "SESSIONDOCK_NODE_ID_FILE holds an invalid node identity",
                    ));
                }
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SESSIONDOCK_NODE_ID_FILE must be a regular file, not a link or directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !id.parent().is_some_and(Path::is_dir) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SESSIONDOCK_NODE_ID_FILE parent directory must exist (the id is minted there on first start)",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
        // Neither credential file may live inside a served, native or other
        // service-owned tree: the id is written by this process on first start.
        for resolved in [token_path, id_path] {
            for boundary in [
                Some(&self.web_dir),
                self.roots.claude.as_ref(),
                self.roots.codex.as_ref(),
                self.roots.grok.as_ref(),
                self.ptyhost_dir.as_ref(),
                self.state_dir.as_ref(),
                self.delivery_dir.as_ref(),
                self.lifecycle_dir.as_ref(),
                self.launcher_config.as_ref(),
                self.codex_index.as_ref(),
                self.audit_dir.as_ref(),
                self.trash_dir.as_ref(),
                self.grok_active.as_ref(),
            ]
            .into_iter()
            .flatten()
            .chain(self.file_roots.iter())
            {
                if overlaps(&resolved, &absolute_components(boundary)?)
                    || boundary
                        .canonicalize()
                        .is_ok_and(|path| overlaps(&resolved, &path))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "node token and id files must stay outside frontend, native, host, state, delivery, lifecycle, launcher, audit, trash, Codex index, Grok active and file access paths",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Bug-report configuration is all-or-nothing: the bundle directory is a
    /// private 0700 directory like audit/trash, disjoint from every other
    /// path; the repository is an existing directory equal to or inside a
    /// file write root (attachments are written there) and therefore already
    /// outside every private and native path.
    fn validate_bug_report(&self) -> io::Result<()> {
        let (dir, repo) = match (&self.bug_report_dir, &self.bug_report_repo) {
            (None, None) => return Ok(()),
            (Some(dir), Some(repo)) => (dir, repo),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SESSIONDOCK_BUG_REPORT_DIR and SESSIONDOCK_BUG_REPORT_REPO must be set together (bug reports fail closed)",
                ));
            }
        };
        let resolved = crate::bug_report::validate_directory(dir)?;
        for boundary in [
            Some(&self.web_dir),
            self.roots.claude.as_ref(),
            self.roots.codex.as_ref(),
            self.roots.grok.as_ref(),
            self.ptyhost_dir.as_ref(),
            self.state_dir.as_ref(),
            self.delivery_dir.as_ref(),
            self.lifecycle_dir.as_ref(),
            self.codex_index.as_ref(),
            self.launcher_config.as_ref(),
            self.audit_dir.as_ref(),
            self.trash_dir.as_ref(),
            self.grok_active.as_ref(),
            self.node_token_file.as_ref(),
            self.node_id_file.as_ref(),
            Some(repo),
        ]
        .into_iter()
        .flatten()
        .chain(self.file_roots.iter())
        {
            if overlaps(&resolved, &absolute_components(boundary)?)
                || boundary
                    .canonicalize()
                    .is_ok_and(|path| overlaps(&resolved, &path))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "bug-report directory requires an independent directory outside frontend, native, host, state, delivery, lifecycle, launcher, audit, trash, Codex index, node credential, repository and file access paths",
                ));
            }
        }
        if !repo.is_absolute() || repo.components().any(|part| part == Component::ParentDir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_BUG_REPORT_REPO requires an explicit absolute directory without relative jumps",
            ));
        }
        let repo = repo.canonicalize().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_BUG_REPORT_REPO must be an existing directory",
            )
        })?;
        if !repo.is_dir() || repo.parent().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SESSIONDOCK_BUG_REPORT_REPO must be an existing directory below the filesystem root",
            ));
        }
        if !self
            .file_write_roots
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .any(|root| repo.starts_with(root))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "SESSIONDOCK_BUG_REPORT_REPO must equal or lie inside a file write root (report attachments are written through the file write service)",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_launcher(
        &self,
        launcher: &crate::lifecycle::launcher::Config,
    ) -> io::Result<()> {
        let invalid = || {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "launcher host/cwd configuration must match the explicit host directory and stay outside private/native/frontend roots",
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
        for root in &launcher.cwd_roots {
            let root = root.canonicalize().map_err(|_| invalid())?;
            for boundary in [
                Some(&self.web_dir),
                self.roots.claude.as_ref(),
                self.roots.codex.as_ref(),
                self.roots.grok.as_ref(),
                self.ptyhost_dir.as_ref(),
                self.state_dir.as_ref(),
                self.delivery_dir.as_ref(),
                self.lifecycle_dir.as_ref(),
                self.launcher_config.as_ref(),
                self.codex_index.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                if overlaps(&root, &absolute_components(boundary)?)
                    || boundary
                        .canonicalize()
                        .is_ok_and(|other| overlaps(&root, &other))
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn absolute_components(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a configured delivery boundary cannot be resolved",
        )
    })?;
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            component => result.push(component.as_os_str()),
        }
    }
    Ok(result)
}

/// Resolved location of a file that may not exist yet: the existing parent is
/// canonicalized and the file name appended, so overlap checks see aliases.
fn resolve_file(path: &Path) -> io::Result<PathBuf> {
    if let Ok(resolved) = path.canonicalize() {
        return Ok(resolved);
    }
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a configured credential file path has no file name",
        )
    })?;
    let parent = path.parent().unwrap_or(Path::new("/"));
    Ok(parent
        .canonicalize()
        .unwrap_or(absolute_components(parent)?)
        .join(name))
}

/// Advisory configuration checks only. The store must independently verify
/// capabilities, ownership, private files, path identities and writer leases
/// when opening an explicitly initialized ledger.
fn validate_delivery_directory(path: &Path) -> io::Result<PathBuf> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "SESSIONDOCK_DELIVERY_DIR requires an explicit existing absolute directory without relative jumps",
        )
    };
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(invalid());
    }
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor).map_err(|_| invalid())?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if metadata.file_type().is_symlink() || reparse {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "delivery directory and its ancestors must not be symlinks or reparse points",
            ));
        }
        if !metadata.is_dir() {
            return Err(invalid());
        }
        #[cfg(unix)]
        if ancestor == path {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o777 != 0o700 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "delivery directory requires private owner-only permissions (0700)",
                ));
            }
        }
    }
    path.canonicalize().map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn lifecycle_and_launcher_configuration_are_private_independent_boundaries() {
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
            assert!(config.validate().is_err(), "{name}");
        }
        config.lifecycle_dir = Some(root.path().join("receipts"));
        for name in [
            "web", "native", "host", "state", "delivery", "receipts", "files",
        ] {
            let file = root.path().join(name).join("launcher.json");
            std::fs::write(&file, b"{}").unwrap();
            config.launcher_config = Some(file);
            assert!(config.validate().is_err(), "{name}");
        }
        config.launcher_config = Some(private);
        config.lifecycle_dir = Some("relative".into());
        assert!(config.validate().is_err());
    }
    use super::*;

    fn private_directory(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        #[cfg(not(unix))]
        std::fs::create_dir(path).unwrap();
    }

    #[test]
    fn delivery_requires_existing_absolute_directory_without_creating_anything() {
        let (temp, mut config) = names_config();
        assert!(config.delivery_dir.is_none());
        for invalid in [
            PathBuf::new(),
            PathBuf::from("delivery"),
            temp.path().join("missing-delivery"),
            temp.path().join("names/session_index.jsonl"),
            temp.path().join("names/../state"),
        ] {
            config.delivery_dir = Some(invalid);
            assert_eq!(
                config.validate().unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert!(!temp.path().join("missing-delivery").exists());
    }

    #[test]
    fn delivery_is_disjoint_in_both_directions_from_every_boundary() {
        let (temp, mut config) = names_config();
        for name in [
            "web", "claude", "codex", "grok", "host", "state", "files", "names",
        ] {
            let path = temp.path().join(name);
            // Ensure rejection measures isolation rather than permissions.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            config.delivery_dir = Some(path.clone());
            assert_eq!(
                config.validate().unwrap_err().kind(),
                io::ErrorKind::PermissionDenied,
                "same {name}"
            );
            if name != "names" {
                let child = path.join("delivery-child");
                private_directory(&child);
                config.delivery_dir = Some(child);
                assert_eq!(
                    config.validate().unwrap_err().kind(),
                    io::ErrorKind::PermissionDenied,
                    "inside {name}"
                );
            }
        }
        let delivery = temp.path().join("delivery");
        private_directory(&delivery);
        let child = delivery.join("nested");
        private_directory(&child);
        let index = child.join("session_index.jsonl");
        std::fs::write(&index, b"").unwrap();
        for name in [
            "web", "claude", "codex", "grok", "host", "state", "files", "index",
        ] {
            let (_other_temp, mut isolated) = names_config();
            isolated.delivery_dir = Some(delivery.clone());
            match name {
                "web" => isolated.web_dir = child.clone(),
                "claude" => isolated.roots.claude = Some(child.clone()),
                "codex" => isolated.roots.codex = Some(child.clone()),
                "grok" => isolated.roots.grok = Some(child.clone()),
                "host" => isolated.ptyhost_dir = Some(child.clone()),
                "state" => isolated.state_dir = Some(child.clone()),
                "files" => isolated.file_roots = vec![child.clone()],
                "index" => isolated.codex_index = Some(index.clone()),
                _ => unreachable!(),
            }
            assert_eq!(
                isolated.validate().unwrap_err().kind(),
                io::ErrorKind::PermissionDenied,
                "contains {name}"
            );
        }
        // A configured child need not already exist to be recognized as an
        // overlap, including when its spelling contains relative components.
        config.delivery_dir = Some(delivery.clone());
        config.web_dir = delivery.join("nested/../not-created-web");
        assert_eq!(
            config.validate().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn delivery_external_directory_is_retained_without_initializing_ledger() {
        let (temp, mut config) = names_config();
        let delivery = temp.path().join("delivery");
        private_directory(&delivery);
        config.delivery_dir = Some(delivery.clone());
        config.validate().unwrap();
        assert_eq!(config.delivery_dir.as_ref(), Some(&delivery));
        assert_eq!(std::fs::read_dir(&delivery).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn delivery_rejects_nonprivate_directories_and_aliases_cannot_bypass_isolation() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (temp, mut config) = names_config();
        let delivery = temp.path().join("delivery");
        private_directory(&delivery);
        config.delivery_dir = Some(delivery.clone());
        std::fs::set_permissions(&delivery, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            config.validate().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::set_permissions(&delivery, std::fs::Permissions::from_mode(0o700)).unwrap();
        let alias = temp.path().join("delivery-alias");
        symlink(&delivery, &alias).unwrap();
        config.delivery_dir = Some(alias.clone());
        assert_eq!(
            config.validate().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(config.delivery_dir.as_ref(), Some(&alias));
        let nested = delivery.join("nested");
        private_directory(&nested);
        config.delivery_dir = Some(alias.join("nested"));
        assert_eq!(
            config.validate().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        config.delivery_dir = Some(delivery.clone());
        config.file_roots = vec![alias];
        assert_eq!(
            config.validate().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read_dir(delivery).unwrap().count(), 1); // Only our nested directory.
    }

    #[test]
    fn delivery_from_env_matches_direct_validation() {
        const CHILD: &str = "AGENTHUB_TEST_DELIVERY_CONFIG_CHILD";
        if let Some(expected) = env::var_os(CHILD) {
            let configured = env::var_os("SESSIONDOCK_DELIVERY_DIR").map(PathBuf::from);
            match Config::from_env() {
                Ok(config) => {
                    assert_eq!(expected, "valid");
                    assert_eq!(config.delivery_dir, configured);
                    assert_eq!(
                        std::fs::read_dir(config.delivery_dir.unwrap())
                            .unwrap()
                            .count(),
                        0
                    );
                }
                Err(_) => assert_eq!(expected, "invalid"),
            }
            return;
        }
        // Isolate environment parsing in child test processes; do not mutate
        // the parallel test runner's environment or invoke any agent CLI.
        let (temp, config) = names_config();
        let delivery = temp.path().join("delivery");
        private_directory(&delivery);
        for (path, expected) in [
            (delivery.clone(), "valid"),
            (PathBuf::new(), "invalid"),
            (PathBuf::from("relative-delivery"), "invalid"),
            (temp.path().join("missing"), "invalid"),
            (temp.path().join("names/session_index.jsonl"), "invalid"),
            (config.web_dir.clone(), "invalid"),
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
                    "config::tests::delivery_from_env_matches_direct_validation",
                    "--nocapture",
                ])
                .env(CHILD, expected)
                .env("SESSIONDOCK_WEB_DIR", &config.web_dir)
                .env("SESSIONDOCK_DELIVERY_DIR", path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(std::fs::read_dir(delivery).unwrap().count(), 0);
    }

    #[test]
    fn pool_budgets_default_by_parallelism_and_parse_bounded_env() {
        const CHILD: &str = "AGENTHUB_TEST_POOLS_CONFIG_CHILD";
        if let Some(expected) = env::var_os(CHILD) {
            match Config::from_env() {
                Ok(config) => {
                    assert_eq!(expected, "valid");
                    assert_eq!(config.pools.read_workers, 3);
                    assert_eq!(config.pools.wait, std::time::Duration::from_millis(0));
                    assert_eq!(config.pools.history_page_events, 7);
                }
                Err(error) => assert_eq!(expected, "invalid", "{error}"),
            }
            return;
        }
        let defaults = Pools::default();
        let cores = std::thread::available_parallelism().unwrap().get();
        assert_eq!(defaults.read_workers, (cores / 2).clamp(8, 32));
        assert_eq!(defaults.wait, std::time::Duration::from_secs(10));
        assert_eq!(defaults.history_page_events, 2000);
        // Fixed 4 : 2 : 8 became W : W/2 : 2W, each clamped.
        for (workers, probes, responses) in [
            (1, 2, 8),
            (8, 4, 16),
            (16, 8, 32),
            (32, 16, 64),
            (64, 16, 64),
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
        for (workers, wait, events, expected) in [
            ("3", "0", "7", "valid"),
            ("0", "0", "7", "invalid"),
            ("65", "0", "7", "invalid"),
            ("3", "60001", "7", "invalid"),
            ("3", "0", "0", "invalid"),
            ("3", "0", "10001", "invalid"),
            ("three", "0", "7", "invalid"),
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
                    "config::tests::pool_budgets_default_by_parallelism_and_parse_bounded_env",
                    "--nocapture",
                ])
                .env(CHILD, expected)
                .env("SESSIONDOCK_WEB_DIR", &config.web_dir)
                .env("SESSIONDOCK_READ_WORKERS", workers)
                .env("SESSIONDOCK_ADMISSION_WAIT_MS", wait)
                .env("SESSIONDOCK_HISTORY_PAGE_EVENTS", events)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{workers}/{wait}/{events}: {}",
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
    fn codex_index_requires_existing_absolute_file_and_native_codex_root() {
        let (temp, mut config) = names_config();
        for invalid in [
            PathBuf::new(),
            PathBuf::from("session_index.jsonl"),
            temp.path().join("missing.jsonl"),
            temp.path().join("names"),
        ] {
            config.codex_index = Some(invalid);
            assert_eq!(
                config.validate().unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        config.codex_index = Some(temp.path().join("names/session_index.jsonl"));
        config.roots.codex = None;
        assert_eq!(
            config.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn codex_index_is_disjoint_from_every_configured_directory() {
        let (temp, mut config) = names_config();
        assert!(config.validate().is_ok());
        for name in ["web", "claude", "codex", "grok", "host", "state", "files"] {
            let directory = temp.path().join(name).join("nested");
            std::fs::create_dir(&directory).unwrap();
            let index = directory.join("session_index.jsonl");
            std::fs::write(&index, b"").unwrap();
            config.codex_index = Some(index);
            assert_eq!(
                config.validate().unwrap_err().kind(),
                io::ErrorKind::PermissionDenied,
                "index inside {name} was allowed"
            );
        }
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
    fn codex_index_aliases_cannot_hide_overlap_or_bypass_names_nofollow() {
        use std::os::unix::fs::symlink;
        let (temp, mut config) = names_config();
        let native_index = temp.path().join("codex/session_index.jsonl");
        std::fs::write(&native_index, b"").unwrap();
        let native_alias = temp.path().join("native-index-alias");
        symlink(native_index, &native_alias).unwrap();
        config.codex_index = Some(native_alias.clone());
        assert_eq!(
            config.validate().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
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
            let error = store.list(false).unwrap_err();
            assert_eq!(error.status, 503);
            assert!(error.message.contains("Codex 名称索引"));
        }
    }

    #[test]
    fn no_implicit_data_roots_and_no_public_listener() {
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
        assert!(config.validate().is_err());
        config.bind = "[::1]:8741".parse().unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn runtime_and_native_directories_cannot_be_served_as_assets() {
        let temp = tempfile::tempdir().unwrap();
        let web = temp.path().join("web");
        let private = web.join("private");
        std::fs::create_dir_all(&private).unwrap();
        let mut config = Config {
            web_dir: web,
            ptyhost_dir: Some(private.clone()),
            ..Config::default()
        };
        assert!(config.validate().is_err());
        config.ptyhost_dir = None;
        config.roots.codex = Some(private);
        assert!(config.validate().is_err());
        config.roots.codex = None;
        config.web_dir = temp.path().join("missing-web");
        config.state_dir = Some(temp.path().to_path_buf());
        config.ptyhost_dir = Some(temp.path().join("web"));
        assert!(config.validate().is_err());
    }

    #[test]
    fn file_authority_is_separate_from_private_inputs_and_static_assets() {
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
            assert!(config.validate().is_err(), "{name}");
        }
        config.file_roots = vec![temp.path().to_owned()];
        assert!(config.validate().is_err());
    }

    #[cfg(unix)]
    fn node_files(temp: &Path) -> (PathBuf, PathBuf) {
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
    fn node_listener_rejects_wildcards_bad_credentials_and_service_owned_paths() {
        use std::os::unix::fs::PermissionsExt;
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
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("wildcard")
        );
        let mut config = full();
        config.node_bind = Some(config.bind);
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("differ")
        );
        // Token grammar and privacy are startup errors, not silent downgrades.
        let short = temp.path().join("short");
        std::fs::write(&short, "short\n").unwrap();
        std::fs::set_permissions(&short, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut config = full();
        config.node_token_file = Some(short);
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("32–256")
        );
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            full()
                .validate()
                .unwrap_err()
                .to_string()
                .contains("group or others")
        );
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        // An existing id file must already hold a node id; a missing one needs its parent.
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
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("parent directory")
        );
        let mut config = full();
        config.node_id_file = Some(token.clone());
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("different files")
        );
        // Credential files never live inside a served or native tree.
        let native = temp.path().join("claude");
        std::fs::create_dir(&native).unwrap();
        let mut config = full();
        config.roots.claude = Some(native.clone());
        config.node_id_file = Some(native.join("node-id"));
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
        let mut config = full();
        config.web_dir = temp.path().to_path_buf();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
    }
}
