//! Native safe-point interjections. Native enqueue is not model consumption.
use super::{Bridge, Session, required};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

impl Bridge {
    pub(super) async fn steer(&self, session: &Session, input: &Value) -> Result<Value, String> {
        let request_id = required(input, "request_id")?;
        let text = required(input, "text")?;
        let _gate = session.steer_gate.lock().await;
        {
            let state = session.live.lock().unwrap();
            if let Some(receipt) = state.steers.get(request_id) {
                return cached(receipt);
            }
        }
        self.check_conflict(&session.sid, &session.path, Some(session.connection.pid))
            .await?;
        let response = {
            let mut state = session.live.lock().unwrap();
            if state.generation != session.connection.generation
                || !session.connection.alive.load(Ordering::Acquire)
            {
                return Err("grok_sdk_disconnected".into());
            }
            if !state.running || !state.prompt_started {
                return Err("grok_sdk_steer_requires_active_turn".into());
            }
            let mut params =
                json!({"sessionId":session.sid,"text":text,"interjectionId":request_id});
            if let Some(images) = input.get("images").filter(|v| !v.is_null()) {
                let images = images.as_array().ok_or("grok_sdk_invalid_images")?;
                if !images.is_empty() {
                    if !state.images {
                        return Err("grok_sdk_images_unsupported".into());
                    }
                    let mut content = vec![json!({"type":"text","text":text})];
                    for image in images {
                        content.push(json!({"type":"image","data":required(image,"data")?,"mimeType":required(image,"mimeType")?}));
                    }
                    params["content"] = json!(content);
                }
            }
            // Register before the transport can deliver an actor echo. Do not
            // retry an uncertain write: the native interjection id is an echo
            // correlation key, not a documented native idempotency guarantee.
            let generation = state.generation;
            let order = state.seq + 1;
            state.steers.insert(
                request_id.to_owned(),
                json!({"request_id":request_id,"interjection_id":request_id,
                "session_id":session.sid,"text":text,"status":"dispatching","accepted":false,
                "native_received":false,"consumed":null,"generation":generation,"order":order}),
            );
            let response = session.connection.begin_request("x.ai/interject", params);
            state.publish();
            response
        };
        let result = match response {
            Ok(response) => response
                .await
                .unwrap_or_else(|_| Err("grok_sdk_disconnected".to_owned())),
            Err(error) => Err(error),
        };
        let mut state = session.live.lock().unwrap();
        let mut result = result;
        if let Ok(native) = &result {
            let payload = native.get("result").unwrap_or(native);
            if !native.get("error").is_none_or(Value::is_null) || payload["status"] != "queued" {
                result = Err("grok_sdk_interject_ack_unknown".into());
            }
        }
        if let Err(error) = &result {
            if error == "grok_sdk_native_error: code -32601" {
                state.extensions.insert("steer".into(), false);
            }
        } else {
            state.extensions.insert("steer".into(), true);
        }
        let receipt = state.steers.get_mut(request_id).expect("registered steer");
        match &result {
            Ok(_) => {
                receipt["accepted"] = json!(true);
                if receipt["native_received"] != true {
                    receipt["status"] = json!("native_queued");
                }
            }
            Err(error) => {
                let rejected = matches!(
                    error.as_str(),
                    "grok_sdk_native_error: code -32601" | "grok_sdk_native_error: code -32602"
                );
                receipt["status"] = json!(if rejected { "rejected" } else { "uncertain" });
                receipt["accepted"] = if rejected { json!(false) } else { Value::Null };
                receipt["error"] = json!(error);
            }
        }
        let receipt = receipt.clone();
        state.publish();
        result?;
        Ok(receipt)
    }
}

fn cached(receipt: &Value) -> Result<Value, String> {
    if let Some(error) = receipt["error"].as_str() {
        Err(error.to_owned())
    } else {
        Ok(receipt.clone())
    }
}
