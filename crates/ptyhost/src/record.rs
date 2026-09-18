//! 会话录制接线：把 pty 输出、尺寸变化和退出按模型消费顺序写进
//! `ptyhost-record` 的分段日志（`<dir>/records/<created_ms>-<host_pid>-<name>/`）。
//!
//! 只有屏幕线程和 finish 调用这里；写盘失败不影响会话，只是停止录制并在 stderr
//! 记一次。checkpoint 取自终端模型（与 attach 回放同构），在每个新分段开头，
//! 因此淘汰最旧分段后，剩余最旧分段仍能独立重建画面。

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ptyhost_record::Frame;
use ptyhost_record::store::{Store, StoreConfig};
use serde_json::{Value, json};

/// 落盘间隔：dirty 超过这个时间就 `sync_data`。
pub const SYNC_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug)]
pub struct RecordConfig {
    pub segment_bytes: u64,
    pub total_bytes: u64,
}

impl Default for RecordConfig {
    fn default() -> Self {
        let store = StoreConfig::default();
        Self {
            segment_bytes: store.segment_bytes,
            total_bytes: store.total_bytes,
        }
    }
}

pub struct Recorder {
    store: Store,
    dir: PathBuf,
    meta: Value,
    last_sync: Instant,
    failed: bool,
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 目录名里只留 `[A-Za-z0-9._-]`，其余换成 `_`，最多 64 字节。
fn safe_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if out.is_empty() || out.starts_with('.') {
        out.insert(0, '_');
    }
    out
}

fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, value.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

impl Recorder {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        base: &Path,
        name: &str,
        created_ms: u64,
        argv: &[String],
        cwd: Option<&str>,
        meta: &Value,
        cols: u16,
        rows: u16,
        config: RecordConfig,
    ) -> io::Result<Self> {
        let records = base.join("records");
        std::fs::create_dir_all(&records)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&records, std::fs::Permissions::from_mode(0o700));
        }
        let dir = records.join(format!(
            "{created_ms}-{}-{}",
            std::process::id(),
            safe_name(name)
        ));
        let store = Store::open(
            &dir,
            StoreConfig {
                segment_bytes: config.segment_bytes,
                total_bytes: config.total_bytes,
            },
        )?;
        let meta = json!({
            "name": name,
            "host_pid": std::process::id(),
            "created_ms": created_ms,
            "argv": argv,
            "cwd": cwd.unwrap_or(""),
            "meta": meta,
            "cols": cols,
            "rows": rows,
            "format": {"magic": "SDRC", "version": ptyhost_record::VERSION},
        });
        write_json(&dir.join("meta.json"), &meta)?;
        Ok(Self {
            store,
            dir,
            meta,
            last_sync: Instant::now(),
            failed: false,
        })
    }

    fn fail(&mut self, what: &str, error: io::Error) {
        if !self.failed {
            eprintln!("ptyhost: 录制停止（{what}）: {error}");
        }
        self.failed = true;
    }

    /// 下一条输出前是否需要先写一个 checkpoint（新分段）。
    pub fn needs_checkpoint(&self) -> bool {
        !self.failed && self.store.needs_segment()
    }

    /// 开新分段，首帧是当前画面。`state` 与 attach 回放字节同构。
    pub fn checkpoint(&mut self, cols: u16, rows: u16, state: Vec<u8>) {
        if self.failed {
            return;
        }
        if let Err(error) = self.store.begin_segment(now_unix_ms(), cols, rows, state) {
            self.fail("begin_segment", error);
        }
    }

    fn append(&mut self, frame: &Frame) {
        if self.failed {
            return;
        }
        match self.store.append(now_unix_ms(), frame) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("segment clock overflow") => {
                // 单段时钟溢出（约 49 天）：由调用方下一轮 needs_checkpoint 处理不了，
                // 这里直接强制开段——没有画面可取，用空状态，读取端会看到 gap。
                if let Err(error) = self.store.begin_segment(now_unix_ms(), 0, 0, Vec::new()) {
                    self.fail("begin_segment", error);
                    return;
                }
                if let Err(error) = self.store.append(now_unix_ms(), frame) {
                    self.fail("append", error);
                }
            }
            Err(error) => self.fail("append", error),
        }
    }

    pub fn output(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.append(&Frame::Output(bytes.to_vec()));
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.append(&Frame::Resize { cols, rows });
    }

    /// 到间隔就落盘；不在每帧上做。
    pub fn maybe_sync(&mut self) {
        if self.failed || self.last_sync.elapsed() < SYNC_INTERVAL {
            return;
        }
        self.last_sync = Instant::now();
        if let Err(error) = self.store.sync() {
            self.fail("sync", error);
        }
    }

    /// 会话结束：写 Exit 帧、关闭分段、把退出信息补进 meta.json。
    pub fn exit(&mut self, exit: &Value) {
        if !self.failed {
            self.append(&Frame::Exit(exit.to_string()));
            if let Err(error) = self.store.close() {
                self.fail("close", error);
            }
        }
        if let Some(map) = self.meta.as_object_mut() {
            map.insert("exit".into(), exit.clone());
            map.insert("ended_ms".into(), json!(now_unix_ms()));
        }
        let _ = write_json(&self.dir.join("meta.json"), &self.meta);
    }

    pub fn rename(&mut self, name: &str) {
        if let Some(map) = self.meta.as_object_mut() {
            map.insert("name".into(), json!(name));
        }
        let _ = write_json(&self.dir.join("meta.json"), &self.meta);
    }

    /// `info` 应答里的录制概况。
    pub fn info(&self) -> Value {
        json!({
            "dir": self.dir.to_string_lossy(),
            "segments": self.store.segments().len(),
            "bytes": self.store.total_bytes(),
            "active": !self.failed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::safe_name;

    #[test]
    fn names_are_filesystem_safe() {
        assert_eq!(safe_name("claude/main:1"), "claude_main_1");
        assert_eq!(safe_name(".hidden"), "_.hidden");
        assert_eq!(safe_name(""), "_");
        assert_eq!(safe_name(&"x".repeat(100)).len(), 64);
    }
}
