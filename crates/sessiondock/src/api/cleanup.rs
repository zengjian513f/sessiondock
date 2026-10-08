use crate::state::AppState;
use axum::{Json, extract::State, http::header, response::IntoResponse};

pub async fn get(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.cleanup_counts.summary()),
    )
}

pub(crate) fn spawn(state: AppState) {
    state
        .cleanup_counts
        .clone()
        .spawn(state.shutdown.clone(), move || {
            let state = state.clone();
            async move {
                // Refresh the catalog first so the process-to-session mapping sees new rows.
                let sessions = state
                    .reader
                    .run_wait(&state.shutdown, |store| store.list_view(true))
                    .await
                    .map_err(|error| error.message)?;
                let live = super::runtime::live_document(&state, true)
                    .await
                    .map_err(|error| error.message)?;
                Ok((sessions, live))
            }
        });
}
