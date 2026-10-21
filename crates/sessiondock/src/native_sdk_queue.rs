//! Private durable input admission journal. Native files and provider runtimes
//! remain provider-owned; this journal never infers consumption from an ACK.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Input {
    pub id: String,
    pub request_id: String,
    pub delivery_id: String,
    pub operation: String,
    pub target: Value,
    pub draft: Value,
    pub input: Option<Value>,
    pub state: String,
    pub error: Option<String>,
    pub result: Option<Value>,
    pub consumption: String,
    pub attempts: u64,
    #[serde(default)]
    pub actions: BTreeMap<String, Value>,
    #[serde(default)]
    pub stop_requested: bool,
}

type Document = BTreeMap<String, Vec<Input>>;
pub(super) struct Journal {
    path: Option<PathBuf>,
    rows: Mutex<Document>,
    pub events: broadcast::Sender<Arc<str>>,
}

pub(super) fn scope(target: &Value) -> String {
    json!([
        target["source"],
        target["uid"],
        target["agent"].as_str().unwrap_or_default()
    ])
    .to_string()
}

pub(super) fn input_id(operation: &str, request: &str) -> String {
    if operation == "turn" {
        format!("bridge:{request}")
    } else {
        format!("bridge:{operation}:{request}")
    }
}

fn terminal(input: &Input) -> bool {
    matches!(input.state.as_str(), "completed" | "cancelled")
}

fn log_transition(before: Option<&Input>, after: &Input, reason: &str) {
    if before.is_some_and(|before| {
        before.state == after.state
            && before.consumption == after.consumption
            && before.delivery_id == after.delivery_id
            && before.stop_requested == after.stop_requested
    }) {
        return;
    }
    crate::log::transition(
        "info",
        "native_sdk.input_transition",
        json!({"source":after.target["source"],"uid":after.target["uid"],
            "session_id":after.target["session_id"],"agent":after.target["agent"],
            "request_id":after.request_id,"delivery_id":after.delivery_id,
            "previous_delivery_id":before.map(|row|row.delivery_id.as_str()),
            "from":before.map(|row|row.state.as_str()),"to":after.state,
            "previous_consumption":before.map(|row|row.consumption.as_str()),
            "consumption":after.consumption,"attempts":after.attempts,
            "error_present":after.error.is_some(),"stop_requested":after.stop_requested,
            "reason":reason}),
    );
}

