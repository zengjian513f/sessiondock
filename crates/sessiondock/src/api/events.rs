//! Node UI invalidations share one cached observer per debug view.
use crate::{state::AppState, ui_events::Snapshot};
use axum::{
    extract::{RawQuery, State},
    response::Response,
};
use serde_json::Value;

async fn document(response: Result<Response, crate::error::ApiError>) -> Option<Value> {
    let response = response.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}
pub async fn events(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    let view = crate::sessions::debug_run_of(query.as_deref());
    let shutdown = state.shutdown.clone();
    let receiver = state.ui_events.clone().subscribe(view.clone(), move || {
        let state = state.clone();
        let view = view.clone();
        let query = query.clone();
        async move {
            let (sessions, live, term) = tokio::join!(
                state.reader.run(move |store| store.list_view(false, &view)),
                super::runtime::live(State(state.clone()), RawQuery(query.clone())),
                super::terminal::list(State(state.clone()), RawQuery(query)),
            );
            Some(Snapshot::new(
                sessions.ok()?,
                document(live).await?,
                document(term).await?,
            ))
        }
    });
    crate::ui_events::stream(receiver, shutdown)
}
