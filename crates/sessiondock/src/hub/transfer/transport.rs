//! Select a configured data path without changing the migration transaction.
use super::*;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Report {
    pub method: String,
    pub network: Option<String>,
    pub fallback: Option<String>,
}

impl Transfers {
    pub(super) async fn transport(
        &self,
        client: &Client,
        source: &Target,
        target: &Target,
        supported: bool,
        operation: &Value,
        journal: &mut Journal,
    ) -> Result<(), TransferError> {
        let (source_id, _) = namespace::split(&journal.request.uid, true)
            .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
        let route = self.policy.route(&source_id, &journal.request.target_node);
        journal.transport = Some(Report {
            method: "hub".into(),
            network: route.map(|r| r.name.clone()),
            fallback: None,
        });
        if let Some(route) = route {
            if supported {
                journal.transport.as_mut().unwrap().method = "scp".into();
                self.save(journal).await?;
                let archive = call(
                    client,
                    source,
                    "/api/session/transfer/scp/prepare",
                    operation,
                    true,
                )
                .await?
                .1;
                let archive: crate::transfer::transport::Archive = serde_json::from_value(archive)?;
                archive.validate()?;
                if archive.operation_id != journal.request.operation_id {
                    return Err(TransferError::new("move_format", "源端迁移包与操作不符"));
                }
                journal.bytes_total = archive.bytes;
                journal.bytes_sent = 0;
                self.save(journal).await?;
                let received = call(
                    client,
                    target,
                    "/api/session/transfer/scp/receive",
                    &json!({"source": route.nodes[&source_id], "archive":archive}),
                    false,
                )
                .await;
                let received = match received {
                    Ok((_, value)) if value["code"].is_string() => (409, value),
                    Ok(received) => received,
                    Err(error) if error.code == "move_transport_unavailable" => {
                        (409, json!({"code": error.code, "error":error.message}))
                    }
                    Err(error) if error.code != "move_node_unavailable" => return Err(error),
                    Err(error) => {
                        // A lost response is not a failed copy. Wait for the
                        // target's worker before inspecting its durable receipt.
                        let settled = call(
                            client,
                            target,
                            "/api/session/transfer/scp/settle",
                            operation,
                            false,
                        )
                        .await?;
                        if settled.0 == 200 && settled.1["incoming"] == true {
                            settled
                        } else {
                            // With no receipt the original error is ambiguous;
                            // normal cancellation/recovery reconciles both ends.
                            return Err(error);
                        }
                    }
                };
                call(
                    client,
                    source,
                    "/api/session/transfer/scp/release",
                    &serde_json::to_value(&archive)?,
                    true,
                )
                .await?;
                if received.0 == 200 {
                    Self::received(&received.1, &journal.request.operation_id)?;
                    journal.bytes_sent = archive.bytes;
                    self.save(journal).await?;
                    return Ok(());
                }
                if received.1["code"] != "move_transport_unavailable" {
                    return Err(remote_error(&received.1));
                }
                // Only a transport failure acknowledged after scp has stopped
                // can fall back. Validation/conflict/cancellation errors cannot.
                let settled = call(
                    client,
                    target,
                    "/api/session/transfer/scp/settle",
                    operation,
                    false,
                )
                .await?;
                if settled.0 == 200 {
                    if settled.1["incoming"] != true {
                        return Err(TransferError::new(
                            "move_conflict",
                            "目标操作并非迁移接收记录",
                        ));
                    }
                    Self::received(&settled.1, &journal.request.operation_id)?;
                    journal.bytes_sent = archive.bytes;
                    self.save(journal).await?;
                    return Ok(());
                }
                if settled.0 != 404 {
                    return Err(remote_error(&settled.1));
                }
                journal.transport.as_mut().unwrap().fallback = Some(
                    received.1["error"]
                        .as_str()
                        .unwrap_or("SCP 不可用，改用 Hub 中转")
                        .into(),
                );
            } else {
                journal.transport.as_mut().unwrap().fallback =
                    Some("节点版本不支持直传，使用 Hub 中转".into());
            }
        }
        journal.transport.as_mut().unwrap().method = "hub".into();
        self.save(journal).await?;
        self.stream(client, source, target, operation, journal)
            .await
    }

    fn received(value: &Value, id: &str) -> Result<(), TransferError> {
        if value["operation_id"] != id
            || !matches!(
                value["phase"].as_str(),
                Some("planned" | "ready" | "complete")
            )
        {
            return Err(TransferError::new(
                "move_recovery_required",
                "目标接收结果尚未确认",
            ));
        }
        Ok(())
    }
}
