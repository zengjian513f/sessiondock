//! The hub's node registry (`hub.py` `Registry`): `hub-nodes.json`, node
//! health with offline strikes, the per-node offline cache with its on-disk
//! session snapshot, conditional `sig` probes and the streaming search reader.
//!
//! The hub owns node state. `Monitor` checks every enabled node each
//! `PROBE_INTERVAL` (and at once when nudged by a user action); page requests
//! only consult that state and never wait on a node known to be down. Machines
//! being switched off is normal, so the last session list of each node is
//! persisted under the cache directory and shown as an offline cache until the
//! node is back. Registration stays a server-side operation: it validates the
//! URL against the allowed networks, asks the node's `/api/meta` and uses the
//! returned `node_id` as the primary key.
//!
//! Locking: one `std::sync::Mutex` over nodes/cache/health, never held across
//! network I/O (a cold search must not serialize heartbeats and lists).

#[cfg(all(test, unix))]
mod tests;

use std::{
    collections::HashMap,
    fmt, fs,
    io::{self, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::{FutureExt, StreamExt};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use super::Target;
use super::{
    client::{Client, ClientError, JSON_LIMIT, REQUEST_TIMEOUT, Request, request_failure},
    identity::{PROTOCOL, is_node_id, is_token},
};

pub const PROBE_INTERVAL: Duration = Duration::from_secs(10);
/// A node that is currently online is only painted offline after this many
/// consecutive failed checks: one slow answer from a busy machine, or a
/// service restart during deployment, must not gray out its console.
pub const OFFLINE_STRIKES: u32 = 2;
/// 机器配色在注册表里按机器配置：加机器只改配置，不用改代码。没配的机器没有颜色。
pub const NODE_PALETTE: [&str; 8] = [
    "blue", "violet", "amber", "teal", "rose", "lime", "cyan", "fuchsia",
];
/// Paths whose last good answer is served while the node is offline.
pub const CACHED_PATHS: [&str; 2] = ["/api/sessions", "/api/term/list"];
/// Default node networks: loopback plus the deployment WireGuard /24.
pub const DEFAULT_NETWORKS: &str = "127.0.0.0/8,::1/128,10.0.0.0/24";
const NAME_LIMIT: usize = 80;
const CACHE_LIMIT: usize = 128;
const CHECK_CONCURRENCY: usize = 16;

/// One `hub-nodes.json` entry. `Debug` hides the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub url: String,
    pub token: String,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 注册表里没写 enabled 的机器都算启用；只有设置页明确停用过才是 false。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// 这台机器的控制台渲染：`None`/`grid` = 服务端网格（默认），`xterm` = 浏览器解析。
    /// 是展示属性，和名称、配色一样存在中央，所有浏览器一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<String>,
}

/// The console renderers a machine can be set to.
pub const RENDERERS: [&str; 2] = ["grid", "xterm"];

impl Node {
    pub fn enabled(&self) -> bool {
        self.enabled != Some(false)
    }

    pub fn renderer(&self) -> &str {
        match self.renderer.as_deref() {
            Some("xterm") => "xterm",
            _ => "grid",
        }
    }

    pub fn color(&self) -> &str {
        self.color.as_deref().unwrap_or("")
    }
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("url", &self.url)
            .field("color", &self.color)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

/// An allowed node network in CIDR form, strict like `ipaddress.ip_network`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    addr: IpAddr,
    prefix: u8,
}

impl Network {
    pub fn contains(&self, addr: IpAddr) -> bool {
        match (self.addr, addr) {
            (IpAddr::V4(net), IpAddr::V4(addr)) => {
                let mask = mask(self.prefix, 32) as u32;
                u32::from(net) & mask == u32::from(addr) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(addr)) => {
                let mask = mask(self.prefix, 128);
                u128::from(net) & mask == u128::from(addr) & mask
            }
            _ => false,
        }
    }
}

/// The top `prefix` bits of a `bits`-wide address, as a `u128`.
fn mask(prefix: u8, bits: u8) -> u128 {
    if prefix == 0 {
        return 0;
    }
    let low = (!0u128) >> (128 - u32::from(bits));
    ((!0u128) << (u32::from(bits) - u32::from(prefix))) & low
}

impl FromStr for Network {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        let (addr, prefix) = match value.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (value, None),
        };
        let addr: IpAddr = addr
            .parse()
            .map_err(|_| format!("{value:?} does not appear to be an IPv4 or IPv6 network"))?;
        let bits = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix
                .parse::<u8>()
                .ok()
                .filter(|prefix| *prefix <= bits)
                .ok_or_else(|| format!("{value:?} is not a valid prefix length"))?,
            None => bits,
        };
        if masked(addr, prefix) != addr {
            return Err(format!("{value:?} has host bits set"));
        }
        Ok(Network { addr, prefix })
    }
}

