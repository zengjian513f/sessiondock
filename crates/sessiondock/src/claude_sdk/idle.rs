//! Reclaim only an unobserved, inactive runtime; native files remain untouched.
use super::*;

const IDLE: Duration = Duration::from_secs(10 * 60);

impl Bridge {
    pub(super) fn start_reaper(self: &Arc<Self>) {
        let bridge = Arc::downgrade(self);
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! { _ = shutdown.cancelled() => break, _ = ticks.tick() => {} }
                let Some(bridge) = bridge.upgrade() else {
                    break;
                };
                let sessions: Vec<_> = lock(&bridge.state).sessions.values().cloned().collect();
                for session in sessions {
                    let Ok(_runtime) = session.runtime.try_write() else {
                        continue;
                    };
                    let retiring = {
                        let mut live = lock(&session.live);
                        if live.outbound.is_none()
                            || live.events.receiver_count() != 0
                            || !live.turns.is_empty()
                            || live.native_running
                            || !live.requests.is_empty()
                            || !live.controls.is_empty()
                            || live.refreshing_usage
                            || !live.background_tasks.is_empty()
                        {
                            live.idle_since = None;
                            None
                        } else {
                            let now = tokio::time::Instant::now();
                            let since = *live.idle_since.get_or_insert(now);
                            if now.duration_since(since) < IDLE {
                                None
                            } else {
                                live.ready = false;
                                live.idle_stop.clone().zip(live.runtime_done.clone())
                            }
                        }
                    };
                    if let Some((stop, mut done)) = retiring {
                        stop.cancel();
                        // Keep the runtime gate until the old process group is
                        // reaped. A new open cannot send into a retiring stdin.
                        while !*done.borrow_and_update() {
                            if done.changed().await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });
    }
}
