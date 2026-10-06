//! 会话最终画面：宿主退出时把终端模型的最后画面写成一份网格快照（带全部回滚
//! 历史，与网格 attach 的 `reset:true` 快照同一格式）到
//! `<dir>/screens/<created_ms>-<host_pid>-<name>/`，Web 服务据此只读显示已退出
//! 的会话。只在退出时写一次；写失败不影响退出流程，只在 stderr 记一行。

use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

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

fn private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// 退出时要保存的内容。`meta` 是宿主 `run --meta` 的 JSON。
pub struct FinalScreen<'a> {
    pub name: &'a str,
    pub created_ms: u64,
    pub argv: &'a [String],
    pub cwd: Option<&'a str>,
    pub meta: &'a Value,
    pub cols: u16,
    pub rows: u16,
    /// 网格快照 JSON 行。
    pub snapshot: &'a str,
    pub exit: &'a Value,
}

impl FinalScreen<'_> {
    /// 先写进同级临时目录再整体改名，读取端只会看到完整的一份。
    pub fn write(&self, base: &Path) -> io::Result<()> {
        let screens = base.join("screens");
        private_dir(&screens)?;
        let id = format!(
            "{}-{}-{}",
            self.created_ms,
            std::process::id(),
            safe_name(self.name)
        );
        let temporary = screens.join(format!(".{id}.tmp"));
        let _ = std::fs::remove_dir_all(&temporary);
        private_dir(&temporary)?;
        let meta = json!({
            "name": self.name,
            "host_pid": std::process::id(),
            "created_ms": self.created_ms,
            "ended_ms": now_unix_ms(),
            "argv": self.argv,
            "cwd": self.cwd.unwrap_or(""),
            "meta": self.meta,
            "cols": self.cols,
            "rows": self.rows,
            "exit": self.exit,
        });
        let written = (|| {
            std::fs::write(temporary.join("snapshot.json"), self.snapshot)?;
            std::fs::write(temporary.join("meta.json"), meta.to_string())?;
            std::fs::rename(&temporary, screens.join(&id))
        })();
        if written.is_err() {
            let _ = std::fs::remove_dir_all(&temporary);
        }
        written
    }
}
