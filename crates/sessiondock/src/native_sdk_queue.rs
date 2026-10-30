//! Private durable input admission journal. Native files and provider runtimes
//! remain provider-owned; this journal never infers consumption from an ACK.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs, io,
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
    /// Execution observation is independent of the completed send receipt.
    #[serde(default)]
    pub runtime_state: String,
    #[serde(default)]
    pub runtime_error: Option<String>,
}

type Document = BTreeMap<String, Vec<Input>>;
pub(super) struct Journal {
    database: Mutex<rusqlite::Connection>,
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

// Receipts retain native verdicts and operation identities, never a second
// transcript. Draft/prepared input remain available for explicit recovery.
fn compact(input: &mut Input) {
    if let Some(raw) = input
        .result
        .as_mut()
        .and_then(|v| v.get_mut("raw"))
        .and_then(Value::as_object_mut)
    {
        raw.remove("items");
        raw.remove("subtasks");
    }
}
fn hot(input: &Input) -> bool {
    !terminal(input) || !input.runtime_state.is_empty()
}

fn log_transition(before: Option<&Input>, after: &Input, reason: &str) {
    if before.is_some_and(|before| {
        before.state == after.state
            && before.consumption == after.consumption
            && before.delivery_id == after.delivery_id
            && before.stop_requested == after.stop_requested
            && before.runtime_state == after.runtime_state
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
            "runtime_state":after.runtime_state,"previous_runtime_state":before.map(|row|row.runtime_state.as_str()),
            "reason":reason}),
    );
}

