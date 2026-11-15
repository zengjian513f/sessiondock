//! Incremental native management-port discovery. The log is append-only within
//! one runtime; replacement, truncation and a different requested PID reset it.
use crate::file_version::FileVersion;
use std::{
    io::{BufRead, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

const UNREADABLE: &str =
    "agy_sdk_control_unavailable: 无法取得本实例的原生管理接口；任务继续运行。";
#[derive(Clone, Default)]
struct Observation {
    matched: bool,
    port: Option<u16>,
}
impl Observation {
    fn line(&mut self, line: &str, pid: u32, complete: bool) {
        let line = line
            .strip_suffix('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .unwrap_or(line);
        if complete && line.ends_with(&format!("Starting language server process with pid {pid}")) {
            self.matched = true;
            self.port = None;
        } else if self.matched && self.port.is_none() {
            self.port = line
                .split_once("Language server listening on random port at ")
                .and_then(|(_, tail)| tail.strip_suffix(" for HTTP"))
                .and_then(|port| port.parse().ok());
        }
    }
    fn result(&self) -> Result<u16, &'static str> {
        if !self.matched {
            Err("agy_sdk_control_unavailable: 原生接口进程身份不匹配；任务继续运行。")
        } else {
            self.port
                .ok_or("agy_sdk_control_unavailable: 此 CLI 未提供本机管理接口。")
        }
    }
}
struct Cursor {
    version: FileVersion,
    pid: u32,
    offset: u64,
    tail: String,
    observation: Observation,
}
static LOGS: LazyLock<Mutex<indexmap::IndexMap<PathBuf, Cursor>>> = LazyLock::new(Default::default);

pub(super) async fn port(path: &Path, pid: u32) -> Result<u16, String> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || read(&path, pid))
        .await
        .map_err(|_| UNREADABLE.to_string())?
        .map_err(str::to_owned)
}
fn read(path: &Path, pid: u32) -> Result<u16, &'static str> {
    let mut logs = LOGS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(version) = FileVersion::read(path) else {
        logs.shift_remove(path);
        return Err(UNREADABLE);
    };
    let mut cursor = logs
        .shift_remove(path)
        .filter(|prior| {
            prior.pid == pid
                && prior.version.same_file(&version)
                && version.size() >= prior.offset
                && (version.size() > prior.offset || prior.version == version)
        })
        .unwrap_or_else(|| Cursor {
            version: version.clone(),
            pid,
            offset: 0,
            tail: String::new(),
            observation: Observation::default(),
        });
    if cursor.offset < version.size() {
        let mut file = std::fs::File::open(path).map_err(|_| UNREADABLE)?;
        file.seek(SeekFrom::Start(cursor.offset))
            .map_err(|_| UNREADABLE)?;
        // Limit this read to the observed extent; appends during the read are
        // processed on the next call with their own filesystem version.
        let mut reader =
            std::io::BufReader::new(std::io::Read::take(file, version.size() - cursor.offset));
        loop {
            let before = cursor.tail.len();
            let read = reader.read_line(&mut cursor.tail).map_err(|_| UNREADABLE)?;
            cursor.offset += read as u64;
            if read == 0 {
                break;
            }
            if cursor.tail.ends_with('\n') {
                cursor.observation.line(&cursor.tail, pid, true);
                cursor.tail.clear();
            } else {
                debug_assert!(cursor.tail.len() > before);
                break;
            }
        }
    }
    cursor.version = version;
    // An unterminated final port line has the same meaning as str::lines().
    // Re-evaluate it after future appends instead of committing partial text.
    let mut latest = cursor.observation.clone();
    latest.line(&cursor.tail, pid, false);
    let result = latest.result();
    // Failed discovery is retried from the start. An incomplete/invalid
    // startup log may be rewritten in place before the next observation.
    if result.is_ok() {
        logs.insert(path.to_owned(), cursor);
    }
    while logs.len() > 128 {
        logs.shift_remove_index(0);
    }
    result
}
