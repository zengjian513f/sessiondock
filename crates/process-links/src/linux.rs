//! Linux adapter: only the current uid's processes, selected environment keys,
//! and established sockets. No commands, environment dumps, signals or writes.
use super::{Connection, Process};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::{Ipv4Addr, Ipv6Addr},
    path::Path,
};

pub const IDENTITIES: [(&str, &str); 5] = [
    ("CODEX_THREAD_ID", "codex"),
    ("CODEX_COMPANION_SESSION_ID", "codex"),
    ("CLAUDE_CODE_SESSION_ID", "claude"),
    ("GROK_SESSION_ID", "grok"),
    ("CODEX_SESSION_ID", "codex"),
];

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub process: Process,
    pub started_at: f64,
    pub parent: u32,
    pub connection: Option<Connection>,
    pub identities: Vec<(String, String)>,
    pub sockets: Vec<Connection>,
    pub multiplexed: bool,
    pub shared_parent: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub boot_id: String,
    pub entries: BTreeMap<u32, Entry>,
}

/// Identities the nearest owned ancestor (inclusive) inherited from its own
/// launcher. Below that owner they name the launcher, so they must not win
/// over the nearer owner; identities the owner's CLI set itself still count.
pub fn launcher_identities<'a>(
    snapshot: &'a Snapshot,
    pid: u32,
    owned: impl Fn(&Entry) -> bool,
) -> &'a [(String, String)] {
    let mut current = pid;
    let mut seen = BTreeSet::new();
    while current > 1 && seen.insert(current) {
        let Some(entry) = snapshot.entries.get(&current) else {
            break;
        };
        if entry.shared_parent {
            break;
        }
        if owned(entry) {
            return &entry.identities;
        }
        current = entry.parent;
    }
    &[]
}

pub fn identity(root: &Path, pid: u32) -> Option<(Process, u32)> {
    let stat = fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let fields: Vec<_> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    if fields.first().is_some_and(|s| *s == "Z") {
        return None;
    }
    Some((
        Process {
            pid,
            start: fields.get(19)?.parse().ok()?,
        },
        fields.get(1)?.parse().ok()?,
    ))
}

fn endpoint(raw: &str) -> Option<(String, u16)> {
    let (ip, port) = raw.split_once(':')?;
    let addr = match ip.len() {
        8 => Ipv4Addr::from(u32::from_str_radix(ip, 16).ok()?.to_ne_bytes()).to_string(),
        32 => {
            let mut bytes = [0u8; 16];
            for i in 0..4 {
                bytes[4 * i..4 * i + 4].copy_from_slice(
                    &u32::from_str_radix(&ip[8 * i..8 * i + 8], 16)
                        .ok()?
                        .to_ne_bytes(),
                );
            }
            let addr = Ipv6Addr::from(bytes);
            addr.to_ipv4_mapped()
                .map_or_else(|| addr.to_string(), |v| v.to_string())
        }
        _ => return None,
    };
    Some((addr, u16::from_str_radix(port, 16).ok()?))
}

fn socket_table(root: &Path) -> BTreeMap<u64, Connection> {
    let mut table = BTreeMap::new();
    for name in ["tcp", "tcp6"] {
        let Ok(raw) = fs::read_to_string(root.join("net").join(name)) else {
            continue;
        };
        for line in raw.lines().skip(1) {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 10 || fields[3] != "01" {
                continue;
            }
            let (Some((client_ip, client_port)), Some((server_ip, server_port)), Ok(inode)) =
                (endpoint(fields[1]), endpoint(fields[2]), fields[9].parse())
            else {
                continue;
            };
            table.insert(
                inode,
                Connection {
                    client_ip,
                    client_port,
                    server_ip,
                    server_port,
                },
            );
        }
    }
    table
}

fn unix_listeners(root: &Path) -> BTreeSet<u64> {
    fs::read_to_string(root.join("net/unix"))
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields.get(3) == Some(&"00010000"))
                .then(|| fields.get(6)?.parse().ok())
                .flatten()
        })
        .collect()
}