impl Journal {
    pub fn open(directory: Option<&PathBuf>) -> io::Result<Self> {
        use rusqlite::params;
        if let Some(directory) = directory {
            fs::create_dir_all(directory)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
            }
        }
        let mut database = match directory {
            Some(dir) => rusqlite::Connection::open(dir.join("native-sdk-inputs.sqlite")),
            None => rusqlite::Connection::open_in_memory(),
        }
        .map_err(io::Error::other)?;
        if let Some(directory) = directory {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    directory.join("native-sdk-inputs.sqlite"),
                    fs::Permissions::from_mode(0o600),
                )?;
            }
        }
        database
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS inputs(scope TEXT NOT NULL, id TEXT NOT NULL,
            active INTEGER NOT NULL, row TEXT NOT NULL, PRIMARY KEY(scope,id));
            CREATE INDEX IF NOT EXISTS active_inputs ON inputs(active);",
            )
            .map_err(io::Error::other)?;
        // One-time import. Commit before unlinking; INSERT OR IGNORE makes a
        // crash between commit and unlink safe without replaying old verdicts.
        if let Some(legacy) = directory
            .map(|dir| dir.join("native-sdk-inputs.json"))
            .filter(|p| p.exists())
        {
            let old: Document =
                serde_json::from_reader(io::BufReader::new(fs::File::open(&legacy)?))
                    .map_err(io::Error::other)?;
            let tx = database.transaction().map_err(io::Error::other)?;
            for (key, entries) in old {
                for mut row in entries {
                    compact(&mut row);
                    tx.execute(
                        "INSERT OR IGNORE INTO inputs VALUES (?1,?2,?3,?4)",
                        params![key, row.id, hot(&row), serde_json::to_string(&row)?],
                    )
                    .map_err(io::Error::other)?;
                }
            }
            tx.commit().map_err(io::Error::other)?;
            fs::remove_file(&legacy)?;
        }
        let mut rows: Document = BTreeMap::new();
        {
            let mut query = database
                .prepare("SELECT scope,row FROM inputs WHERE active=1 ORDER BY rowid")
                .map_err(io::Error::other)?;
            let entries = query
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(io::Error::other)?;
            for entry in entries {
                let (key, value) = entry.map_err(io::Error::other)?;
                rows.entry(key)
                    .or_default()
                    .push(serde_json::from_str(&value).map_err(io::Error::other)?);
            }
        }
        let mut recovered = Vec::new();
        for input in rows.values_mut().flatten() {
            let before = input.clone();
            // Upgrade legacy execution verdicts without reopening delivery.
            if input.operation == "turn"
                && (input.result.is_some() || input.consumption == "observed")
                && matches!(
                    input.state.as_str(),
                    "accepted" | "recovering" | "failed" | "unknown"
                )
            {
                if matches!(input.state.as_str(), "accepted" | "recovering") {
                    input.runtime_state = "observing".into();
                }
                input.state = "completed".into();
                input.runtime_error = input.error.take();
            }
            if !input.runtime_state.is_empty()
                || matches!(input.state.as_str(), "sending" | "recovering")
            {
                if directory.is_some()
                    && cfg!(unix)
                    && input.operation == "turn"
                    && matches!(
                        input.target["source"].as_str(),
                        Some("agy" | "claude" | "codebuddy" | "grok" | "kiro")
                    )
                {
                    input.runtime_state = "recovering".into();
                } else {
                    input.runtime_state.clear();
                    input.runtime_error = Some("服务已重启，原生运行状态未确认。".into());
                }
                if input.result.is_none() && input.consumption != "observed" {
                    if input.runtime_state == "recovering" {
                        // Fence retry actions while consulting the original
                        // host receipt for a send interrupted before its ACK.
                        input.state = "sending".into();
                        input.error = None;
                    } else {
                        input.state = "unknown".into();
                        input.error = Some("服务已重启，原生投递结果未确认；不会自动重发。".into());
                    }
                }
            } else if input.state == "preparing"
                || (input.operation != "turn" && input.state == "queued")
            {
                input.state = "failed".into();
                input.error =
                    Some("准备输入时服务重启，尚未调用原生发送；请明确重试或撤销。".into());
            }
            if before.state != input.state || before.runtime_state != input.runtime_state {
                recovered.push((before, input.clone()));
            }
        }
        let (events, _) = broadcast::channel(128);
        let journal = Self {
            database: Mutex::new(database),
            rows: Mutex::new(rows),
            events,
        };
        for (key, entries) in journal.rows.lock().expect("SDK input journal").iter() {
            for row in entries {
                journal.save(key, row).map_err(io::Error::other)?;
            }
        }
        journal
            .rows
            .lock()
            .expect("SDK input journal")
            .retain(|_, entries| {
                entries.retain(hot);
                !entries.is_empty()
            });
        for (before, after) in recovered {
            log_transition(Some(&before), &after, "service_recovery");
        }
        Ok(journal)
    }

    fn save(&self, key: &str, row: &Input) -> Result<(), String> {
        self.database.lock().map_err(|_| "SDK input database poisoned")?
            .execute("INSERT INTO inputs VALUES (?1,?2,?3,?4) ON CONFLICT(scope,id) DO UPDATE SET active=excluded.active,row=excluded.row",
                rusqlite::params![key,row.id,hot(row),serde_json::to_string(row).map_err(|e|e.to_string())?])
            .map(|_|()).map_err(|e|format!("SDK 输入队列保存失败：{e}"))
    }

    fn stored(&self, key: &str, id: &str) -> Result<Option<Input>, String> {
        use rusqlite::OptionalExtension;
        let value: Option<String> = self
            .database
            .lock()
            .map_err(|_| "SDK input database poisoned")?
            .query_row(
                "SELECT row FROM inputs WHERE scope=?1 AND id=?2",
                [key, id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        value
            .map(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))
            .transpose()
    }

    fn install(rows: &mut Document, key: &str, row: Input) {
        let entries = rows.entry(key.to_owned()).or_default();
        if let Some(index) = entries.iter().position(|entry| entry.id == row.id) {
            if hot(&row) {
                entries[index] = row;
            } else {
                entries.remove(index);
            }
        } else if hot(&row) {
            entries.push(row);
        }
        if entries.is_empty() {
            rows.remove(key);
        }
    }

    fn publish(&self, key: &str, rows: &Document) {
        let _=self.events.send(Arc::from(json!({"scope":key,"type":"input_queue","raw":{"input_queue":Self::visible_rows(rows,key)}}).to_string()));
    }

    pub fn stop(&self, key: &str) -> Result<(), String> {
        let mut rows = self.rows.lock().map_err(|_| "SDK input journal poisoned")?;
        for mut row in rows.get(key).cloned().unwrap_or_default() {
            let before = row.clone();
            row.runtime_state.clear();
            if !terminal(&row) {
                row.state = "cancelled".into();
                row.stop_requested = true;
            }
            compact(&mut row);
            self.save(key, &row)?;
            log_transition(Some(&before), &row, "stop");
            Self::install(&mut rows, key, row);
        }
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
        if let Some(input) = self.stored(&key, &id)? {
            return Ok(input);
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
            runtime_state: String::new(),
            runtime_error: None,
        };
        self.save(&key, &input)?;
        log_transition(None, &input, "admission");
        Self::install(&mut rows, &key, input.clone());
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
        let mut input = rows
            .get(key)
            .and_then(|entries| entries.iter().find(|row| row.id == id))
            .cloned();
        if input.is_none() {
            input = self.stored(key, id)?;
        }
        let mut input = input.ok_or("SDK 队列项目不存在")?;
        let before = input.clone();
        mutate(&mut input)?;
        compact(&mut input);
        self.save(key, &input)?;
        log_transition(Some(&before), &input, "update");
        Self::install(&mut rows, key, input.clone());
        self.publish(key, &rows);
        Ok(input)
    }

    pub fn first(&self, key: &str) -> Option<Input> {
        self.rows
            .lock()
            .ok()?
            .get(key)?
            .iter()
            .find(|row| {
                row.operation == "turn" && (!terminal(row) || !row.runtime_state.is_empty())
            })
            .cloned()
    }

    pub fn runnable(&self) -> Vec<(String, Value)> {
        self.rows
            .lock()
            .expect("SDK input journal")
            .iter()
            .filter_map(|(key, rows)| {
                let row = rows.iter().find(|row| {
                    row.operation == "turn" && (!terminal(row) || !row.runtime_state.is_empty())
                })?;
                (row.state == "queued" || !row.runtime_state.is_empty())
                    .then(|| (key.clone(), row.target.clone()))
            })
            .collect()
    }

    fn visible_rows(rows: &Document, key: &str) -> Value {
        json!(rows.get(key).into_iter().flatten().filter(|row|!terminal(row)).map(|row|json!({
            "id":row.id,"origin":"bridge","operation":row.operation,"request_id":row.request_id,"delivery_id":row.delivery_id,
            "state":if row.state=="recovering"{"accepted"}else{&row.state},"text":row.draft["text"],"error":row.error,"consumption":row.consumption,
            "edit_text":row.input.as_ref().map(|input|input["text"].clone()).unwrap_or_else(||row.draft["text"].clone()),
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
