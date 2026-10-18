//! Native cumulative model totals and summary context have different scopes.
use super::*;

fn number(value: &Value, field: &str) -> f64 {
    value[field].as_f64().unwrap_or(0.0)
}

pub(super) fn models(live: &mut Live, models: &Value, cost: Option<&Value>) {
    let Some(entries) = models.as_object() else {
        return;
    };
    let mut input = 0.0;
    let mut output = 0.0;
    let mut read = 0.0;
    let mut write = 0.0;
    let mut reasoning = 0.0;
    let mut has_reasoning = false;
    let mut total_cost = 0.0;
    for entry in entries.values() {
        input += number(entry, "inputTokens");
        output += number(entry, "outputTokens");
        read += number(entry, "cacheReadInputTokens");
        write += number(entry, "cacheCreationInputTokens");
        total_cost += number(entry, "costUSD");
        if entry["thinkingTokens"].is_number() {
            has_reasoning = true;
            reasoning += number(entry, "thinkingTokens");
        }
    }
    live.model_usage = models.clone();
    live.usage["tokens"] = json!({"input":input,"output":output,"total_tokens":input+output+read+write,
        "reasoning":if has_reasoning {json!(reasoning)} else {Value::Null},
        "cache":{"read":read,"write":write}});
    live.usage["cost_usd"] = cost
        .filter(|v| v.is_number())
        .cloned()
        .unwrap_or(json!(total_cost));
    live.usage["basis"] =
        json!("native cumulative modelUsage; estimated cost; thinking included in output");
    live.usage["model_usage"] = models.clone();
}

pub(super) fn result(live: &mut Live, frame: &Value) {
    let known = number(&live.usage["tokens"], "total_tokens") > 0.0
        || number(&live.usage, "cost_usd") > 0.0;
    let zeroed = frame["modelUsage"].as_object().is_some_and(|models| {
        models.values().all(|model| {
            [
                "inputTokens",
                "outputTokens",
                "thinkingTokens",
                "cacheReadInputTokens",
                "cacheCreationInputTokens",
                "costUSD",
            ]
            .iter()
            .all(|field| number(model, field) == 0.0)
        })
    }) && frame["total_cost_usd"].as_f64() == Some(0.0);
    // The official protocol permits startup/crash error results to carry
    // placeholder zero accounting. They do not undo prior observed spending.
    if known && zeroed && frame["is_error"] == true {
        live.usage["last_error_accounting"] =
            json!({"model_usage":frame["modelUsage"],"cost_usd":frame["total_cost_usd"]});
        return;
    }
    models(live, &frame["modelUsage"], frame.get("total_cost_usd"));
}

pub(super) fn assistant(live: &mut Live, id: &str, native: &Value) {
    let Some(fields) = native.as_object() else {
        return;
    };
    // Several completed blocks share the same API message ID and can carry
    // partial counters. Merge maxima for that message; never add its blocks.
    let entry = live
        .message_usage
        .entry(id.into())
        .or_insert_with(|| json!({}));
    for (key, value) in fields {
        if value
            .as_f64()
            .is_some_and(|next| next >= number(entry, key))
        {
            entry[key] = value.clone();
        }
    }
    if live.model_usage.is_object() {
        return;
    }
    let mut totals =
        json!({"input":0.0,"output":0.0,"reasoning":null,"cache":{"read":0.0,"write":0.0}});
    for message in live.message_usage.values() {
        for (target, source) in [("input", "input_tokens"), ("output", "output_tokens")] {
            totals[target] = json!(number(&totals, target) + number(message, source));
        }
        for (target, source) in [
            ("read", "cache_read_input_tokens"),
            ("write", "cache_creation_input_tokens"),
        ] {
            totals["cache"][target] =
                json!(number(&totals["cache"], target) + number(message, source));
        }
    }
    totals["total_tokens"] = json!(
        number(&totals, "input")
            + number(&totals, "output")
            + number(&totals["cache"], "read")
            + number(&totals["cache"], "write")
    );
    live.usage["tokens"] = totals;
    live.usage["basis"] =
        json!("observed API message IDs; partial until native modelUsage arrives");
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
            session.refresh_usage().await;
            lock(&session.live).refreshing_usage = false;
        });
    }
}
