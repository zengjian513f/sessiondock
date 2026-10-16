//! Native deletion through Agy's own resume picker. No native SQLite writes.
use super::launcher::Launcher;
use ptyhost_client::{CaptureKind, ControlOp, ControlReply, HostClient, Limits};
use rusqlite::{Connection, OpenFlags};
use std::{path::Path, time::Duration};

pub(crate) static DELETE: std::sync::LazyLock<std::sync::Arc<tokio::sync::Mutex<()>>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Mutex::new(())));
const NAME: &str = "agy-delete";

fn present(home: &Path, sid: &str) -> Result<bool, String> {
    let db = Connection::open_with_flags(
        home.join("conversation_summaries.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "无法读取 Agy 原生会话索引".to_owned())?;
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_summaries WHERE conversation_id=?1)",
        [sid],
        |r| r.get(0),
    )
    .map_err(|_| "无法核对 Agy 原生会话索引".to_owned())
}

fn deleted(home: &Path, sid: &str) -> Result<bool, String> {
    if present(home, sid)? {
        return Ok(false);
    }
    for path in [
        home.join("conversations").join(format!("{sid}.db")),
        home.join("brain").join(sid),
    ] {
        if path
            .try_exists()
            .map_err(|_| "无法核对 Agy 原生文件删除结果")?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn send(client: &HostClient, text: &str) -> Result<(), String> {
    client
        .request(NAME, ControlOp::Send { text: text.into() })
        .await
        .map(|_| ())
        .map_err(|_| "Agy 删除输入未确认，请刷新列表核对".into())
}

async fn screen(client: &HostClient) -> Option<crate::delivery::driver::ScreenCapture> {
    let ControlReply::Capture(c) = client
        .request(
            NAME,
            ControlOp::Capture {
                kind: CaptureKind::Screen,
                styled: false,
                join: false,
                lines: 0,
            },
        )
        .await
        .ok()?
    else {
        return None;
    };
    if c.lag != Some(0) || c.dropped != Some(0) || c.resets != Some(0) {
        return None;
    }
    Some(crate::delivery::driver::ScreenCapture {
        text: c.text,
        cursor: (c.cursor[0], c.cursor[1]),
        lag: c.lag,
        dropped: c.dropped,
        resets: c.resets,
        alt: c.alt,
    })
}

async fn wait_screen(
    client: &HostClient,
    accept: impl Fn(&crate::delivery::driver::ScreenCapture) -> bool,
) -> Result<(), String> {
    loop {
        if let Some(frame) = screen(client).await
            && accept(&frame)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn picker_row<'a>(text: &'a str, sid: &str) -> Option<&'a str> {
    // Anchor in the picker below the last editor boundary, never in history.
    let (_, menu) = text.rsplit_once("(tab to cycle)")?;
    let rows: Vec<_> = menu
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    (rows.first() == Some(&"Conversations")
        && rows.get(1) == Some(&format!("Search: {sid}").as_str())
        && rows.get(2).is_some_and(|s| s.starts_with("> [CURRENT] "))
        && rows
            .get(3)
            .is_some_and(|s| s.starts_with("Keyboard:") && s.contains("f4 Delete")))
    .then(|| rows[2])
}

pub async fn remove(launcher: &Launcher, sid: &str, cwd: &Path, home: &Path) -> Result<(), String> {
    if deleted(home, sid)? {
        return Ok(());
    }
    let id = super::store::fresh_request_id().map_err(|_| "无法创建 Agy 删除任务")?;
    let directory = launcher.host_dir().join(format!(".agy-delete-{id}"));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&directory)
        .map_err(|_| "无法创建 Agy 删除工作目录")?;
    let result = async {
        let client = HostClient::new(
            &directory,
            Limits {
                operation_timeout: Duration::from_secs(2),
                ..Limits::default()
            },
        )
        .map_err(|_| "无法连接 Agy 删除终端")?;
        let mut command = launcher
            .agy_delete_command(&directory, sid, cwd)
            .map_err(|_| "Agy 启动项不可用")?;
        let mut child = command.spawn().map_err(|_| "Agy 删除终端启动失败")?;
        let operation = async {
            wait_screen(&client, |c| {
                crate::conversation::agy_editor(c).as_deref() == Some("")
            })
            .await?;
            send(&client, "/resume\r").await?;
            wait_screen(&client, |c| {
                c.text
                    .rsplit_once("(tab to cycle)")
                    .is_some_and(|(_, menu)| {
                        menu.contains("Type to search conversations...")
                            && menu.contains("f4 Delete")
                    })
            })
            .await?;
            // SID comes from the native inventory; paste it as text, not terminal controls.
            client
                .request(
                    NAME,
                    ControlOp::Paste {
                        text: sid.into(),
                        bracketed: true,
                    },
                )
                .await
                .map_err(|_| "Agy 会话选择未确认")?;
            wait_screen(&client, |c| picker_row(&c.text, sid).is_some()).await?;
            let before = screen(&client).await.ok_or("Agy 会话选择画面丢失")?;
            let selected = picker_row(&before.text, sid).ok_or("Agy 会话选择已变化")?;
            let markers = selected.matches("[Delete? (y/n)]").count();
            send(&client, "\x1bOS").await?; // F4, advertised by the verified picker.
            wait_screen(&client, |c| {
                picker_row(&c.text, sid)
                    .is_some_and(|s| s.matches("[Delete? (y/n)]").count() == markers + 1)
            })
            .await?;
            send(&client, "y").await?;
            while !deleted(home, sid)? {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok::<_, String>(())
        };
        let outcome = tokio::time::timeout(Duration::from_secs(25), operation)
            .await
            .unwrap_or_else(|_| {
                Err("Agy 原生删除未确认（启动、菜单或删除超时），请刷新列表核对后重试".into())
            });
        // Always stop the private worker, including partial/failed menu operations.
        let _ = client.request(NAME, ControlOp::Kill { force: true }).await;
        let _ = tokio::task::spawn_blocking(move || {
            let _ = child.kill();
            let _ = child.wait();
        })
        .await;
        // A lost terminal acknowledgement after confirmation is resolved by native state.
        if deleted(home, sid)? {
            Ok(())
        } else {
            outcome.and(Err("Agy 原生会话仍存在，删除未完成".into()))
        }
    }
    .await;
    let _ = std::fs::remove_dir_all(directory);
    result
}