pub fn collect(root: &Path) -> Snapshot {
    #[cfg(unix)]
    let uid = unsafe { libc::geteuid() };
    #[cfg(not(unix))]
    let uid = 0;
    collect_uid(root, uid)
}

pub fn collect_uid(root: &Path, uid: u32) -> Snapshot {
    let mut entries = BTreeMap::new();
    let sockets = socket_table(root);
    let listeners = unix_listeners(root);
    let boot_id = fs::read_to_string(root.join("sys/kernel/random/boot_id"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let boot_time = fs::read_to_string(root.join("stat"))
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            line.strip_prefix("btime ")
                .and_then(|value| value.parse::<f64>().ok())
        })
        .unwrap_or(0.0);
    #[cfg(target_os = "linux")]
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    #[cfg(not(target_os = "linux"))]
    let ticks = 100.0;
    let Ok(dirs) = fs::read_dir(root) else {
        return Snapshot { boot_id, entries };
    };
    for dir in dirs.flatten() {
        let Ok(pid) = dir.file_name().to_string_lossy().parse() else {
            continue;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if dir.metadata().ok().map(|m| m.uid()) != Some(uid) {
                continue;
            }
        }
        let Some((process, parent)) = identity(root, pid) else {
            continue;
        };
        let path = dir.path();
        let env = fs::read(path.join("environ")).unwrap_or_default();
        let mut identities = Vec::new();
        let mut connection = None;
        for part in env.split(|v| *v == 0) {
            let Ok(part) = std::str::from_utf8(part) else {
                continue;
            };
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            if key == "SSH_CONNECTION" {
                connection = Connection::parse(value);
            }
            if let Some((_, source)) = IDENTITIES.iter().find(|(k, _)| *k == key) {
                identities.push((source.to_string(), value.to_ascii_lowercase()));
            }
        }
        identities.sort_by_key(|(source, sid)| {
            IDENTITIES
                .iter()
                .position(|(_, s)| *s == source)
                .unwrap_or(99)
                + usize::from(sid.is_empty()) * 99
        });
        let cmd = fs::read(path.join("cmdline")).unwrap_or_default();
        let mut args = cmd
            .split(|v| *v == 0)
            .filter_map(|v| std::str::from_utf8(v).ok());
        let name = args.next().unwrap_or("").rsplit('/').next().unwrap_or("");
        let args: Vec<_> = args.collect();
        let started_at = boot_time + process.start as f64 / ticks;
        let mut entry = Entry {
            process,
            started_at,
            parent,
            connection,
            identities,
            sockets: Vec::new(),
            multiplexed: args.iter().any(|a| {
                *a == "-M" || a.contains("ControlMaster=yes") || a.contains("ControlMaster=auto")
            }),
            shared_parent: name.starts_with("tmux"),
        };
        if name == "ssh"
            && let Ok(fds) = fs::read_dir(path.join("fd"))
        {
            let mut inodes = BTreeSet::new();
            for fd in fds.flatten() {
                let Ok(target) = fs::read_link(fd.path()) else {
                    continue;
                };
                let target = target.to_string_lossy();
                if let Some(inode) = target
                    .strip_prefix("socket:[")
                    .and_then(|s| s.strip_suffix(']'))
                    .and_then(|v| v.parse::<u64>().ok())
                {
                    inodes.insert(inode);
                }
            }
            entry.multiplexed |= inodes.iter().any(|i| listeners.contains(i));
            entry.sockets = inodes
                .iter()
                .filter_map(|i| sockets.get(i).cloned())
                .collect();
        }
        // /proc is not an atomic snapshot. Discard a process that exited or
        // changed identity while its environment and descriptors were read.
        if identity(root, pid).is_some_and(|(current, _)| current == entry.process) {
            entries.insert(pid, entry);
        }
    }
    Snapshot { boot_id, entries }
}
