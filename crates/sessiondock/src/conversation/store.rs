//! Session-owned drafts and one-shot submission identities. No CLI acknowledgment queue.
use crate::delivery::executor::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

type Result<T> = std::result::Result<T, Failure>;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Document {
    drafts: BTreeMap<String, Draft>,
    requests: BTreeMap<String, Submission>,
    uploads: BTreeMap<String, Upload>,
    legacy: BTreeMap<String, Value>,
    aliases: BTreeMap<String, String>,
    reports: BTreeMap<String, Value>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Draft {
    pub revision: u64,
    pub value: Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Submission {
    pub key: String,
    pub id: String,
    pub payload: Value,
    #[serde(default)]
    pub attachments: Vec<String>,
    pub phase: String,
    pub result: Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Upload {
    pub key: String,
    pub id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub published: Option<Value>,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub created_at: u64,
}
pub struct Store {
    directory: PathBuf,
    state: Mutex<Document>,
}
pub fn fingerprint(value: &Value) -> Value {
    use sha2::{Digest, Sha256};
    json!(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("JSON value serialization"))
    ))
}
// Pending editor markers identify the original submission; the persisted
// receipt hash must verify them before removing any content, including at boot.
fn completed_editor_value(
    requests: &BTreeMap<String, Submission>,
    key: &str,
    value: &Value,
) -> Option<Value> {
    let id = value["requestId"].as_str()?;
    let row = requests.get(&format!("{key}\0{id}"))?;
    if row.phase != "sent" {
        return None;
    }
    let normalize = |body: &Value| {
        json!({"text":body["text"],
        "attachments":body["attachments"].as_array().into_iter().flatten().map(|a|json!({"upload_id":a["upload_id"],"number":a["number"]})).collect::<Vec<_>>(),
        "quotes":body["quotes"].as_array().cloned().unwrap_or_default()})
    };
    let mut candidates = Vec::new();
    if let Some(body) = value["requestText"]
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
    {
        candidates.push(normalize(&body));
    }
    let attachments = row
        .attachments
        .iter()
        .map(|id| {
            let a = value["attachments"]
                .as_array()?
                .iter()
                .find(|a| a["uploaded"]["upload_id"] == id.as_str())?;
            Some(json!({"upload_id":id,"number":a["number"]}))
        })
        .collect::<Option<Vec<_>>>();
    if let Some(attachments) = attachments {
        candidates.push(json!({"text":value["text"],"attachments":attachments,"quotes":value["quotes"].as_array().cloned().unwrap_or_default()}));
        if id.starts_with("report-send:") && value["report_text"].is_string() {
            candidates.push(json!({"text":value["report_text"],"attachments":attachments,"quotes":value["quotes"].as_array().cloned().unwrap_or_default()}));
            candidates
                .push(json!({"text":value["report_text"],"attachments":attachments,"quotes":[]}));
        }
    }
    let submitted = candidates
        .into_iter()
        .find(|body| fingerprint(body) == row.payload)?;
    let mut next = value.clone();
    if next["text"] == submitted["text"] {
        next["text"] = json!("");
    }
    if let Some(items) = next.get_mut("attachments").and_then(Value::as_array_mut) {
        items.retain(|a| {
            !submitted["attachments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|sent| {
                    a["uploaded"]["upload_id"] == sent["upload_id"] && a["number"] == sent["number"]
                })
        });
    }
    if let Some(items) = next.get_mut("quotes").and_then(Value::as_array_mut) {
        items.retain(|q| !submitted["quotes"].as_array().unwrap().contains(q));
    }
    for field in ["requestId", "requestText", "report_prompt", "report_text"] {
        next.as_object_mut()?.remove(field);
    }
    if next["text"].as_str().unwrap_or("").is_empty()
        && next["attachments"].as_array().is_none_or(Vec::is_empty)
        && next["quotes"].as_array().is_none_or(Vec::is_empty)
    {
        next["nextAttachmentNumber"] = json!(1);
    }
    Some(next)
}
fn io(error: std::io::Error) -> Failure {
    Failure::new(
        503,
        "conversation_storage",
        format!("会话保存失败：{error}"),
    )
}
impl Store {
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory).map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(io)?;
        }
        let directory = directory.canonicalize().map_err(io)?;
        let path = directory.join("conversation-ledger.json");
        let mut state: Document = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
                .ok_or_else(|| {
                    Failure::new(503, "conversation_storage", "会话记录无法读取，保留原文件")
                })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Document::default(),
            Err(e) => return Err(io(e)),
        };
        for request in state.requests.values_mut() {
            if request.payload.is_object() {
                request.attachments = request.payload["attachments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a["upload_id"].as_str().map(str::to_owned))
                    .collect();
                request.payload = fingerprint(&request.payload);
            }
        }
        let repair = state
            .drafts
            .iter()
            .any(|(key, d)| completed_editor_value(&state.requests, key, &d.value).is_some());
        let store = Self {
            directory,
            state: Mutex::new(state),
        };
        if repair {
            store.update(|doc| {
                for (key, draft) in &mut doc.drafts {
                    if let Some(value) = completed_editor_value(&doc.requests, key, &draft.value) {
                        draft.value = value;
                        draft.revision += 1;
                    }
                }
                Ok(())
            })?;
        }
        Ok(store)
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    fn update<T>(&self, work: impl FnOnce(&mut Document) -> Result<T>) -> Result<T> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = state.clone();
        let result = work(&mut next)?;
        let bytes = serde_json::to_vec(&next)
            .map_err(|_| Failure::new(503, "conversation_storage", "会话记录无法序列化"))?;
        let temporary = self
            .directory
            .join(format!(".conversation-{}.tmp", super::random_id()?));
        let installed = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.directory.join("conversation-ledger.json"))?;
            // Once installed, later writes must use this document even if
            // directory sync fails and its acknowledgment is lost.
            *state = next.clone();
            #[cfg(unix)]
            fs::File::open(&self.directory)?.sync_all()?;
            Ok::<_, std::io::Error>(())
        })();
        if let Err(error) = installed {
            let _ = fs::remove_file(&temporary);
            return Err(io(error));
        }
        *state = next;
        Ok(result)
    }
    pub fn draft(&self, key: &str) -> Draft {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drafts
            .get(key)
            .cloned()
            .unwrap_or_default()
    }
    pub fn canonical(&self, key: &str) -> String {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .aliases
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.into())
    }
    pub fn link(&self, alias: &str, key: &str) -> Result<()> {
        if alias == key || self.canonical(alias) == key {
            return Ok(());
        }
        self.update(|doc| {
            if doc.aliases.get(alias).is_some_and(|old| old != key) {
                return Err(Failure::new(
                    409,
                    "session_identity",
                    "会话绑定冲突，保留原输入",
                ));
            }
            doc.aliases.insert(alias.into(), key.into());
            Ok(())
        })
    }
    pub fn save(&self, key: &str, expected: u64, value: Value) -> Result<Draft> {
        self.update(|doc| {
            let completed = completed_editor_value(&doc.requests, key, &value);
            let old = doc.drafts.entry(key.into()).or_default();
            if completed.as_ref().is_some_and(|v| {
                v["text"].as_str().unwrap_or("").is_empty()
                    && v["attachments"].as_array().is_none_or(Vec::is_empty)
                    && v["quotes"].as_array().is_none_or(Vec::is_empty)
            }) {
                // A complete sent replica is no edit. It must not replace a
                // later draft even if a stale page adopted the latest revision.
                return Ok(old.clone());
            }
            let value = completed.unwrap_or(value);
            if old.value == value {
                return Ok(old.clone());
            }
            if old.revision != expected {
                return Err(Failure::new(
                    409,
                    "draft_revision",
                    "另一页面已更新草稿，当前输入保留",
                ));
            }
            old.revision = old
                .revision
                .checked_add(1)
                .ok_or_else(|| Failure::new(503, "conversation_storage", "草稿版本无法更新"))?;
            old.value = value;
            Ok(old.clone())
        })
    }
    fn request_key(key: &str, id: &str) -> String {
        format!("{key}\0{id}")
    }
    pub fn request(&self, key: &str, id: &str) -> Option<Submission> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .requests
            .get(&Self::request_key(key, id))
            .cloned()
    }
    pub fn begin(&self, key: &str, id: &str, payload: Value) -> Result<Option<Submission>> {
        let attachments = payload["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["upload_id"].as_str().map(str::to_owned))
            .collect();
        let payload = fingerprint(&payload);
        self.update(|doc| {
            let name = Self::request_key(key, id);
            if let Some(old) = doc.requests.get(&name) {
                if old.payload != payload {
                    return Err(Failure::new(
                        400,
                        "request_conflict",
                        "相同提交 ID 对应了不同内容",
                    ));
                }
                return Ok(Some(old.clone()));
            }
            doc.requests.insert(
                name,
                Submission {
                    key: key.into(),
                    id: id.into(),
                    payload,
                    attachments,
                    phase: "writing".into(),
                    result: Value::Null,
                },
            );
            Ok(None)
        })
    }
    pub fn finish(
        &self,
        key: &str,
        id: &str,
        phase: &str,
        result: Value,
        revision: Option<u64>,
    ) -> Result<()> {
        self.update(|doc| {
            let row = doc
                .requests
                .get_mut(&Self::request_key(key, id))
                .ok_or_else(|| Failure::new(404, "submission_missing", "提交不存在"))?;
            row.phase = phase.into();
            row.result = result;
            if phase == "sent"
                && let Some(draft) = doc.drafts.get_mut(key)
            {
                if let Some(value) = completed_editor_value(&doc.requests, key, &draft.value) {
                    draft.value = value;
                    draft.revision += 1;
                } else if revision == Some(draft.revision) {
                    draft.revision += 1;
                    let session = draft.value["session"].clone();
                    draft.value = json!({"text":"","attachments":[],"quotes":[],"session":session});
                }
            }
            Ok(())
        })
    }
    pub fn upload(&self, key: &str, id: &str) -> Result<Upload> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .uploads
            .get(&Self::request_key(key, id))
            .cloned()
            .ok_or_else(|| Failure::new(404, "attachment_missing", "当前会话附件不存在"))
    }
    pub fn note_upload(&self, upload: Upload) -> Result<()> {
        self.update(|doc| {
            let key = Self::request_key(&upload.key, &upload.id);
            if let Some(old) = doc.uploads.get(&key) {
                if old.name != upload.name || old.size != upload.size || old.sha256 != upload.sha256
                {
                    return Err(Failure::new(400, "attachment_conflict", "附件上传 ID 冲突"));
                }
            } else {
                doc.uploads.insert(key, upload);
            }
            Ok(())
        })
    }
    pub fn published(&self, key: &str, id: &str, value: Value) -> Result<()> {
        self.update(|doc| {
            let row = doc
                .uploads
                .get_mut(&Self::request_key(key, id))
                .ok_or_else(|| Failure::new(404, "attachment_missing", "附件不存在"))?;
            row.published = Some(value);
            Ok(())
        })
    }
    pub fn import(&self, key: &str, value: Value) -> Result<()> {
        self.update(|doc| {
            let copies = doc.legacy.entry(key.into()).or_insert_with(|| json!([]));
            if !copies.is_array() {
                *copies = json!([copies.clone()]);
            }
            if let Some(items) = copies.as_array_mut()
                && !items.contains(&value)
            {
                items.push(value.clone());
            }
            // Restore only an empty server draft, never concatenate messages
            // from another page or restore the old sent-message archive.
            let old = doc.drafts.entry(key.into()).or_default();
            if old.value.is_null()
                && (!value["text"].as_str().unwrap_or("").is_empty()
                    || value["attachments"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty())
                    || value["quotes"].as_array().is_some_and(|a| !a.is_empty()))
            {
                let mut active = value;
                if let Some(map) = active.as_object_mut() {
                    map.remove("saved");
                }
                old.value = active;
                old.revision += 1;
            }
            // A verified legacy File can replace the missing backing of that
            // same attachment, without merging active messages or revisions.
            let snapshot = doc
                .legacy
                .get(key)
                .and_then(Value::as_array)
                .and_then(|copies| copies.last())
                .cloned();
            if let (Some(snapshot), Some(items)) = (
                snapshot,
                old.value
                    .get_mut("attachments")
                    .and_then(Value::as_array_mut),
            ) {
                for item in items {
                    if !item["uploaded"]["upload_id"].is_string()
                        && let Some(source) =
                            snapshot["attachments"].as_array().and_then(|sources| {
                                sources.iter().find(|source| {
                                    source["id"] == item["id"]
                                        && source["file"] == item["file"]
                                        && source["uploaded"]["upload_id"].is_string()
                                })
                            })
                    {
                        item["uploaded"] = source["uploaded"].clone();
                        old.revision += 1;
                    }
                }
            }
            Ok(())
        })
    }
    /// A staged upload still named by its draft, legacy evidence or an
    /// unfinished submission must survive cleanup and explicit discards.
    fn upload_referenced(doc: &Document, upload: &Upload) -> bool {
        doc.drafts.get(&upload.key).is_some_and(|draft| {
            draft.value["attachments"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|a| a["uploaded"]["upload_id"] == upload.id)
            })
        }) || doc
            .legacy
            .get(&upload.key)
            .and_then(Value::as_array)
            .is_some_and(|copies| {
                copies.iter().any(|copy| {
                    let attachments = copy["attachments"].as_array().into_iter().flatten();
                    let saved = copy["saved"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|row| row["attachments"].as_array().into_iter().flatten());
                    attachments
                        .chain(saved)
                        .any(|item| item["uploaded"]["upload_id"] == upload.id)
                })
            })
            || doc.requests.values().any(|request| {
                request.key == upload.key
                    && request.phase != "sent"
                    && request.attachments.contains(&upload.id)
            })
    }
    pub fn retire_unreferenced_uploads(&self, cutoff: u64) -> Result<Vec<Upload>> {
        self.update(|doc| {
            let retired: Vec<_> = doc
                .uploads
                .values()
                .filter(|u| {
                    u.published.is_none()
                        && u.created_at < cutoff
                        && !Self::upload_referenced(doc, u)
                })
                .cloned()
                .collect();
            for upload in &retired {
                doc.uploads
                    .remove(&Self::request_key(&upload.key, &upload.id));
            }
            Ok(retired)
        })
    }
    /// Forget a staged upload the editor removed. Published bytes belong to
    /// the session directory, and a draft or unfinished submission that still
    /// names the upload keeps it: the caller must save the draft first.
    pub fn discard_upload(&self, key: &str, id: &str) -> Result<bool> {
        self.update(|doc| {
            let name = Self::request_key(key, id);
            let Some(upload) = doc.uploads.get(&name) else {
                return Ok(false);
            };
            if upload.published.is_some() {
                return Err(Failure::new(
                    409,
                    "attachment_published",
                    "附件已发布到会话目录，不能丢弃",
                ));
            }
            if Self::upload_referenced(doc, upload) {
                return Err(Failure::new(
                    409,
                    "attachment_referenced",
                    "草稿或未完成的发送仍引用该附件",
                ));
            }
            doc.uploads.remove(&name);
            Ok(true)
        })
    }
    pub fn legacy(&self, key: &str) -> Value {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .legacy
            .get(key)
            .cloned()
            .unwrap_or_else(|| json!([]))
    }
    pub fn uploads(&self) -> Vec<Upload> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .uploads
            .values()
            .cloned()
            .collect()
    }
    pub fn report(&self, key: &str, id: &str) -> Option<Value> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .reports
            .get(&Self::request_key(key, id))
            .cloned()
    }
    pub fn note_report(&self, key: &str, id: &str, value: Value) -> Result<()> {
        self.update(|doc| {
            doc.reports.insert(Self::request_key(key, id), value);
            Ok(())
        })
    }
    pub fn refresh_report_worker(&self, key: &str, report_id: &str, worker: &Value) -> Result<()> {
        self.update(|doc| {
            let prefix = format!("{key}\0");
            for (id, row) in &mut doc.reports {
                if id.starts_with(&prefix) && row["result"]["report_id"] == report_id {
                    row["result"]["worker"] = worker.clone();
                }
            }
            Ok(())
        })
    }
    /// The logical draft key of a launch receipt and whether a native UID
    /// shares it: the receipt's own key, or the earlier receipt a restart
    /// chained it to; shared once an alias from a native UID (or the key
    /// itself) is not a `launch:` key.
    fn launch_draft(doc: &Document, record_id: &str) -> (String, bool) {
        let launch_key = format!("launch:{record_id}");
        let key = doc.aliases.get(&launch_key).cloned().unwrap_or(launch_key);
        let shared = !key.starts_with("launch:")
            || doc
                .aliases
                .iter()
                .any(|(alias, target)| *target == key && !alias.starts_with("launch:"));
        (key, shared)
    }
    /// Drop the input retained for a receipt the operator discarded, so
    /// `drafts` stops advertising the deleted session. A draft shared with a
    /// native UID stays: that session still shows it. Returns whether a draft
    /// was removed.
    pub fn forget_launch(&self, record_id: &str) -> Result<bool> {
        {
            let doc = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let (key, shared) = Self::launch_draft(&doc, record_id);
            if shared || !doc.drafts.contains_key(&key) {
                return Ok(false);
            }
        }
        self.update(|doc| {
            let (key, shared) = Self::launch_draft(doc, record_id);
            if shared {
                return Ok(false);
            }
            Ok(doc.drafts.remove(&key).is_some())
        })
    }
    pub fn drafts(&self) -> Vec<(String, Draft)> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drafts
            .iter()
            .filter(|(_, d)| {
                d.value["session"].is_object()
                    && (!d.value["text"].as_str().unwrap_or("").is_empty()
                        || d.value["attachments"]
                            .as_array()
                            .is_some_and(|a| !a.is_empty())
                        || d.value["quotes"].as_array().is_some_and(|a| !a.is_empty()))
            })
            .map(|(key, d)| (key.clone(), d.clone()))
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolate_reload_and_refuse_stale_edits() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let a = store.save("session-a", 0, json!({"text":"a"})).unwrap();
        store.save("session-b", 0, json!({"text":"b"})).unwrap();
        assert!(store.save("session-a", 0, json!({"text":"stale"})).is_err());
        store
            .begin("session-a", "send", json!({"text":"a"}))
            .unwrap();
        store
            .save("session-a", a.revision, json!({"text":"new edit"}))
            .unwrap();
        store
            .finish(
                "session-a",
                "send",
                "sent",
                json!({"ok":true}),
                Some(a.revision),
            )
            .unwrap();
        let reopened = Store::open(temp.path()).unwrap();
        assert_eq!(reopened.draft("session-a").value["text"], "new edit");
        assert_eq!(reopened.draft("session-b").value["text"], "b");
        assert_eq!(reopened.request("session-a", "send").unwrap().phase, "sent");
        assert!(reopened.request("session-b", "send").is_none());
        assert!(
            reopened
                .begin("session-a", "send", json!({"text":"different"}))
                .is_err()
        );
        assert!(
            reopened
                .begin("session-a", "send", json!({"text":"a"}))
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn sent_report_cannot_be_restored_and_late_edits_survive_repair() {
        let temp = tempfile::tempdir().unwrap();
        let s = Store::open(temp.path()).unwrap();
        let id = "report-send:known";
        let original = json!({"text":"report","report_text":"report","report_prompt":"diagnostics","requestId":id,
            "attachments":[{"id":"a","number":1,"uploaded":{"upload_id":"a"}}],"quotes":[],"session":{"uid":"tmux:owned"}});
        let draft = s.save("a", 0, original.clone()).unwrap();
        s.begin(
            "a",
            id,
            json!({"text":"report","attachments":[{"upload_id":"a","number":1}],"quotes":[]}),
        )
        .unwrap();
        let mut edited = original.clone();
        edited["text"] = json!("later edit");
        edited["attachments"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"b","number":2,"uploaded":{"upload_id":"b"}}));
        edited["quotes"] = json!([{"id":"q","text":"later quote"}]);
        s.save("a", draft.revision, edited.clone()).unwrap();
        s.finish("a", id, "sent", json!({"ok":true}), Some(draft.revision))
            .unwrap();
        let current = s.draft("a");
        assert_eq!(current.value["text"], "later edit");
        assert_eq!(current.value["attachments"].as_array().unwrap().len(), 1);
        assert_eq!(current.value["attachments"][0]["id"], "b");
        assert_eq!(current.value["quotes"][0]["text"], "later quote");
        assert!(current.value.get("requestId").is_none());
        let restored = s.save("a", current.revision, original.clone()).unwrap();
        assert_eq!(restored.value["text"], "later edit");
        assert_eq!(restored.value["attachments"][0]["id"], "b");
        // Simulate the old deployed code writing its pre-SEND snapshot back.
        s.update(|doc| {
            doc.drafts.get_mut("a").unwrap().value = edited;
            Ok(())
        })
        .unwrap();
        let reopened = Store::open(temp.path()).unwrap().draft("a");
        assert_eq!(reopened.value["text"], "later edit");
        assert_eq!(reopened.value["attachments"][0]["id"], "b");
        assert_eq!(reopened.value["quotes"][0]["text"], "later quote");
    }
    #[test]
    fn sent_snapshot_preserves_changed_quotes_and_removed_attachment() {
        let temp = tempfile::tempdir().unwrap();
        let s = Store::open(temp.path()).unwrap();
        let body = json!({"text":"send","attachments":[{"upload_id":"a","number":1}],"quotes":[{"id":"q","text":"original"}]});
        let original = json!({"text":"send","attachments":[{"id":"a","number":1,"uploaded":{"upload_id":"a"}}],
            "quotes":body["quotes"],"requestId":"send-id","requestText":serde_json::to_string(&body).unwrap()});
        let draft = s.save("a", 0, original.clone()).unwrap();
        s.begin("a", "send-id", body).unwrap();
        let mut edited = original.clone();
        edited["attachments"] = json!([]);
        edited["quotes"] = json!([{"id":"q","text":"changed"}]);
        s.save("a", draft.revision, edited).unwrap();
        s.finish(
            "a",
            "send-id",
            "sent",
            json!({"ok":true}),
            Some(draft.revision),
        )
        .unwrap();
        let current = s.draft("a");
        assert_eq!(current.value["text"], "");
        assert_eq!(current.value["quotes"][0]["text"], "changed");
        let mut unverified = original;
        unverified["text"] = json!("unverified input");
        unverified["requestText"] = json!("{}");
        let saved = s.save("a", current.revision, unverified).unwrap();
        assert_eq!(saved.value["text"], "unverified input");
    }
    #[test]
    fn clear_only_the_submitted_draft_and_keep_deduplication() {
        let temp = tempfile::tempdir().unwrap();
        let s = Store::open(temp.path()).unwrap();
        let d = s.save("a", 0, json!({"text":"send"})).unwrap();
        s.begin("a", "id", json!({"text":"send"})).unwrap();
        s.finish("a", "id", "sent", json!({"ok":true}), Some(d.revision))
            .unwrap();
        assert_eq!(s.draft("a").value["text"], "");
        assert!(
            s.begin("a", "id", json!({"text":"send"}))
                .unwrap()
                .is_some()
        );
    }
}

