//! Local liveness JSON: version, `api_version` 1, and `stage: "read_only"`.
//! Configured metadata is snapshotted off the reactor; an uncertain commit is
//! 503 and is never retried here. Audit counters stay zero with `enabled:false`
//! when intake is unconfigured. This is not old Hub node-protocol compatibility.
use crate::{error::ApiError, state::AppState};
use axum::{Json, extract::State};
use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
    api_version: u32,
    stage: &'static str,
    /// Browser diagnostics intake counters; `enabled:false` when unconfigured.
    audit: crate::audit::Snapshot,
}

pub async fn get_health(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    if let Some(metadata) = state.metadata {
        // snapshot() can wait on an admitted fsync transaction's mutex. Keep
        // that wait off the reactor, and surface an uncertain commit as 503.
        state
            .reader
            .run(move |_| {
                metadata
                    .snapshot()
                    .map(|_| ())
                    .map_err(|error| crate::sessions::SessionError {
                        status: error.status,
                        message: error.message,
                    })
            })
            .await?;
    }
    Ok(Json(HealthResponse {
        status: "ok",
        service: "sessiondock",
        version: env!("CARGO_PKG_VERSION"),
        api_version: 1,
        stage: "read_only",
        audit: state
            .audit
            .as_ref()
            .map(|service| service.snapshot())
            .unwrap_or_default(),
    }))
}
