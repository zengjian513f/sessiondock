//! Read-only evidence for SDK admission, not a process-control or list API.
//! Adapters retain their native SID/shared-runtime policies. Missing evidence
//! never invents a global lock. Run snapshot on a blocking worker.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, serde::Deserialize)]
pub(crate) struct Process {
    pub pid: u32,
    #[serde(default)]
    pub ppid: u32,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub started: f64,
}

impl Process {
    pub fn is_cli(&self, name: &str) -> bool {
        // Native executable or a node/bun script, never arbitrary prompt args.
        let executable = |arg: &str| {
            let base = arg.rsplit(['/', '\\']).next().unwrap_or(arg);
            base.trim_end_matches(".exe")
                .trim_end_matches(".cmd")
                .eq_ignore_ascii_case(name)
        };
        self.args.first().is_some_and(|a| executable(a))
            || self.args.first().is_some_and(|a| {
                ["node", "bun", "nodejs"].iter().any(|n| {
                    a.rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(a)
                        .trim_end_matches(".exe")
                        == *n
                }) && self.args.get(1).is_some_and(|a| {
                    executable(a)
                        || (a.ends_with("/cli.js") || a.ends_with("\\cli.js"))
                            && a.replace('\\', "/")
                                .split('/')
                                .any(|part| part == name || part == format!("{name}-code"))
                })
            })
    }

    pub fn option(&self, names: &[&str]) -> Option<&str> {
        for (i, arg) in self.args.iter().enumerate() {
            if arg == "--" {
                break;
            }
            for name in names {
                if arg == name {
                    return self
                        .args
                        .get(i + 1)
                        .map(String::as_str)
                        .filter(|v| !v.starts_with('-'));
                }
                if let Some(value) = arg.strip_prefix(name).and_then(|v| v.strip_prefix('=')) {
                    return Some(value);
                }
            }
        }
        None
    }

    pub fn holds(&self, path: &Path) -> bool {
        self.files.iter().any(|file| same_path(file, path))
    }

    pub fn same_cwd(&self, path: &Path) -> bool {
        self.cwd.as_ref().is_some_and(|cwd| same_path(cwd, path))
    }

    pub fn session_matches(&self, sid: &str, path: &Path) -> bool {
        self.option(&["--session-id", "--resume-id", "--resume", "-r"])
            .is_some_and(|value| {
                value.eq_ignore_ascii_case(sid) || same_path(Path::new(value), path)
            })
            || self.holds(path)
    }

    /// Kiro's own open diagnostic log, with timestamps newer than this process.
    pub fn kiro_sessions(&self) -> BTreeSet<String> {
        use std::io::BufRead;
        let mut result = BTreeSet::new();
        for path in &self.files {
            let text = path.to_string_lossy().replace('\\', "/");
            if !text.contains("/.kiro/logs/") || !text.ends_with("/kiro.log") {
                continue;
            }
            let Ok(file) = std::fs::File::open(path) else {
                continue;
            };
            for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
                let Ok(row) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                let Some(at) = row["timestamp"]
                    .as_str()
                    .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                else {
                    continue;
                };
                if at.timestamp_millis() as f64 / 1000.0 < self.started {
                    continue;
                }
                if let Some(rest) = row["message"].as_str().and_then(|m| {
                    m.strip_prefix("[KiroAgent] ACP session/load complete sessionId=")
                }) && let Some(sid) = rest.split_whitespace().next()
                {
                    result.insert(sid.to_ascii_lowercase());
                }
                if row["rootTurnId"].is_string()
                    && let Some(sid) = row["rootConversationId"].as_str().filter(|s| !s.is_empty())
                {
                    result.insert(sid.to_ascii_lowercase());
                }
            }
        }
        result
    }
}

pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

pub(crate) struct Snapshot {
    pub processes: Vec<Process>,
    parents: BTreeMap<u32, (u32, f64)>,
}
impl Snapshot {
    fn new(processes: Vec<Process>) -> Self {
        let parents = processes
            .iter()
            .map(|p| (p.pid, (p.ppid, p.started)))
            .collect();
        Self { processes, parents }
    }

    pub fn owns(&self, pid: u32, own_pids: &BTreeSet<u32>) -> bool {
        let mut current = pid;
        let mut seen = BTreeSet::new();
        while current != 0 && seen.insert(current) {
            if own_pids.contains(&current) {
                return true;
            }
            let Some((parent, started)) = self.parents.get(&current) else {
                break;
            };
            if let Some((_, parent_started)) = self.parents.get(parent)
                && *started > 0.0
                && parent_started > started
            {
                break;
            }
            current = *parent;
        }
        false
    }
}

pub(crate) fn snapshot(proc_root: &Path, files: &[PathBuf]) -> Result<Snapshot, String> {
    // Same private proc-root fixture boundary as the existing SDK tests. There
    // is no production environment override for a native process snapshot.
    if proc_root != Path::new("/proc") && proc_root.join("sdk-writers.json").is_file() {
        let bytes = std::fs::read(proc_root.join("sdk-writers.json")).map_err(|e| e.to_string())?;
        return serde_json::from_slice(&bytes)
            .map(Snapshot::new)
            .map_err(|e| format!("SDK writer snapshot: {e}"));
    }
    native_snapshot(proc_root, files)
}

