//! Retry only confirmed model-selection failures; never replay consumed input.
use super::{Bridge, transport::Session};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(super) const CONTINUE: &str = "重试：继续上次因模型不可用而中断的任务。保留已完成的工作，从未完成处继续，不要重复执行已完成的操作。";

#[derive(Default)]
pub(super) struct Retry {
    pub deadline: Option<Instant>,
    pub delay: u64,
    pub cancelled: bool,
}

impl super::live::LiveState {
    pub(super) fn schedule_model_retry(&mut self, error: &Value) {
        let message = error["message"].as_str().unwrap_or_default();
        let confirmed = error["data"]["errorType"] == "InvalidModelError"
            || ((message.starts_with("The model '") || message.starts_with("The selected model "))
                && message
                    .contains("is not available. Please select a different model and try again."));
        if !confirmed || self.model_retry.cancelled || self.interrupted_turn {
            self.cancel_model_retry();
            return;
        }
        let delay = if self.model_retry.delay == 0 {
            15
        } else {
            (self.model_retry.delay * 2).min(600)
        };
        self.model_retry.delay = delay;
        self.model_retry.deadline = Some(Instant::now() + Duration::from_secs(delay));
        self.retry = Some(
            json!({"delay_seconds":delay,"retry_at":(super::transport::now_secs()+delay)*1000,"error":error}),
        );
    }

    pub(super) fn cancel_model_retry(&mut self) {
        self.model_retry = Retry {
            cancelled: true,
            ..Retry::default()
        };
        self.retry = None;
    }
}

impl Bridge {
    pub(crate) fn retry_candidates(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(uid, session)| {
                session
                    .state
                    .lock()
                    .unwrap()
                    .model_retry
                    .deadline
                    .map(|_| uid.clone())
            })
            .collect()
    }

    pub(crate) fn cancel_model_retry(&self, uid: &str) {
        if let Ok(session) = self.get_session(uid) {
            session.cancel_model_retry();
        }
    }

    pub(crate) async fn retry_model(&self, uid: &str) {
        let lock = self.lock_for(uid);
        let _guard = lock.lock().await;
        if self.shutdown.is_cancelled() {
            return;
        }
        let Ok(session) = self.get_session(uid) else {
            return;
        };
        if let Err(error) = session.continue_model_retry().await {
            let mut st = session.state.lock().unwrap();
            st.cancel_model_retry();
            st.runtime_error = Some(error);
            st.publish();
        }
    }
}

impl Session {
    pub(super) fn cancel_model_retry(&self) {
        let mut st = self.state.lock().unwrap();
        st.cancel_model_retry();
        st.publish();
    }
}
