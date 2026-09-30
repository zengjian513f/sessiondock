//! Name a report through the Codex TUI before sending its first model task.
//! Native index confirmation belongs to the same managed instance. No title
//! override and no writes to CLI-owned files are involved.
use super::*;

impl Conversations {
    pub(super) async fn prepare_report_name(
        &self,
        identity: &Identity,
        lease: &LeaseHandle,
        input: &SendInput,
    ) -> Result<(), Failure> {
        if identity.source != "codex" || !input.request_id.starts_with("report-send:") {
            return Ok(());
        }
        let Some((record, note)) = identity.record.as_ref().and_then(|record| {
            self.reports
                .as_ref()?
                .pending_decoration(record.record_id())
                .map(|note| (record, note))
        }) else {
            return Ok(());
        };
        if input.request_id != format!("report-send:{}", note["report_id"].as_str().unwrap_or("")) {
            return Ok(());
        }
        let title = note["title"].as_str().unwrap_or("BUG:");
        let request = format!("report-rename:{}", record.record_id());
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
                    return Ok(());
                }
                self.report_local_command(identity, lease, &format!("/rename {title}"))
                    .await?;
            }
            self.report_local_command(identity, lease, "/status")
                .await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            static SESSION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                regex::Regex::new(
                    r"Session:\s*([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})",
                )
                .unwrap()
            });
            loop {
                // /status identifies the guarded TUI even when no rollout or
                // process-to-native binding exists yet. Never match by title.
                let history = self
                    .driver
                    .capture_history(lease)
                    .await
                    .map_err(driver_error)?;
                if let Some(sid) = SESSION
                    .captures_iter(&history)
                    .last()
                    .map(|c| c[1].to_owned())
                {
                    let name = self
                        .reader
                        .run(move |store| store.codex_name(&sid))
                        .await
                        .map_err(|e| Failure::new(e.status.as_u16(), e.code, e.message))?;
                    if name.as_deref() == Some(title) {
                        self.ensure_sendable(identity, lease).await?;
                        return Ok(());
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(Failure::new(
                        409,
                        "report_rename_unconfirmed",
                        "未确认 Codex 原生命名成功，缺陷任务尚未发送；请检查终端，草稿已保留",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        .await;
        match result {
            Ok(()) => {
                self.store.finish(
                    &identity.key,
                    &request,
                    "sent",
                    json!({"ok":true,"state":"sent"}),
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
