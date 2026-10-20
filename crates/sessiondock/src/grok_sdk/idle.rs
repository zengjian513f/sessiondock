//! Release only demonstrably idle native runtimes; retain identity and receipts.
use super::Bridge;
use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const REAP_INTERVAL: Duration = Duration::from_secs(15);

impl Bridge {
    pub(super) fn start_idle_reaper(self: &Arc<Self>) {
        let bridge = Arc::downgrade(self);
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => return,
                    _ = tokio::time::sleep(REAP_INTERVAL) => {},
                }
                let Some(bridge) = bridge.upgrade() else {
                    return;
                };
                bridge.reap_idle();
            }
        });
    }

    fn reap_idle(&self) {
        // Attach/open and subscribe use the same publication lock. An open
        // refreshes the deadline before exposing a runtime to an HTTP call.
        let Ok(_attach) = self.attach.try_lock() else {
            return;
        };
        let sessions: Vec<_> = self.sessions.lock().unwrap().values().cloned().collect();
        for session in sessions {
            if !session.connection.alive.load(Ordering::Acquire) {
                continue;
            }
            let Ok(_turn) = session.turns.clone().try_lock_owned() else {
                session.live.lock().unwrap().idle_since = None;
                continue;
            };
            let mut state = session.live.lock().unwrap();
            if state.running
                || !state.requests.is_empty()
                || state.events.receiver_count() > 0
                || !state.native_tasks.is_empty()
                || state.incomplete_tasks
                || state.has_pending_steers()
                || session.connection.has_pending(&state)
            {
                state.idle_since = None;
                continue;
            }
            let since = *state.idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() < IDLE_TIMEOUT {
                continue;
            }
            state.idle_reclaimed = true;
            state.idle_since = None;
            session.connection.alive.store(false, Ordering::Release);
            session.connection.stop.cancel();
            state.publish();
        }
    }
}
