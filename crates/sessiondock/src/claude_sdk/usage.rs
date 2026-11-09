//! Native cumulative model totals and summary context have different scopes.
use super::*;
use crate::sdk_common::usage::Usage;

fn counters(live: &mut Live) -> Usage<'_> {
    Usage {
        usage: &mut live.usage,
        model_usage: &mut live.model_usage,
        message_usage: &mut live.message_usage,
    }
}

pub(super) fn models(live: &mut Live, models: &Value, cost: Option<&Value>) {
    counters(live).models(models, cost);
}

pub(super) fn result(live: &mut Live, frame: &Value) {
    counters(live).result(frame);
}

pub(super) fn assistant(live: &mut Live, id: &str, native: &Value) {
    counters(live).assistant(id, native);
}

pub(super) fn context(live: &mut Live, value: &Value, basis: &str) {
    let used = value
        .get("totalTokens")
        .or_else(|| value.get("total_tokens"));
    let max = value
        .get("maxTokens")
        .or_else(|| value.get("rawMaxTokens"))
        .or_else(|| value.get("raw_max_tokens"));
    if let (Some(used), Some(max)) = (
        used.filter(|v| v.is_number()),
        max.filter(|v| v.is_number()),
    ) {
        live.context_usage = value.clone();
        live.usage["context"] = json!({"used_tokens":used,"max_tokens":max,
            "percent":value["percentage"],"basis":basis});
    }
}

impl Session {
    pub(super) async fn refresh_usage(&self) {
        // An unavailable experimental control is a feature downgrade, never a
        // failed turn or a fabricated zero. Keep the last known useful values.
        for (name, request) in [
            (
                "context",
                json!({"subtype":"get_context_usage","detail":"summary"}),
            ),
            (
                "usage",
                json!({"subtype":"get_usage","skip_behaviors":true}),
            ),
        ] {
            let result = self.control(request).await;
            let mut live = lock(&self.live);
            match result {
                Ok(value) => {
                    live.usage_errors.remove(name);
                    if name == "context" {
                        context(
                            &mut live,
                            &value,
                            "native summary: last response usage and local estimates",
                        );
                    } else {
                        models(
                            &mut live,
                            &value["session"]["model_usage"],
                            value["session"].get("total_cost_usd"),
                        );
                        live.usage["native_usage"] = value.clone();
                        // Null means unavailable for this provider, not 0%.
                        if value["rate_limits_available"] == true {
                            live.rate_limits = value["rate_limits"].clone();
                        }
                        live.usage["rate_limits_available"] =
                            value["rate_limits_available"].clone();
                    }
                }
                Err(error) => {
                    live.usage_errors.insert(name.into(), error);
                }
            }
            self.publish(&mut live);
        }
    }

    pub(super) fn schedule_usage(self: &Arc<Self>) {
        {
            let mut live = lock(&self.live);
            if live.refreshing_usage || live.outbound.is_none() {
                return;
            }
            live.refreshing_usage = true;
        }
        let session = self.clone();
        tokio::spawn(async move {
            let _runtime = session.runtime.read().await;
            if lock(&session.live).outbound.is_none() {
                lock(&session.live).refreshing_usage = false;
                return;
            }
            session.refresh_usage().await;
            lock(&session.live).refreshing_usage = false;
        });
    }
}
