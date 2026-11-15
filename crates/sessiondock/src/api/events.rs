//! Node UI invalidations share one cached observer.
use crate::{state::AppState, ui_events::Snapshot};
use axum::{extract::State, response::Response};

pub async fn events(State(state): State<AppState>) -> Response {
    let shutdown = state.shutdown.clone();
    let receiver = state.ui_events.clone().subscribe(move || {
        let state = state.clone();
        async move {
            let (sessions, live, term) = tokio::join!(
                state.reader.run(move |store| store.ui_sessions_snapshot()),
                super::runtime::live_value(&state, false),
                super::terminal::list_value(&state, false),
            );
            Some(Snapshot::from_sessions(
                sessions.ok()?,
                live.ok()?,
                term.ok()?,
            ))
        }
    });
    crate::ui_events::stream(receiver, shutdown)
}