#[cfg(target_os = "linux")]
fn native_snapshot(proc_root: &Path, _files: &[PathBuf]) -> Result<Snapshot, String> {
    let tree = crate::runtime::procscan::ProcTree::open(proc_root.to_path_buf());
    let mut processes = Vec::new();
    for entry in std::fs::read_dir(proc_root)
        .map_err(|e| format!("SDK writer enumeration: {e}"))?
        .flatten()
    {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let path = entry.path();
        let Ok(bytes) = std::fs::read(path.join("cmdline")) else {
            continue;
        };
        let args = bytes
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let ppid = std::fs::read_to_string(path.join("stat"))
            .ok()
            .and_then(|s| {
                s.rsplit_once(')')
                    .and_then(|(_, fields)| fields.split_whitespace().nth(1)?.parse().ok())
            })
            .unwrap_or(0);
        let files = std::fs::read_dir(path.join("fd"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|fd| std::fs::read_link(fd.path()).ok())
            .collect();
        processes.push(Process {
            pid,
            ppid,
            args,
            cwd: std::fs::read_link(path.join("cwd")).ok(),
            files,
            started: tree.started_at(pid).unwrap_or(0.0),
        });
    }
    Ok(Snapshot::new(processes))
}

#[cfg(any(target_os = "macos", windows))]
fn output(command: &mut std::process::Command) -> Result<Vec<u8>, String> {
    let result = command
        .output()
        .map_err(|e| format!("SDK writer enumeration: {e}"))?;
    if !result.status.success() {
        return Err(format!("SDK writer enumeration exited {}", result.status));
    }
    Ok(result.stdout)
}

#[cfg(target_os = "macos")]
fn native_snapshot(_proc_root: &Path, _files: &[PathBuf]) -> Result<Snapshot, String> {
    let listing = output(std::process::Command::new("/bin/ps").args(["-axo", "pid=,ppid=,uid="]))?;
    let mut processes = Vec::new();
    for line in String::from_utf8_lossy(&listing).lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 3 || fields[2].parse::<u32>().ok() != Some(unsafe { libc::geteuid() }) {
            continue;
        }
        let Ok(pid) = fields[0].parse::<u32>() else {
            continue;
        };
        let Some(started) = mac_started(pid) else {
            continue;
        };
        // KERN_PROCARGS2 returns argc, executable, padding, then NUL argv. Do
        // not read beyond argc into the environment (which may hold secrets).
        let mut mib = [libc::CTL_KERN, 49, pid as i32];
        let mut size = 0usize;
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            continue;
        }
        let mut bytes = vec![0u8; size];
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                bytes.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
            || size < 4
        {
            continue;
        }
        bytes.truncate(size);
        let argc = i32::from_ne_bytes(bytes[..4].try_into().unwrap()).max(0) as usize;
        let mut pos = 4;
        while pos < size && bytes[pos] != 0 {
            pos += 1;
        }
        while pos < size && bytes[pos] == 0 {
            pos += 1;
        }
        let args: Vec<_> = bytes[pos..]
            .split(|b| *b == 0)
            .take(argc)
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        processes.push(Process {
            pid,
            ppid: fields[1].parse().unwrap_or(0),
            args,
            cwd: None,
            files: Vec::new(),
            started,
        });
    }
    // lsof emits NUL-delimited paths, so spaces/newlines are not split. Collect
    // cwd and file identities once, including node wrappers and DB writers.
    let result = std::process::Command::new("/usr/sbin/lsof")
        .args([
            "-nP",
            "-a",
            "-u",
            &unsafe { libc::geteuid() }.to_string(),
            "-F0pfn",
        ])
        .output()
        .map_err(|e| format!("SDK file observation: {e}"))?;
    if !result.status.success() && result.stdout.is_empty() {
        return Err("SDK file observation failed".into());
    }
    let mut pid = 0;
    let mut cwd = false;
    for field in result.stdout.split(|b| *b == 0) {
        let field = field.strip_prefix(b"\n").unwrap_or(field);
        if field.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&field[1..]);
        match field[0] {
            b'p' => pid = text.parse().unwrap_or(0),
            b'f' => cwd = text == "cwd",
            b'n' => {
                if let Some(process) = processes.iter_mut().find(|p| p.pid == pid) {
                    if cwd {
                        process.cwd = Some(PathBuf::from(text.as_ref()));
                    } else if text.starts_with('/') {
                        process.files.push(PathBuf::from(text.as_ref()));
                    }
                }
            }
            _ => {}
        }
    }
    processes.retain(|process| mac_started(process.pid) == Some(process.started));
    Ok(Snapshot::new(processes))
}

#[cfg(target_os = "macos")]
fn mac_started(pid: u32) -> Option<f64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    if unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    } != size
    {
        return None;
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid
        || info.pbi_uid != unsafe { libc::geteuid() }
        || info.pbi_status == libc::SZOMB
    {
        return None;
    }
    Some(info.pbi_start_tvsec as f64 + info.pbi_start_tvusec as f64 / 1_000_000.0)
}

#[cfg(windows)]
fn native_snapshot(_proc_root: &Path, files: &[PathBuf]) -> Result<Snapshot, String> {
    use std::os::windows::process::CommandExt;
    let bytes = output(
        std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                include_str!("writers/windows.ps1"),
            ])
            .env(
                "SESSIONDOCK_WRITER_FILES",
                serde_json::to_string(files).map_err(|e| e.to_string())?,
            )
            .creation_flags(0x08000000),
    )?;
    let text = String::from_utf8_lossy(&bytes);
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map(Snapshot::new)
        .map_err(|e| format!("SDK native writer response: {e}"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn native_snapshot(_proc_root: &Path, _files: &[PathBuf]) -> Result<Snapshot, String> {
    Err("SDK writer observation unsupported on this platform".into())
}