impl Journal {
    pub fn open(directory: Option<&PathBuf>) -> io::Result<Self> {
        let path = directory.map(|path| path.join("native-sdk-inputs.json"));
        if let Some(parent) = path.as_ref().and_then(|path| path.parent()) {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
        }
        let mut rows: Document = match path.as_ref().map(fs::read).transpose() {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Ok(None) => BTreeMap::new(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        let mut recovered = Vec::new();
        for input in rows.values_mut().flatten() {
            let before = input.clone();
            // Older bridges classified execution errors as failed sends even
            // after observing native consumption. Preserve the evidence and
            // require explicit reconciliation before any repeat submission.
            if input.state == "failed" && input.consumption == "observed" {
                input.state = "unknown".into();
            }
            if matches!(input.state.as_str(), "sending" | "accepted" | "recovering") {
                if directory.is_some()
                    && cfg!(unix)
                    && input.operation == "turn"
                    && matches!(
                        input.target["source"].as_str(),
                        Some("agy" | "claude" | "grok" | "kiro")
                    )
                {
                    input.state = "recovering".into();
                    input.error = None;
                } else {
                    input.state = "unknown".into();
                    input.error = Some("服务已重启，原生投递结果未确认；请核对会话后明确重试或移除，系统不会自动重发。".into());
                }
            } else if input.state == "preparing"
                || (input.operation != "turn" && input.state == "queued")
            {
                input.state = "failed".into();
                input.error =
                    Some("准备输入时服务重启，尚未调用原生发送；请明确重试或撤销。".into());
            }
            if before.state != input.state {
                recovered.push((before, input.clone()));
            }
        }
        let (events, _) = broadcast::channel(128);
        let journal = Self {
            path,
            rows: Mutex::new(rows),
            events,
        };
        journal
            .save(&journal.rows.lock().expect("SDK input journal"))
            .map_err(io::Error::other)?;
        for (before, after) in recovered {
            log_transition(Some(&before), &after, "service_recovery");
        }
        Ok(journal)
    }

    fn save(&self, rows: &Document) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let temporary = path.with_extension("tmp");
        let result = (|| -> io::Result<()> {
            let mut options = fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(rows)?)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| format!("SDK 输入队列保存失败：{error}"))
    }

    fn publish(&self, key: &str, rows: &Document) {
        let _=self.events.send(Arc::from(json!({"scope":key,"type":"input_queue","raw":{"input_queue":Self::visible_rows(rows,key)}}).to_string()));
    }

    pub fn stop(&self, key: &str) -> Result<(), String> {
        let mut rows = self.rows.lock().map_err(|_| "SDK input journal poisoned")?;
        let mut next = rows.clone();
        for row in next.get_mut(key).into_iter().flatten() {
            if !terminal(row) {
                row.state = "cancelled".into();
                row.stop_requested = true;
            }
        }
        self.save(&next)?;
        for after in next.get(key).into_iter().flatten() {
            let before = rows
                .get(key)
                .and_then(|rows| rows.iter().find(|row| row.id == after.id));
            log_transition(before, after, "stop");
        }
        *rows = next;
        self.publish(key, &rows);
        Ok(())
    }

    pub fn begin(&self, target: &Value, operation: &str, draft: Value) -> Result<Input, String> {
        let request = draft["request_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("SDK 输入缺少 request_id")?;
        let key = scope(target);
        let id = input_id(operation, request);
        let mut rows = self.rows.lock().map_err(|_| "SDK input journal poisoned")?;
        if let Some(input) = rows
            .get(&key)
            .and_then(|rows| rows.iter().find(|row| row.id == id))
        {
            return Ok(input.clone());
        }
        let input = Input {
            id,
            request_id: request.into(),
            delivery_id: request.into(),
            operation: operation.into(),
            target: target.clone(),
            draft,
            input: None,
            state: "preparing".into(),
            error: None,
            result: None,
            consumption: "unverified".into(),
            attempts: 0,
            actions: BTreeMap::new(),
            stop_requested: false,
        };
        let mut next = rows.clone();
        next.entry(key.clone()).or_default().push(input.clone());
        self.save(&next)?;
        log_transition(None, &input, "admission");
        *rows = next;
        self.publish(&key, &rows);
        Ok(input)
    }

    pub fn change(
        &self,
        key: &str,
        id: &str,
        mutate: impl FnOnce(&mut Input) -> Result<(), String>,
    ) -> Result<Input, String> {
        let mut rows = self.rows.lock().map_err(|_| "SDK input journal poisoned")?;
        let mut next = rows.clone();
        let input = next
            .get_mut(key)
            .and_then(|rows| rows.iter_mut().find(|row| row.id == id))
            .ok_or("SDK 队列项目不存在")?;
        let before = input.clone();
        mutate(input)?;
        let input = input.clone();
        self.save(&next)?;
        log_transition(Some(&before), &input, "update");
        *rows = next;
        self.publish(key, &rows);
        Ok(input)
    }

    pub fn first(&self, key: &str) -> Option<Input> {
        self.rows
            .lock()
            .ok()?
            .get(key)?
            .iter()
            .find(|row| row.operation == "turn" && !terminal(row))
            .cloned()
    }

    pub fn runnable(&self) -> Vec<(String, Value)> {
        self.rows
            .lock()
            .expect("SDK input journal")
            .iter()
            .filter_map(|(key, rows)| {
                let row = rows
                    .iter()
                    .find(|row| row.operation == "turn" && !terminal(row))?;
                (matches!(row.state.as_str(), "queued" | "accepted" | "recovering")
                    || (row.state == "unknown"
                        && row.consumption == "observed"
                        && row.target["source"] == "agy"))
                    .then(|| (key.clone(), row.target.clone()))
            })
            .collect()
    }

    fn visible_rows(rows: &Document, key: &str) -> Value {
        json!(rows.get(key).into_iter().flatten().filter(|row|!terminal(row)).map(|row|json!({
            "id":row.id,"origin":"bridge","operation":row.operation,"request_id":row.request_id,
            "state":if row.state=="recovering"{"accepted"}else{&row.state},"text":row.draft["text"],"error":row.error,"consumption":row.consumption,
            "observed_consumed":row.consumption=="observed","attempts":row.attempts,
            "owner":if row.target["source"]=="opencode"{"native"}else{"bridge"},
            "attachments":row.input.as_ref().map(|input|input["attachments"].clone()).unwrap_or_else(||row.draft["attachments"].clone())
        })).collect::<Vec<_>>())
    }

    pub fn visible(&self, key: &str) -> Value {
        Self::visible_rows(&self.rows.lock().expect("SDK input journal"), key)
    }

    pub fn subscribe(&self, key: &str) -> (Value, broadcast::Receiver<Arc<str>>) {
        let rows = self.rows.lock().expect("SDK input journal");
        (Self::visible_rows(&rows, key), self.events.subscribe())
    }

    pub fn receipt(input: &Input) -> Value {
        let mut result = input
            .result
            .clone()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        result["request_id"] = json!(input.request_id);
        result["queue_id"] = json!(input.id);
        result["accepted"] = json!(matches!(
            input.state.as_str(),
            "queued" | "sending" | "accepted" | "recovering" | "completed"
        ));
        result["queued"] = json!(matches!(
            input.state.as_str(),
            "queued" | "sending" | "accepted" | "recovering"
        ));
        result["state"] = json!(if input.state == "recovering" {
            "accepted"
        } else {
            &input.state
        });
        result["consumption"] = json!(input.consumption);
        result["observed_consumed"] = json!(input.consumption == "observed");
        result["cancelled"] = json!(input.state == "cancelled");
        result["error"] = json!(input.error);
        result
    }
}
