//! Report workers use the same SDK creation and durable input paths as the page.
use crate::{
    bug_report::{Report, update_manifest},
    error::ApiError,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde_json::{Value, json};

pub(super) async fn launch(
    state: AppState,
    report: &Report,
    source: &str,
    draft_uid: &str,
    revision: Option<u64>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<Value, String> {
    let conversations = state.conversations.clone().ok_or("服务端草稿不可用")?;
    let identity = conversations
        .identity(draft_uid)
        .await
        .map_err(|e| e.message)?;
    let previous = conversations.store.draft(&identity.key);
    if revision.is_some_and(|r| r != previous.revision) {
        return Err("草稿已被另一页面修改；诊断已保存，未发送".into());
    }
    let body = json!({"source":source,"cwd":state.bug_report.as_ref().unwrap().service.repository(),
        "request_id":format!("bug-report-{}",report.report_id),"model":model,"effort":effort});
    let created = if source == "codex" {
        super::codex_sdk::create(
            State(state.clone()),
            None,
            Json(serde_json::from_value(body).map_err(|e| e.to_string())?),
        )
        .await
    } else {
        super::native_sdk::call(
            State(state.clone()),
            Path("create".into()),
            None,
            Json(body),
        )
        .await
    }
    .map_err(|e| e.message)?
    .0;
    let uid = created["uid"]
        .as_str()
        .ok_or("SDK 创建未返回会话标识")?
        .to_owned();
    conversations
        .store
        .link(&uid, &identity.key)
        .map_err(|e| e.message)?;
    let mut worker = created;
    worker["mode"] = json!("sdk");
    worker["source"] = json!(source);
    worker["kind"] = json!("bug-report");
    worker["report_id"] = json!(report.report_id);
    worker["title"] = json!(report.prompt.lines().next().unwrap_or("BUG:"));
    let request_id = format!("report-send:{}", report.report_id);
    let mut value = previous.value;
    if !value.is_object() {
        value = json!({});
    }
    value["report_text"] = value["text"].clone();
    value["text"] = json!(report.prompt);
    // Attachments and quotes are already frozen into the diagnostic prompt.
    value["attachments"] = json!([]);
    value["quotes"] = json!([]);
    value["session"] = worker.clone();
    value.as_object_mut().unwrap().remove("requestId");
    value.as_object_mut().unwrap().remove("requestText");
    value["sdkSubmission"] = json!({"id":request_id,"key":serde_json::to_string(&json!({
        "uid":uid,"agent":null,"text":report.prompt,"attachments":[],"quotes":[],"model":null,"effort":null
    })).map_err(|e| e.to_string())?});
    let draft = conversations
        .store
        .save(&identity.key, previous.revision, value)
        .map_err(|e| e.message)?;
    if source != "codex" {
        let _ = super::native_sdk::call(
            State(state.clone()),
            Path("open".into()),
            None,
            Json(json!({"uid":uid})),
        )
        .await
        .map_err(|e| e.message)?;
    }
    update_manifest(
        &report.path,
        json!({"status":"starting","worker":worker,"worker_source":source,"worker_mode":"sdk"}),
    )
    .map_err(|e| e.to_string())?;
    let report_id = report.report_id.clone();
    record(
        &state,
        "bug_report.worker_started",
        &report_id,
        &uid,
        json!({"source":source,"mode":"sdk"}),
    );
    let source = source.to_owned();
    let path = report.path.clone();
    let input = json!({"uid":uid,"request_id":request_id,"text":report.prompt,
        "draft_revision":draft.revision});
    tokio::spawn(async move {
        let result: Result<Value, ApiError> = async {
            if source == "codex" {
                let response = super::codex_sdk::turn(
                    State(state.clone()),
                    Json(serde_json::from_value(input.clone()).unwrap()),
                )
                .await?
                .0;
                conversations
                    .sdk_accepted(&uid, &request_id, &input, response.clone())
                    .await
                    .map_err(super::conversation::error)?;
                Ok(response)
            } else {
                Ok(super::native_sdk::call(
                    State(state.clone()),
                    Path("turn".into()),
                    None,
                    Json(input),
                )
                .await?
                .0)
            }
        }
        .await;
        let status = match result {
            Ok(submission)
                if submission["accepted"] == false || submission["raw"]["accepted"] == false =>
            {
                json!({"status":"failed","error":submission,"injection":{"basis":"SDK","draft_retained":true}})
            }
            Ok(submission) => {
                json!({"status":"submitted","submission":submission,"injection":{"basis":"SDK","origin":crate::bug_report::worker::PAGE}})
            }
            Err(error) => {
                json!({"status":"failed","error":error.message,"injection":{"basis":"SDK","draft_retained":true}})
            }
        };
        record(
            &state,
            "bug_report.worker_submission",
            &report_id,
            &uid,
            json!({"source":source,"mode":"sdk","status":status["status"],"error":status["error"]}),
        );
        let _ = update_manifest(&path, status);
    });
    Ok(worker)
}

fn record(state: &AppState, event: &'static str, report: &str, uid: &str, data: Value) {
    if let Some(context) = &state.bug_report {
        context.audit.record(crate::audit::query::ServerEvent {
            event,
            category: "bug-report",
            severity: "info",
            uid,
            trace_id: report,
            page_id: crate::bug_report::worker::PAGE,
            build: "",
            data,
        });
    }
}
