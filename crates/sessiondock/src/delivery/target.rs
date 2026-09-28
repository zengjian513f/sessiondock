//! Managed-instance target resolution and the failure type shared by
//! conversation SEND and the bug-report worker.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::driver::{DeliveryTarget, DriverError};
use crate::state::Reader;

/// Resolves a native UID to the managed instance a SEND writes to.
pub trait TargetResolver: Send + Sync {
    fn resolve<'a>(&'a self, uid: &'a str) -> BoxFuture<'a, Result<DeliveryTarget, Failure>>;
}

/// Production resolver: the unique guard-capable managed host whose verified
/// native association names this UID (runtime catalog), authorized through
/// the lifecycle binding when the instance originates from a launch receipt.
/// Duplicate, unmatched, exited or unauthorized instances are "not linked".
/// It is provider-neutral: the bound target's own source decides nothing here
/// beyond the identity the runtime already verified. A Codex rollback branch
/// with no binding of its own resolves to the host bound to its ancestor when
/// that host's pane runs the branch's process (`RuntimeSnapshot::fork_host`),
/// so the console taken over for the parent keeps delivering after `Esc Esc`.
pub struct ManagedResolver {
    pub runtime: Arc<crate::runtime::ManagedRuntime>,
    pub reader: Reader,
    pub lifecycle: Option<Arc<crate::lifecycle::service::LifecycleService>>,
    pub probes: Arc<Semaphore>,
    /// Process evidence for the fork rule; without a scanner a branch stays
    /// unlinked rather than guessed from the fork graph alone.
    pub proc_scan: Option<Arc<crate::runtime::procscan::ProcScanner>>,
}

fn unlinked() -> Failure {
    Failure::new(
        409,
        "terminal_unlinked",
        "终端会话未连接：该会话没有唯一关联的运行中受管实例",
    )
}

impl TargetResolver for ManagedResolver {
    fn resolve<'a>(&'a self, uid: &'a str) -> BoxFuture<'a, Result<DeliveryTarget, Failure>> {
        Box::pin(async move {
            let _permit = self
                .probes
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| Failure::new(503, "runtime_unavailable", "受控进程观察已关闭"))?;
            let catalog = self
                .reader
                .run(|store| store.native_catalog())
                .await
                .map_err(api_failure)?;
            // Both are fresh observations, independent of one another. Do not
            // serialize a full process scan behind every host probe.
            let (observed, process) =
                tokio::join!(self.runtime.observe(&catalog), self.process_evidence(uid),);
            let observed = observed.map_err(|_| {
                Failure::new(
                    503,
                    "runtime_unavailable",
                    "受控 host 目录不可用或超出观察预算",
                )
            })?;
            let mut matches = observed
                .hosts
                .iter()
                .filter_map(|host| host.bound_target())
                .filter(|target| target.uid() == uid);
            let process_target = process.as_ref().and_then(|(scan, sessions)| {
                observed
                    .codex_process_host(scan, sessions, uid)
                    .or_else(|| observed.fork_host(scan, sessions, uid))?
                    .bound_target()
            });
            let target = match (process_target, matches.next()) {
                (Some(target), _) => target,
                (None, Some(_)) if matches.next().is_some() => return Err(unlinked()),
                (None, Some(target)) => target,
                (None, None) => return Err(unlinked()),
            };
            if target.origin_launch_id().is_some() {
                let lifecycle = self.lifecycle.as_ref().ok_or_else(unlinked)?;
                lifecycle
                    .authorize_native(target)
                    .await
                    .map_err(|_| unlinked())?;
            }
            // The host verifies every capture and write against its own
            // binding: for a rollback branch that is the ancestor's uid.
            Ok(DeliveryTarget {
                name: target.name().to_owned(),
                uid: target.uid().to_owned(),
                instance_id: target.instance_id().to_owned(),
                bound: Arc::new(target.clone()),
            })
        })
    }
}

impl ManagedResolver {
    /// Prefer the TUI actually holding the rollout after /new or a fork to
    /// a second resume that declares this SID but is waiting for its lock.
    async fn process_evidence(
        &self,
        uid: &str,
    ) -> Option<(
        Arc<crate::runtime::procscan::Scan>,
        Vec<crate::runtime::procscan::SessionRow>,
    )> {
        if !uid.starts_with("codex:") {
            return None;
        }
        let scanner = self.proc_scan.as_ref()?;
        let document = self.reader.run(|store| store.list_recent()).await.ok()?;
        let sessions = crate::runtime::procscan::SessionRow::from_list(&document);
        let scan = scanner.snapshot(true).await.ok()?;
        Some((scan.scan, sessions))
    }
}

/// Failure: `{error, code, ...extra}` with an HTTP status.
#[derive(Clone, Debug)]
pub struct Failure {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub extra: Value,
}

impl Failure {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            extra: Value::Null,
        }
    }
}

impl From<DriverError> for Failure {
    fn from(error: DriverError) -> Self {
        let mut failure = Failure::new(error.status, error.code, error.message);
        if let Some(ip) = error.owner_ip {
            failure.extra = json!({"owner": {"ip": ip}});
        }
        failure
    }
}

/// Reader failures keep their code; an unknown session is a 400.
fn api_failure(error: crate::error::ApiError) -> Failure {
    if error.code == "session_error" && error.status.as_u16() == 404 {
        return Failure::new(400, "session_error", "只能向已有的原生会话发送");
    }
    Failure::new(error.status.as_u16(), error.code, error.message)
}
