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
        if let Some(old) = self.store.request(&identity.key, &request) {
            submission_result(&old)?;
            return Ok(());
        }
        let command = format!("/rename {title}");
        let before = self.driver.capture(lease).await.map_err(driver_error)?;
        input::classify("codex", &before).result()?;
        if let Some(old) = self
            .store
            .begin(&identity.key, &request, json!({"title":title}))?
        {
            submission_result(&old)?;
            return Ok(());
        }
        let result = async {
            self.driver
                .paste(lease, &command)
                .await
                .map_err(driver_error)?;
            input::wait_for_pasted_editor("codex", &command, &before, || async {
                self.driver.capture(lease).await.map_err(driver_error)
            })
            .await?;
            self.driver
                .keys(lease, &["Enter"])
                .await
                .map_err(driver_error)?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            loop {
                let listing = self
                    .reader
                    .run(|store| store.list(false))
                    .await
                    .map_err(|e| Failure::new(e.status.as_u16(), e.code, e.message))?;
                for row in listing["sessions"].as_array().into_iter().flatten() {
                    if row["source"] != "codex" || row["renamed_to"] != title {
                        continue;
                    }
                    let Some(uid) = row["uid"].as_str() else {
                        continue;
                    };
                    if self
                        .resolver
                        .resolve(uid)
                        .await
                        .is_ok_and(|target| target.instance_id == lease.instance_id)
                    {
                        // Let the TUI consume its own name-updated notification.
                        tokio::time::sleep(Duration::from_millis(250)).await;
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
}
