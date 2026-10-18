//! Name a report or continuation through the Codex TUI before sending its first model task.
//! Native index confirmation belongs to the same managed instance. No title
//! override and no writes to CLI-owned files are involved.
use super::*;

fn status_sid(history: &str) -> Option<String> {
    static SESSION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"Session:\s*([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})",
        )
        .unwrap()
    });
    SESSION
        .captures_iter(history)
        .last()
        .map(|c| c[1].to_owned())
}

impl Conversations {
    /// The rename handshake already asked this exact guarded TUI for /status.
    /// A shared daemon owns its rollout, so process ancestry cannot recover
    /// that identity. Retain the CLI's answer rather than guessing by title.
    pub(crate) async fn codex_status_identity(&self, record: &Record) -> Option<String> {
        if record.spec().source() != crate::lifecycle::model::Source::Codex {
            return None;
        }
        let key = self
            .store
            .canonical(&format!("launch:{}", record.record_id()));
        let receipt = ["report-rename", "continue-rename"]
            .into_iter()
            .find_map(|prefix| {
                self.store
                    .request(&key, &format!("{prefix}:{}", record.record_id()))
                    .filter(|receipt| receipt.phase == "sent")
            })?;
        if let Some(sid) = receipt.result["sid"].as_str() {
            return Some(sid.to_owned());
        }
        // Upgrade recovery: old successful rename receipts omitted the SID.
        // Only read their retained status card; never inject a new command or
        // claim a browser's terminal. Missing scrollback stays pending.
        let target = self
            .lifecycle
            .target(record.record_id().to_owned())
            .await
            .ok()?;
        let lease = self.driver.conversation_launch(target);
        let history = self.driver.capture_history(&lease).await.ok()?;
        status_sid(&history)
    }

    /// Old report receipts retained neither SID nor the status card. Their
    /// server-minted report ID still correlates a completed guarded SEND with
    /// the native user task. Titles and arbitrary quoted mentions do not count.
    pub(crate) fn sent_report_identity(&self, record: &Record) -> Option<String> {
        let key = self
            .store
            .canonical(&format!("launch:{}", record.record_id()));
        let rename = self
            .store
            .request(&key, &format!("report-rename:{}", record.record_id()))?;
        if rename.phase != "sent" || rename.result["sid"].is_string() {
            return None;
        }
        let note = self
            .reports
            .as_ref()?
            .pending_decoration(record.record_id())?;
        let id = note["report_id"].as_str()?;
        let sent = self.store.request(&key, &format!("report-send:{id}"))?;
        (sent.phase == "sent").then(|| id.to_owned())
    }

    pub(super) async fn prepare_report_name(
        &self,
        identity: &Identity,
        lease: &LeaseHandle,
        input: &SendInput,
    ) -> Result<(), Failure> {
        if identity.source != "codex" {
            return Ok(());
        }
        let Some(record) = identity.record.as_ref() else {
            return Ok(());
        };
        let draft = self.store.draft(&identity.key);
        let continuation = draft.value["continuation_title"].as_str().filter(|_| {
            draft.value["requestId"] == input.request_id && draft.value["text"] == input.text
        });
        let note = self
            .reports
            .as_ref()
            .and_then(|reports| reports.pending_decoration(record.record_id()));
        let (title, request) = if let Some(title) = continuation {
            (title, format!("continue-rename:{}", record.record_id()))
        } else if let Some(note) = note.as_ref().filter(|note| {
            input.request_id == format!("report-send:{}", note["report_id"].as_str().unwrap_or(""))
        }) {
            (
                note["title"].as_str().unwrap_or("BUG:"),
                format!("report-rename:{}", record.record_id()),
            )
        } else {
            return Ok(());
        };
        let previous = self.store.request(&identity.key, &request);
        if previous.as_ref().is_some_and(|old| old.phase == "sent") {
            return Ok(());
        }
        let result = async {
            // A previous unconfirmed rename may already have succeeded. Never
            // replay it: query this exact launch and reconcile its native name.
            if previous.is_none() {
                if let Some(old) =
                    self.store
                        .begin(&identity.key, &request, json!({"title":title}))?
                {
                    submission_result(&old)?;
                    return Ok(old.result["sid"].as_str().map(str::to_owned));
                }
                self.report_local_command(identity, lease, &format!("/rename {title}"))
                    .await?;
            }
            self.report_local_command(identity, lease, "/status")
                .await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            loop {
                // /status identifies the guarded TUI even when no rollout or
                // process-to-native binding exists yet. Never match by title.
                let history = self
                    .driver
                    .capture_history(lease)
                    .await
                    .map_err(driver_error)?;
                if let Some(sid) = status_sid(&history) {
                    let native_sid = sid.clone();
                    let name = self
                        .reader
                        .run(move |store| store.codex_name(&sid))
                        .await
                        .map_err(|e| Failure::new(e.status.as_u16(), e.code, e.message))?;
                    if name.as_deref() == Some(title) {
                        self.ensure_sendable(identity, lease).await?;
                        return Ok(Some(native_sid));
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(Failure::new(
                        409,
                        if continuation.is_some() {
                            "continue_rename_unconfirmed"
                        } else {
                            "report_rename_unconfirmed"
                        },
                        "未确认 Codex 原生命名成功，任务尚未发送；请检查终端，草稿已保留",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        .await;
        match result {
            Ok(sid) => {
                self.store.finish(
                    &identity.key,
                    &request,
                    "sent",
                    json!({"ok":true,"state":"sent","sid":sid}),
                    None,
                )?;
                Ok(())
            }
            Err(error) => {
                self.store.finish(
                    &identity.key,
                    &request,
                    "error",
                    json!({"error":error.message,"code":error.code}),
                    None,
                )?;
                Err(error)
            }
        }
    }
    async fn report_local_command(
        &self,
        identity: &Identity,
        lease: &LeaseHandle,
        command: &str,
    ) -> Result<(), Failure> {
        self.ensure_sendable(identity, lease).await?;
        let before = self.driver.capture(lease).await.map_err(driver_error)?;
        input::classify("codex", &before).result()?;
        self.driver
            .paste(lease, command)
            .await
            .map_err(driver_error)?;
        input::wait_for_pasted_editor("codex", command, &before, || async {
            self.driver.capture(lease).await.map_err(driver_error)
        })
        .await?;
        self.driver
            .keys(lease, &["Enter"])
            .await
            .map_err(driver_error)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let screen = self.driver.capture(lease).await.map_err(driver_error)?;
            let status = input::classify("codex", &screen);
            if status.ready() {
                return Ok(());
            }
            if (status.code != input::INPUT_PENDING && status.state != input::InputState::Starting)
                || tokio::time::Instant::now() >= deadline
            {
                return status.result();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