fn masked(addr: IpAddr, prefix: u8) -> IpAddr {
    match addr {
        IpAddr::V4(addr) => IpAddr::V4((u32::from(addr) & mask(prefix, 32) as u32).into()),
        IpAddr::V6(addr) => IpAddr::V6((u128::from(addr) & mask(prefix, 128)).into()),
    }
}

/// Comma-separated CIDR list (`--node-networks` / `SESSIONDOCK_HUB_NETWORKS`).
pub fn parse_networks(value: &str) -> Result<Vec<Network>, String> {
    value.split(',').map(str::parse).collect()
}

#[derive(Debug)]
pub enum RegistryError {
    /// Rejected input; the text is user-facing.
    Invalid(String),
    /// No node with this id.
    NotFound(String),
    /// The node did not answer `/api/meta` (registration only).
    Node(ClientError),
    /// The registry or snapshot file could not be written.
    Io(io::Error),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::NotFound(id) => write!(f, "机器未注册或已移除：{id}"),
            Self::Node(error) => write!(f, "{}", error.message(REQUEST_TIMEOUT)),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<io::Error> for RegistryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn invalid(message: impl Into<String>) -> RegistryError {
    RegistryError::Invalid(message.into())
}

fn is_https(value: &str) -> bool {
    value
        .split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
}

/// `Registry.register` input; `id`, when given, must equal the node's own id.
#[derive(Clone, Debug, Default)]
pub struct Registration {
    pub name: String,
    pub url: String,
    pub token: String,
    pub color: String,
    pub id: Option<String>,
}

/// What `register` / `update_display` report back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NodeRow {
    pub id: String,
    pub name: String,
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    pub renderer: String,
}

/// Node health as the monitor last saw it. Serialized keys are merged into
/// the public node rows.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Health {
    /// `None` until the monitor has checked the node.
    pub online: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strikes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_since: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_since: Option<f64>,
}

impl Health {
    fn to_map(&self) -> Map<String, Value> {
        match serde_json::to_value(self) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        }
    }
}

/// Query string pairs as sent upstream (`urlencode(query, doseq=True)`).
pub type Query = [(String, String)];

/// `quote_plus` on keys and values.
pub fn encode_query(query: &Query) -> String {
    let mut out = String::new();
    for (key, value) in query {
        if !out.is_empty() {
            out.push('&');
        }
        quote_plus(key, &mut out);
        out.push('=');
        quote_plus(value, &mut out);
    }
    out
}

fn quote_plus(value: &str, out: &mut String) {
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
}

/// `federation.public_payload` hook: the aggregation layer rewrites wire
/// references per node; the registry caches the rewritten payload.
pub type PublicPayload = fn(Value, &Node, &str) -> Value;

fn identity_payload(data: Value, _node: &Node, _path: &str) -> Value {
    data
}

/// Streamed search events (`progress` / `matches` callbacks).
#[derive(Clone, Debug, PartialEq)]
pub enum SearchEvent {
    Progress { done: u64, total: u64 },
    Matches(Vec<Value>),
}

/// One node's answer: the (public) payload plus the failure record the
/// aggregate lists under `errors`, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Fetched {
    pub data: Value,
    pub failure: Option<Value>,
}

impl Fetched {
    pub fn ok(&self) -> bool {
        self.failure.is_none()
    }
}

type CacheKey = (String, String, String);

struct Inner {
    nodes: Vec<Node>,
    /// Insertion-ordered so the variant bound evicts the oldest entry.
    cache: IndexMap<CacheKey, (f64, Value)>,
    health: HashMap<String, Health>,
    snapshot_sigs: HashMap<String, Value>,
}

pub struct Registry {
    path: PathBuf,
    snapshot_dir: PathBuf,
    networks: Vec<Network>,
    inner: Mutex<Inner>,
    wake: Notify,
    public_payload: PublicPayload,
}

