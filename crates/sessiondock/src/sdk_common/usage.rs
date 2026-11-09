//! Shared Claude/CodeBuddy stream-json accounting; borrowed fields keep each
//! adapter's checkpoint schema and context controls independent.

use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) struct Usage<'a> {
    pub usage: &'a mut Value,
    pub model_usage: &'a mut Value,
    pub message_usage: &'a mut BTreeMap<String, Value>,
}

fn number(value: &Value, field: &str) -> f64 {
    value[field].as_f64().unwrap_or(0.0)
}

impl Usage<'_> {
    pub(crate) fn models(&mut self, models: &Value, cost: Option<&Value>) {
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
        *self.model_usage = models.clone();
        self.usage["tokens"] = json!({"input":input,"output":output,"total_tokens":input+output+read+write,
        "reasoning":if has_reasoning {json!(reasoning)} else {Value::Null},
        "cache":{"read":read,"write":write}});
        self.usage["cost_usd"] = cost
            .filter(|v| v.is_number())
            .cloned()
            .unwrap_or(json!(total_cost));
        self.usage["basis"] =
            json!("native cumulative modelUsage; estimated cost; thinking included in output");
        self.usage["model_usage"] = models.clone();
    }

    pub(crate) fn result(&mut self, frame: &Value) {
        let known = number(&self.usage["tokens"], "total_tokens") > 0.0
            || number(&self.usage, "cost_usd") > 0.0;
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
            self.usage["last_error_accounting"] =
                json!({"model_usage":frame["modelUsage"],"cost_usd":frame["total_cost_usd"]});
            return;
        }
        self.models(&frame["modelUsage"], frame.get("total_cost_usd"));
    }

    pub(crate) fn assistant(&mut self, id: &str, native: &Value) {
        let Some(fields) = native.as_object() else {
            return;
        };
        // Several completed blocks share the same API message ID and can carry
        // partial counters. Merge maxima for that message; never add its blocks.
        let entry = self
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
        if self.model_usage.is_object() {
            return;
        }
        let mut totals =
            json!({"input":0.0,"output":0.0,"reasoning":null,"cache":{"read":0.0,"write":0.0}});
        for message in self.message_usage.values() {
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
        self.usage["tokens"] = totals;
        self.usage["basis"] =
            json!("observed API message IDs; partial until native modelUsage arrives");
    }
}