#[cfg(test)]
mod upload_tests {
    use super::*;
    #[test]
    fn same_upload_id_is_scoped_and_conflicting_bytes_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        for key in ["a", "b"] {
            store
                .note_upload(Upload {
                    key: key.into(),
                    id: "same".into(),
                    name: "x".into(),
                    mime: "text/plain".into(),
                    size: 1,
                    published: None,
                    sha256: key.into(),
                    created_at: 0,
                })
                .unwrap();
        }
        assert_eq!(store.upload("a", "same").unwrap().sha256, "a");
        assert_eq!(store.upload("b", "same").unwrap().sha256, "b");
        let mut conflict = store.upload("a", "same").unwrap();
        conflict.sha256 = "different".into();
        assert!(store.note_upload(conflict).is_err());
        assert_eq!(
            Store::open(temp.path())
                .unwrap()
                .upload("a", "same")
                .unwrap()
                .sha256,
            "a"
        );
    }
    #[test]
    fn discard_forgets_only_free_staged_uploads() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let upload = |id: &str, published: Option<Value>| Upload {
            key: "a".into(),
            id: id.into(),
            name: "x".into(),
            mime: "text/plain".into(),
            size: 1,
            published,
            sha256: id.into(),
            created_at: 0,
        };
        for id in ["free", "drafted", "sending"] {
            store.note_upload(upload(id, None)).unwrap();
        }
        store
            .note_upload(upload("published", Some(json!({"path":"/p"}))))
            .unwrap();
        store
            .save(
                "a",
                0,
                json!({"text":"","attachments":[{"id":"d","uploaded":{"upload_id":"drafted"}}],"quotes":[]}),
            )
            .unwrap();
        store
            .begin(
                "a",
                "req",
                json!({"text":"","attachments":[{"upload_id":"sending"}],"quotes":[]}),
            )
            .unwrap();
        assert!(store.discard_upload("a", "free").unwrap());
        assert!(!store.discard_upload("a", "free").unwrap());
        assert!(!store.discard_upload("other", "drafted").unwrap());
        assert_eq!(
            store.discard_upload("a", "drafted").unwrap_err().code,
            "attachment_referenced"
        );
        assert_eq!(
            store.discard_upload("a", "sending").unwrap_err().code,
            "attachment_referenced"
        );
        assert_eq!(
            store.discard_upload("a", "published").unwrap_err().code,
            "attachment_published"
        );
        // Saving the draft without the attachment releases it.
        let current = store.draft("a");
        store
            .save(
                "a",
                current.revision,
                json!({"text":"","attachments":[],"quotes":[]}),
            )
            .unwrap();
        assert!(store.discard_upload("a", "drafted").unwrap());
        let reopened = Store::open(temp.path()).unwrap();
        assert!(reopened.upload("a", "free").is_err());
        assert!(reopened.upload("a", "drafted").is_err());
        assert!(reopened.upload("a", "sending").is_ok());
        assert!(reopened.upload("a", "published").is_ok());
    }
    #[test]
    fn uploads_follow_the_canonical_draft_key_across_a_link() {
        // The HTTP layer keys drafts and staged uploads by the same resolved
        // identity, so an upload staged before a pending launch is bound to
        // its native session stays reachable through the alias afterwards.
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let launch = "launch:record";
        store
            .save(
                store.canonical(launch).as_str(),
                0,
                json!({"text":"typed early"}),
            )
            .unwrap();
        store
            .note_upload(Upload {
                key: store.canonical(launch),
                id: "early".into(),
                name: "x".into(),
                mime: "text/plain".into(),
                size: 1,
                published: None,
                sha256: "early".into(),
                created_at: 0,
            })
            .unwrap();
        // The native UID had no draft of its own: it becomes an alias of the
        // launch key, exactly as `Conversations::identity` links it.
        store.link("claude:native", launch).unwrap();
        let key = store.canonical("claude:native");
        assert_eq!(key, launch);
        assert_eq!(store.draft(&key).value["text"], "typed early");
        assert_eq!(store.upload(&key, "early").unwrap().id, "early");
        assert_eq!(store.canonical(launch), launch);
    }
    #[test]
    fn cleanup_retains_drafts_unknown_writes_and_published_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        for id in ["draft", "unknown", "orphan", "published"] {
            store
                .note_upload(Upload {
                    key: "a".into(),
                    id: id.into(),
                    name: "x".into(),
                    mime: "text/plain".into(),
                    size: 1,
                    published: None,
                    sha256: id.into(),
                    created_at: 0,
                })
                .unwrap();
        }
        store
            .save(
                "a",
                0,
                json!({"attachments":[{"uploaded":{"upload_id":"draft"}}]}),
            )
            .unwrap();
        store
            .begin(
                "a",
                "write",
                json!({"attachments":[{"upload_id":"unknown"}]}),
            )
            .unwrap();
        store
            .published("a", "published", json!({"path":"history-file"}))
            .unwrap();
        let retired = store.retire_unreferenced_uploads(10).unwrap();
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].id, "orphan");
        for id in ["draft", "unknown", "published"] {
            assert!(store.upload("a", id).is_ok());
        }
    }
    #[test]
    fn empty_legacy_import_does_not_create_null_attachment_fields() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        for value in [
            json!({"text":"","attachments":[],"quotes":[]}),
            json!({"legacy_queue":[{"text":"old evidence"}]}),
        ] {
            store.import("empty", value).unwrap();
            assert!(store.draft("empty").value.is_null());
            assert_eq!(store.draft("empty").revision, 0);
        }
        store
            .import(
                "empty",
                json!({"text":"","attachments":[],"quotes":[{"text":"quotation"}]}),
            )
            .unwrap();
        assert_eq!(store.draft("empty").value["quotes"][0]["text"], "quotation");
        assert_eq!(
            Store::open(temp.path())
                .unwrap()
                .legacy("empty")
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }
    #[test]
    fn legacy_archive_never_restores_successful_sends_or_stacks() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        store
            .import("a", json!({"text":"editing","saved":[{"text":"sent"}]}))
            .unwrap();
        assert_eq!(store.draft("a").value["text"], "editing");
        assert!(store.draft("a").value.get("saved").is_none());
        store.import("a", json!({"text":"other page"})).unwrap();
        assert_eq!(store.draft("a").value["text"], "editing");
        store.link("launch:worker", "a").unwrap();
        assert_eq!(store.canonical("launch:worker"), "a");
        assert!(store.link("launch:worker", "b").is_err());
    }
    #[test]
    fn discarded_launch_forgets_its_draft_but_never_a_native_session_draft() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let session = |name: &str| json!({"uid":format!("tmux:{name}"),"name":name});
        // A plain pending receipt: its retained input goes with the discard.
        store
            .save(
                "launch:plain",
                0,
                json!({"text":"rclone config","attachments":[],"quotes":[],"session":session("plain")}),
            )
            .unwrap();
        assert_eq!(store.drafts().len(), 1);
        assert!(store.forget_launch("plain").unwrap());
        assert!(store.drafts().is_empty());
        assert_eq!(store.draft("launch:plain").revision, 0);
        assert!(!store.forget_launch("plain").unwrap());
        // A restarted receipt chains to the first one; discarding the
        // restart drops the shared logical draft.
        store
            .save(
                "launch:first",
                0,
                json!({"text":"kept across restart","attachments":[],"quotes":[],"session":session("second")}),
            )
            .unwrap();
        store.link("launch:second", "launch:first").unwrap();
        assert!(store.forget_launch("second").unwrap());
        assert!(store.drafts().is_empty());
        // A launch bound to a native UID shares that session's draft, in
        // either alias direction: the native row still shows it.
        store
            .save(
                "claude:native",
                0,
                json!({"text":"native input","attachments":[],"quotes":[],"session":{"uid":"claude:native"}}),
            )
            .unwrap();
        store.link("launch:bound", "claude:native").unwrap();
        assert!(!store.forget_launch("bound").unwrap());
        store
            .save(
                "launch:origin",
                0,
                json!({"text":"origin input","attachments":[],"quotes":[],"session":{"uid":"claude:later"}}),
            )
            .unwrap();
        store.link("claude:later", "launch:origin").unwrap();
        assert!(!store.forget_launch("origin").unwrap());
        assert_eq!(store.drafts().len(), 2);
        assert_eq!(Store::open(temp.path()).unwrap().drafts().len(), 2);
    }
}