impl Registry {
    /// Load `hub-nodes.json` (absent = empty), validate every URL and seed the
    /// offline cache of enabled nodes from their snapshots.
    pub fn open(path: &Path, networks: Vec<Network>, cache_dir: &Path) -> io::Result<Self> {
        let nodes: Vec<Node> = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {error}", path.display()),
                )
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let registry = Self {
            path: path.to_path_buf(),
            snapshot_dir: cache_dir.to_path_buf(),
            networks,
            inner: Mutex::new(Inner {
                nodes: Vec::new(),
                cache: IndexMap::new(),
                health: HashMap::new(),
                snapshot_sigs: HashMap::new(),
            }),
            wake: Notify::new(),
            public_payload: identity_payload,
        };
        for node in &nodes {
            if !is_node_id(&node.id) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: invalid node id", path.display()),
                ));
            }
            registry.validate_url(&node.url).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {error}", path.display()),
                )
            })?;
        }
        {
            let mut inner = registry.lock();
            for node in nodes.iter().filter(|node| node.enabled()) {
                registry.load_snapshot(&mut inner, node);
            }
            inner.nodes = nodes;
        }
        Ok(registry)
    }

    /// Install the wire-namespace rewrite applied to every node payload.
    pub fn with_public_payload(mut self, public_payload: PublicPayload) -> Self {
        self.public_payload = public_payload;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ---- node state -------------------------------------------------------

    pub fn snapshot_path(&self, nid: &str) -> PathBuf {
        self.snapshot_dir.join(format!("{nid}.sessions.json"))
    }

    /// Seed the offline cache from disk so a switched-off machine still lists
    /// its sessions after a hub restart. State stays unknown until the monitor
    /// has checked the node.
    fn load_snapshot(&self, inner: &mut Inner, node: &Node) {
        let Ok(bytes) = fs::read(self.snapshot_path(&node.id)) else {
            return;
        };
        let Ok(raw) = serde_json::from_slice::<Value>(&bytes) else {
            return;
        };
        let Some(stamp) = number(&raw["stamp"]) else {
            return;
        };
        let data = raw["data"].clone();
        if !data.is_object() || !data["sessions"].is_array() {
            return;
        }
        inner.cache.insert(sessions_key(&node.id), (stamp, data));
        inner.health.entry(node.id.clone()).or_insert(Health {
            online: None,
            last_seen: Some(stamp),
            ..Health::default()
        });
    }

    /// Decide under the lock whether the snapshot changed; the write itself
    /// happens outside it, off the runtime.
    fn snapshot_needed(&self, inner: &mut Inner, nid: &str, data: &Value) -> bool {
        let sig = data.get("sig").cloned().unwrap_or(Value::Null);
        if truthy(&sig)
            && inner.snapshot_sigs.get(nid) == Some(&sig)
            && self.snapshot_path(nid).exists()
        {
            return false;
        }
        inner.snapshot_sigs.insert(nid.to_string(), sig);
        true
    }

    async fn write_snapshot(&self, nid: &str, stamp: f64, data: &Value) {
        let dir = self.snapshot_dir.clone();
        let nid = nid.to_string();
        let data = data.clone();
        let _ = tokio::task::spawn_blocking(move || {
            // Best effort: a full disk must not fail the probe.
            let _ = write_snapshot_file(&dir, &nid, stamp, &data);
        })
        .await;
    }

    fn drop_snapshot(&self, nid: &str) {
        let _ = fs::remove_file(self.snapshot_path(nid));
    }

    /// Health of one node; `None` when the monitor never recorded it (or the
    /// node is disabled).
    pub fn state(&self, nid: &str) -> Option<Health> {
        self.lock().health.get(nid).cloned()
    }

    pub fn offline(&self, nid: &str) -> bool {
        self.state(nid)
            .is_some_and(|health| health.online == Some(false))
    }

    fn sessions_sig(&self, nid: &str) -> Option<String> {
        self.lock()
            .cache
            .get(&sessions_key(nid))
            .and_then(|(_, data)| data.get("sig"))
            .and_then(|sig| sig.as_str())
            .filter(|sig| !sig.is_empty())
            .map(str::to_string)
    }

    /// Conditional session fetch: the node answers a tiny `unchanged` when its
    /// list signature still matches, so heartbeats cost bytes, not megabytes.
    pub async fn check(&self, client: &Client, node: &Node) -> Fetched {
        let query: Vec<(String, String)> = match self.sessions_sig(&node.id) {
            Some(sig) => vec![("sig".to_string(), sig)],
            None => Vec::new(),
        };
        self.query(client, node, "/api/sessions", &query, client.timeout, None)
            .await
    }

    /// One monitor pass: refresh state and the session snapshot of every node.
    pub async fn check_all(&self, client: &Client) {
        let nodes = self.all();
        futures_util::stream::iter(nodes)
            .map(|node| async move {
                self.check(client, &node).await;
            })
            .buffer_unordered(CHECK_CONCURRENCY)
            .collect::<Vec<()>>()
            .await;
    }

    /// Ask the monitor to re-check now (a user just acted on an offline node).
    pub fn nudge(&self) {
        self.wake.notify_one();
    }

    /// Quick inline re-check before refusing an explicit action on an offline
    /// node, so a machine that just came back is usable at once.
    pub async fn recheck(&self, client: &Client, node: &Node) -> bool {
        self.query(client, node, "/api/live", &[], client.recheck, None)
            .await;
        self.nudge();
        !self.offline(&node.id)
    }

    /// `http(s)://<literal IP>[:port]` with no path, credentials, query or
    /// fragment, inside one of the allowed networks. Literal IPs eliminate DNS
    /// rebinding and make the allowlist auditable.
    pub fn validate_url(&self, value: &str) -> Result<SocketAddr, RegistryError> {
        const SHAPE: &str = "节点地址必须是 http(s)://私网IP:端口，不含路径或凭据";
        let Some((scheme, rest)) = value.split_once("://") else {
            return Err(invalid(SHAPE));
        };
        let tls = scheme.eq_ignore_ascii_case("https");
        if !scheme.eq_ignore_ascii_case("http") && !tls {
            return Err(invalid(SHAPE));
        }
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        if !(tail.is_empty() || tail == "/") || authority.contains('@') || authority.is_empty() {
            return Err(invalid(SHAPE));
        }
        let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
            let Some((host, port)) = rest.split_once(']') else {
                return Err(invalid(SHAPE));
            };
            match port.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None if port.is_empty() => (host, None),
                None => return Err(invalid(SHAPE)),
            }
        } else {
            // `urlparse` splits an unbracketed authority at the first colon.
            match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
        if host.is_empty() {
            return Err(invalid(SHAPE));
        }
        let address: IpAddr = host.parse().map_err(|_| invalid(SHAPE))?;
        if !self
            .networks
            .iter()
            .any(|network| network.contains(address))
        {
            return Err(invalid("节点地址不在 Hub 允许的网络内"));
        }
        let port = match port.filter(|port| !port.is_empty()) {
            Some(port) => port
                .parse::<u16>()
                .ok()
                .filter(|port| *port >= 1)
                .ok_or_else(|| invalid("invalid port"))?,
            None => {
                if tls {
                    443
                } else {
                    80
                }
            }
        };
        Ok(SocketAddr::new(address, port))
    }

    /// Where requests to this node go; the URL was validated at registration.
    pub fn target(&self, node: &Node) -> Result<Target, RegistryError> {
        Ok(Target {
            addr: self.validate_url(&node.url)?,
            token: node.token.clone(),
            tls: is_https(&node.url),
        })
    }

    /// 只含启用的机器：聚合、监控、代理和 uid 解析都从这里取，停用的机器对它们
    /// 不存在（双系统的两台机器有一台开着另一台必然关着，不必反复去探）。
    pub fn all(&self) -> Vec<Node> {
        self.lock()
            .nodes
            .iter()
            .filter(|node| node.enabled())
            .cloned()
            .collect()
    }

    pub fn get(&self, nid: &str) -> Option<Node> {
        self.all().into_iter().find(|node| node.id == nid)
    }

    /// 包括停用的机器；只有设置页改属性（含重新启用）用它。
    pub fn find(&self, nid: &str) -> Option<Node> {
        self.lock()
            .nodes
            .iter()
            .find(|node| node.id == nid)
            .cloned()
    }

    fn save(&self, inner: &Inner) -> io::Result<()> {
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension("tmp");
        let mut file = private_create(&temp)?;
        serde_json::to_writer_pretty(&mut file, &inner.nodes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &self.path)
    }

    /// Server-side registration: validate, ask the node who it is, replace any
    /// entry with that id, forget its cache/health/snapshot, nudge the monitor.
    pub async fn register(
        &self,
        client: &Client,
        body: Registration,
    ) -> Result<NodeRow, RegistryError> {
        let name = body.name.trim().to_string();
        let token = body.token.trim().to_string();
        let url = body.url.trim_end_matches('/').to_string();
        if name.is_empty() || name.chars().count() > NAME_LIMIT || !is_token(&token) {
            return Err(invalid("请输入机器名称、地址和至少 32 字符的节点凭据"));
        }
        let addr = self.validate_url(&url)?;
        let target = Target {
            addr,
            token: token.clone(),
            tls: is_https(&url),
        };
        let (status, meta) = client
            .json(&target, "GET", "/api/meta", None, client.timeout)
            .await
            .map_err(RegistryError::Node)?;
        let node_id = meta.get("node_id").and_then(Value::as_str).unwrap_or("");
        if status != 200
            || meta.get("mode").and_then(Value::as_str) != Some("local")
            || meta.get("protocol").and_then(Value::as_u64) != Some(u64::from(PROTOCOL))
            || !is_node_id(node_id)
        {
            return Err(invalid("节点认证或协议检查失败，请先升级节点并配置凭据"));
        }
        let color = body.color.trim().to_lowercase();
        if !color.is_empty() && !NODE_PALETTE.contains(&color.as_str()) {
            return Err(invalid(format!(
                "机器颜色只能取 {}",
                NODE_PALETTE.join("、")
            )));
        }
        let node = Node {
            url,
            token,
            id: node_id.to_string(),
            name: name.clone(),
            color: (!color.is_empty()).then_some(color.clone()),
            enabled: None,
            renderer: None,
        };
        {
            let mut inner = self.lock();
            if body
                .id
                .as_deref()
                .is_some_and(|expected| !expected.is_empty() && expected != node.id)
            {
                return Err(invalid("地址对应另一台机器，不能覆盖原节点身份"));
            }
            inner.nodes.retain(|existing| existing.id != node.id);
            inner.nodes.push(node.clone());
            inner.cache.retain(|key, _| key.0 != node.id);
            inner.health.remove(&node.id);
            self.drop_snapshot(&node.id);
            self.save(&inner)?;
        }
        self.nudge();
        Ok(NodeRow {
            id: node.id,
            name,
            color,
            enabled: None,
            renderer: "grid".to_string(),
        })
    }

    /// 改机器的名称、配色和启用状态。地址和凭据不在这里，它们只在服务器端注册时设定。
    pub fn update_display(
        &self,
        nid: &str,
        name: Option<&str>,
        color: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<NodeRow, RegistryError> {
        let mut wake = false;
        let row = {
            let mut inner = self.lock();
            let index = inner
                .nodes
                .iter()
                .position(|node| node.id == nid)
                .ok_or_else(|| RegistryError::NotFound(nid.to_string()))?;
            if let Some(name) = name {
                let clean = name.trim();
                if clean.is_empty() || clean.chars().count() > NAME_LIMIT {
                    return Err(invalid("机器名称不能为空，且不超过 80 个字符"));
                }
                if inner
                    .nodes
                    .iter()
                    .any(|node| node.id != nid && node.name == clean)
                {
                    return Err(invalid(format!("已有机器叫 {clean}")));
                }
                inner.nodes[index].name = clean.to_string();
            }
            if let Some(color) = color {
                let clean = color.trim().to_lowercase();
                if !clean.is_empty() && !NODE_PALETTE.contains(&clean.as_str()) {
                    return Err(invalid(format!(
                        "机器颜色只能取 {}",
                        NODE_PALETTE.join("、")
                    )));
                }
                inner.nodes[index].color = (!clean.is_empty()).then_some(clean);
            }
            if let Some(flag) = enabled
                && flag != inner.nodes[index].enabled()
            {
                if flag {
                    inner.nodes[index].enabled = None;
                    let node = inner.nodes[index].clone();
                    // 磁盘快照没删，重新启用后先把上次的列表拿回来
                    self.load_snapshot(&mut inner, &node);
                    wake = true;
                } else {
                    // 停用即视同不存在：内存里的健康状态和缓存一并清掉，监控不再探它
                    inner.nodes[index].enabled = Some(false);
                    inner.cache.retain(|key, _| key.0 != nid);
                    inner.health.remove(nid);
                }
            }
            self.save(&inner)?;
            let node = &inner.nodes[index];
            NodeRow {
                id: node.id.clone(),
                name: node.name.clone(),
                color: node.color().to_string(),
                enabled: Some(node.enabled()),
                renderer: node.renderer().to_string(),
            }
        };
        if wake {
            self.nudge();
        }
        Ok(row)
    }

    /// 设置页的“控制台渲染”：`grid`（或空 = 默认）/ `xterm`。
    pub fn set_renderer(&self, nid: &str, renderer: &str) -> Result<NodeRow, RegistryError> {
        let mut inner = self.lock();
        let index = inner
            .nodes
            .iter()
            .position(|node| node.id == nid)
            .ok_or_else(|| RegistryError::NotFound(nid.to_string()))?;
        let clean = renderer.trim().to_lowercase();
        if !clean.is_empty() && !RENDERERS.contains(&clean.as_str()) {
            return Err(invalid(format!(
                "控制台渲染只能取 {}",
                RENDERERS.join("、")
            )));
        }
        inner.nodes[index].renderer = (clean == "xterm").then_some(clean);
        self.save(&inner)?;
        let node = &inner.nodes[index];
        Ok(NodeRow {
            id: node.id.clone(),
            name: node.name.clone(),
            color: node.color().to_string(),
            enabled: Some(node.enabled()),
            renderer: node.renderer().to_string(),
        })
    }

    pub fn remove(&self, nid: &str) -> io::Result<()> {
        let mut inner = self.lock();
        inner.nodes.retain(|node| node.id != nid);
        inner.cache.retain(|key, _| key.0 != nid);
        inner.health.remove(nid);
        self.drop_snapshot(nid);
        self.save(&inner)
    }

    /// `Registry.request`: one JSON round trip with the node headers.
    pub async fn request(
        &self,
        client: &Client,
        node: &Node,
        path: &str,
        method: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<(u16, Value), ClientError> {
        let target = self
            .target(node)
            .map_err(|_| ClientError::Invalid("invalid node url"))?;
        client.json(&target, method, path, body, timeout).await
    }

    /// Keep reading while the node makes progress, including cold scans.
    /// Streams `progress`/`matches` to `events`; a JSON (non-NDJSON) answer is
    /// returned whole for old nodes.
    async fn search_request(
        &self,
        client: &Client,
        node: &Node,
        query: &Query,
        events: Option<&mpsc::Sender<SearchEvent>>,
        found: &mut IndexMap<String, Value>,
    ) -> Result<(u16, Value), ClientError> {
        let target = self
            .target(node)
            .map_err(|_| ClientError::Invalid("invalid node url"))?;
        let mut pairs: Vec<(String, String)> = query
            .iter()
            .filter(|(key, _)| key != "progress")
            .cloned()
            .collect();
        pairs.push(("progress".to_string(), "1".to_string()));
        let path = format!("/api/search?{}", encode_query(&pairs));
        let response = client
            .open(
                &target,
                Request {
                    method: "GET",
                    target: &path,
                    headers: &[],
                    body: None,
                    connect: client.timeout,
                    idle: client.search_idle,
                },
            )
            .await?;
        let status = response.status;
        let ndjson = response
            .header("Content-Type")
            .is_some_and(|value| value.contains("application/x-ndjson"));
        let mut body = response.into_body();
        if !ndjson {
            let raw = body.read_to_end(JSON_LIMIT).await?;
            let value = serde_json::from_slice(&raw)
                .map_err(|_| ClientError::Invalid("body is not JSON"))?;
            return Ok((status, value));
        }
        let mut remaining = JSON_LIMIT;
        loop {
            let Some(line) = body.read_line(remaining + 1).await? else {
                return Err(ClientError::Invalid("搜索响应不完整"));
            };
            remaining = remaining
                .checked_sub(line.len())
                .ok_or(ClientError::Invalid("节点响应过大"))?;
            let event: Value = serde_json::from_slice(&line)
                .map_err(|_| ClientError::Invalid("bad search event"))?;
            match event.get("type").and_then(Value::as_str) {
                Some("progress") => {
                    let done =
                        integer(&event["done"]).ok_or(ClientError::Invalid("bad progress"))?;
                    let total =
                        integer(&event["total"]).ok_or(ClientError::Invalid("bad progress"))?;
                    if let Some(events) = events
                        && events
                            .send(SearchEvent::Progress { done, total })
                            .await
                            .is_err()
                    {
                        return Err(ClientError::Invalid("search cancelled"));
                    }
                }
                Some("matches") => {
                    let results = event
                        .get("results")
                        .and_then(Value::as_array)
                        .ok_or(ClientError::Invalid("bad matches"))?;
                    let public =
                        (self.public_payload)(json!({"results": results}), node, "/api/search");
                    let rows = public
                        .get("results")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for row in &rows {
                        let uid = row
                            .get("uid")
                            .and_then(Value::as_str)
                            .ok_or(ClientError::Invalid("row without uid"))?;
                        found.insert(uid.to_string(), row.clone());
                    }
                    if let Some(events) = events
                        && events.send(SearchEvent::Matches(rows)).await.is_err()
                    {
                        return Err(ClientError::Invalid("search cancelled"));
                    }
                }
                Some("result") => {
                    let data = event
                        .get("data")
                        .cloned()
                        .ok_or(ClientError::Invalid("result without data"))?;
                    return Ok((status, data));
                }
                Some("error") => return Err(ClientError::Invalid("节点搜索失败")),
                _ => {}
            }
        }
    }

    /// Fetch `path` from a node, record its health, cache cacheable answers
    /// and fall back to the offline cache on failure. Never holds the lock
    /// across the network round trip.
    pub async fn query(
        &self,
        client: &Client,
        node: &Node,
        path: &str,
        query: &Query,
        timeout: Duration,
        events: Option<&mpsc::Sender<SearchEvent>>,
    ) -> Fetched {
        let encoded = encode_query(query);
        let key: CacheKey = (node.id.clone(), path.to_string(), encoded.clone());
        let stamp = now();
        let mut found: IndexMap<String, Value> = IndexMap::new();
        let search = path == "/api/search";
        let outcome = if search {
            self.search_request(client, node, query, events, &mut found)
                .await
        } else {
            let target = if encoded.is_empty() {
                format!("{path}?")
            } else {
                format!("{path}?{encoded}")
            };
            self.request(client, node, &target, "GET", None, timeout)
                .await
        };
        let (status, error) = match outcome {
            Ok((status, data)) if status == 200 => {
                if !data.is_object() {
                    (Some(status), ClientError::Invalid("body is not an object"))
                } else {
                    let unchanged = path == "/api/sessions" && truthy(&data["unchanged"]);
                    let data = if unchanged {
                        data
                    } else {
                        (self.public_payload)(data, node, path)
                    };
                    let snapshot = {
                        let mut inner = self.lock();
                        if !search && !unchanged {
                            inner.cache.insert(key.clone(), (stamp, data.clone()));
                            // Bound variant caches (debug views / forced refreshes).
                            while inner.cache.len() > CACHE_LIMIT {
                                inner.cache.shift_remove_index(0);
                            }
                        }
                        let snapshot = path == "/api/sessions"
                            && !unchanged
                            && query.iter().all(|(key, _)| key == "force" || key == "sig");
                        if snapshot {
                            inner
                                .cache
                                .insert(sessions_key(&node.id), (stamp, data.clone()));
                        }
                        inner.health.insert(
                            node.id.clone(),
                            Health {
                                online: Some(true),
                                last_seen: Some(stamp),
                                checked_at: Some(stamp),
                                ..Health::default()
                            },
                        );
                        snapshot && self.snapshot_needed(&mut inner, &node.id, &data)
                    };
                    if snapshot {
                        self.write_snapshot(&node.id, stamp, &data).await;
                    }
                    return Fetched {
                        data,
                        failure: None,
                    };
                }
            }
            Ok((status, _)) => (Some(status), ClientError::Invalid("non-200 status")),
            Err(error) => (None, error),
        };
        // Errors intentionally omit URL / token / upstream text.
        let waited = if search { client.search_idle } else { timeout };
        let (code, reason) = request_failure(Some(&error), status, waited);
        let (prior, cached) = {
            let mut inner = self.lock();
            let prior = inner.health.get(&node.id).cloned().unwrap_or_default();
            // A failed search says nothing about the node's live/terminal APIs.
            if !search {
                let checked = now();
                let strikes = prior.strikes.unwrap_or(0) + 1;
                let failed_since = prior.failed_since.unwrap_or(checked);
                // Hold a previously reachable node visible for one more
                // probe; its own requests still report their real error.
                let tentative = prior.online == Some(true) && strikes < OFFLINE_STRIKES;
                let mut state = Health {
                    online: Some(tentative),
                    error: Some(reason.clone()),
                    error_code: Some(code.to_string()),
                    failed_path: Some(path.to_string()),
                    checked_at: Some(checked),
                    strikes: Some(strikes),
                    failed_since: Some(failed_since),
                    ..prior.clone()
                };
                state.offline_since = if tentative {
                    None
                } else {
                    // Report the outage from its first failure, not from the
                    // probe that finally gave up on the node.
                    prior.offline_since.or(Some(failed_since))
                };
                inner.health.insert(node.id.clone(), state);
            }
            let cached = self.cached(&inner, &node.id, path, &key);
            (prior, cached)
        };
        let mut data = stale_payload(path, cached.as_ref());
        if search && !found.is_empty() {
            data["results"] = Value::Array(found.into_values().collect());
        }
        Fetched {
            data,
            failure: Some(json!({
                "node_id": node.id, "name": node.name,
                "error": reason, "error_code": code, "last_seen": prior.last_seen,
            })),
        }
    }

    fn cached(&self, inner: &Inner, nid: &str, path: &str, key: &CacheKey) -> Option<(f64, Value)> {
        let mut cached = if CACHED_PATHS.contains(&path) {
            inner.cache.get(key).cloned()
        } else {
            None
        };
        if cached.is_none() && path == "/api/sessions" {
            cached = inner.cache.get(&sessions_key(nid)).cloned();
        }
        cached
    }

    /// Aggregate-side query: a node the monitor knows to be offline is never
    /// waited on. Its last failure and offline cache are returned instead.
    pub async fn fetch(
        &self,
        client: &Client,
        node: &Node,
        path: &str,
        query: &Query,
        events: Option<&mpsc::Sender<SearchEvent>>,
    ) -> Fetched {
        let nid = &node.id;
        let key: CacheKey = (nid.clone(), path.to_string(), encode_query(query));
        let (health, cached) = {
            let inner = self.lock();
            (
                inner.health.get(nid).cloned().unwrap_or_default(),
                self.cached(&inner, nid, path, &key),
            )
        };
        // Never hold registry state while waiting for network I/O. A cold search
        // otherwise serializes every node, heartbeat, list and creation response.
        if health.online != Some(false) {
            if path == "/api/sessions"
                && query.is_empty()
                && let Some((_, data)) = &cached
                && truthy(&data["sig"])
            {
                let checked = self.check(client, node).await;
                if !checked.ok() || !truthy(&checked.data["unchanged"]) {
                    return checked;
                }
                let inner = self.lock();
                let latest = inner.cache.get(&sessions_key(nid)).cloned().or(cached);
                return Fetched {
                    data: latest.map(|(_, data)| data).unwrap_or_else(|| json!({})),
                    failure: None,
                };
            }
            return self
                .query(client, node, path, query, client.timeout, events)
                .await;
        }
        Fetched {
            data: stale_payload(path, cached.as_ref()),
            failure: Some(json!({
                "node_id": nid, "name": node.name,
                "error": health.error.clone().unwrap_or_else(|| "节点暂时离线".to_string()),
                "error_code": health.error_code.clone().unwrap_or_else(|| "connection_failed".to_string()),
                "last_seen": health.last_seen, "offline_since": health.offline_since,
            })),
        }
    }

    /// Enabled machines with their health, never url or token.
    pub fn public(&self) -> Vec<Value> {
        let inner = self.lock();
        inner
            .nodes
            .iter()
            .filter(|node| node.enabled())
            .map(|node| {
                let mut row = Map::new();
                row.insert("id".into(), node.id.clone().into());
                row.insert("name".into(), node.name.clone().into());
                row.insert("color".into(), node.color().into());
                row.insert("renderer".into(), node.renderer().into());
                merge_health(&mut row, inner.health.get(&node.id));
                Value::Object(row)
            })
            .collect()
    }

    /// 设置页用的完整名单：按注册表顺序，含停用的机器；顺序不随启用状态变，
    /// 只由用户在设置页拖动决定。
    pub fn machines(&self) -> Vec<Value> {
        let inner = self.lock();
        inner
            .nodes
            .iter()
            .map(|node| {
                let mut row = Map::new();
                row.insert("id".into(), node.id.clone().into());
                row.insert("name".into(), node.name.clone().into());
                row.insert("color".into(), node.color().into());
                row.insert("renderer".into(), node.renderer().into());
                row.insert("enabled".into(), node.enabled().into());
                merge_health(
                    &mut row,
                    if node.enabled() {
                        inner.health.get(&node.id)
                    } else {
                        None
                    },
                );
                Value::Object(row)
            })
            .collect()
    }

    /// 按设置页拖出来的顺序重排注册表；必须是全部机器（含停用的）的一个排列。
    pub fn reorder(&self, ids: &[String]) -> Result<Vec<String>, RegistryError> {
        let mut inner = self.lock();
        let known: Vec<String> = inner.nodes.iter().map(|node| node.id.clone()).collect();
        let mut sorted_ids = ids.to_vec();
        sorted_ids.sort();
        let mut sorted_known = known.clone();
        sorted_known.sort();
        let distinct = sorted_ids.windows(2).all(|pair| pair[0] != pair[1]);
        if sorted_ids != sorted_known || !distinct {
            return Err(invalid("顺序必须包含每台机器各一次"));
        }
        if ids != known.as_slice() {
            let mut by_id: HashMap<String, Node> = inner
                .nodes
                .drain(..)
                .map(|node| (node.id.clone(), node))
                .collect();
            inner.nodes = ids
                .iter()
                .map(|id| by_id.remove(id).expect("checked permutation"))
                .collect();
            self.save(&inner)?;
        }
        Ok(inner.nodes.iter().map(|node| node.id.clone()).collect())
    }

    /// `reorder` from a JSON body value with a shape check.
    pub fn reorder_json(&self, ids: &Value) -> Result<Vec<String>, RegistryError> {
        let ids: Vec<String> = ids
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .ok_or_else(|| invalid("ids 必须是机器 id 列表"))?;
        self.reorder(&ids)
    }
}

/// Deep copy of the last good answer with rows marked stale;
/// a terminal list additionally reports itself disabled.
pub fn stale_payload(path: &str, cached: Option<&(f64, Value)>) -> Value {
    let mut data = cached
        .map(|(_, data)| data.clone())
        .unwrap_or_else(|| json!({}));
    if let Some((stamp, _)) = cached {
        for key in ["sessions", "pending"] {
            if let Some(rows) = data.get_mut(key).and_then(Value::as_array_mut) {
                for row in rows.iter_mut().filter_map(Value::as_object_mut) {
                    row.insert("stale".into(), Value::Bool(true));
                    row.insert("last_seen".into(), json!(stamp));
                }
            }
        }
    }
    if path == "/api/term/list"
        && let Some(map) = data.as_object_mut()
    {
        map.insert("enabled".into(), Value::Bool(false));
        map.insert("sources".into(), json!({}));
    }
    data
}

fn merge_health(row: &mut Map<String, Value>, health: Option<&Health>) {
    match health {
        Some(health) => row.extend(health.to_map()),
        None => {
            row.insert("online".into(), Value::Null);
        }
    }
}

fn sessions_key(nid: &str) -> CacheKey {
    (nid.to_string(), "/api/sessions".to_string(), String::new())
}

/// Truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// `int(value)` for progress counters.
fn integer(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number
            .as_f64()
            .filter(|number| *number >= 0.0)
            .map(|number| number as u64),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// `hub-cache/<nid>.sessions.json` = `{stamp, data}`, 0600, temp + rename.
fn write_snapshot_file(dir: &Path, nid: &str, stamp: f64, data: &Value) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let temp = dir.join(format!("{nid}.sessions.tmp"));
    let mut file = private_create(&temp)?;
    serde_json::to_writer(&mut file, &json!({"stamp": stamp, "data": data}))?;
    file.flush()?;
    drop(file);
    fs::rename(&temp, dir.join(format!("{nid}.sessions.json")))
}

/// `os.open(O_WRONLY|O_CREAT|O_TRUNC, 0o600)`.
fn private_create(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// The monitor task: `check_all` every interval or as soon as nudged.
pub struct Monitor {
    task: tokio::task::JoinHandle<()>,
    shutdown: CancellationToken,
}

impl Monitor {
    pub fn spawn(
        registry: Arc<Registry>,
        client: Arc<Client>,
        shutdown: CancellationToken,
    ) -> Self {
        Self::spawn_every(registry, client, shutdown, PROBE_INTERVAL)
    }

    pub fn spawn_every(
        registry: Arc<Registry>,
        client: Arc<Client>,
        shutdown: CancellationToken,
        interval: Duration,
    ) -> Self {
        let shutdown = shutdown.child_token();
        // Like `start_monitor` clearing the event: a nudge from before the
        // monitor existed does not schedule a second immediate pass.
        let _ = registry.wake.notified().now_or_never();
        let stop = shutdown.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = registry.check_all(&client) => {}
                }
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = registry.wake.notified() => {}
                    _ = tokio::time::sleep(interval) => {}
                }
            }
        });
        Self { task, shutdown }
    }

    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = self.task.await;
    }
}
