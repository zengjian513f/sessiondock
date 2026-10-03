//! External native media and persisted-output files, checked in place.
//! Tool arguments and ordinary message strings are not filesystem references.
use super::{Operation, TransferError, TransferService};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

// Claude's background-task output is process scratch data, not a transcript
// dependency. Keep historical pointers as text even after the task log expires.
fn claude_task_output(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "output")
        && path
            .parent()
            .is_some_and(|dir| dir.file_name().is_some_and(|name| name == "tasks"))
        && path.components().any(|part| {
            part.as_os_str().to_str().is_some_and(|name| {
                name.strip_prefix("claude-")
                    .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()))
            })
        })
}

fn image(value: &Value, paths: &mut BTreeSet<String>) {
    let typed = matches!(
        value["type"].as_str(),
        Some("image" | "input_image" | "image_url")
    ) || (value["type"] == "file"
        && ["media_type", "mime_type", "mimeType"].iter().any(|k| {
            value[*k]
                .as_str()
                .or_else(|| value["file"][*k].as_str())
                .is_some_and(|s| s.starts_with("image/"))
        }));
    if typed
        && let Ok(Some(image)) = crate::media::NativeImage::from_block(value)
        && let Some(path) = image.file_ref()
    {
        paths.insert(path.into());
    }
}
fn output(value: &Value, paths: &mut BTreeSet<String>) {
    if let Some(text) = value.as_str() {
        if text.starts_with("Full output saved to:") || text.contains("<persisted-output>") {
            for line in text.lines() {
                if let Some(path) = line.trim().strip_prefix("Full output saved to:") {
                    let path = path
                        .split("</persisted-output>")
                        .next()
                        .unwrap_or("")
                        .trim();
                    if !path.is_empty() {
                        paths.insert(path.into());
                    }
                }
            }
        }
    } else if let Some(values) = value.as_array() {
        for value in values {
            output(value, paths);
        }
    } else if value.is_object() {
        for key in ["outputFile", "output_file", "transcriptPath"] {
            if let Some(path) = value[key].as_str().filter(|s| !s.is_empty()) {
                paths.insert(path.into());
            }
        }
        for key in ["content", "text", "stdout", "stderr"] {
            output(&value[key], paths);
        }
    }
}
fn references(value: &Value, paths: &mut BTreeSet<String>) {
    if let Some(values) = value.as_array() {
        for value in values {
            references(value, paths);
        }
    } else if value.is_object() {
        image(value, paths);
        if let Some(result) = value.get("toolUseResult") {
            output(result, paths);
        }
        if matches!(
            value["type"].as_str(),
            Some("tool_result" | "function_call_output")
        ) {
            output(&value["content"], paths);
            output(&value["output"], paths);
        }
        // Known record/content envelopes only: never recurse into tool input,
        // configuration, or JSON-looking text supplied by a user.
        for key in [
            "payload",
            "message",
            "content",
            "output",
            "params",
            "update",
            "rawOutput",
        ] {
            if value[key].is_object() || value[key].is_array() {
                references(&value[key], paths);
            }
        }
    }
}

pub(super) fn collect(
    service: &TransferService,
    op: &Operation,
) -> Result<BTreeMap<PathBuf, BTreeSet<PathBuf>>, TransferError> {
    let publications = service.publications(op);
    let bundled: BTreeSet<_> = publications.iter().map(|f| f.target.clone()).collect();
    let mut owned_directories = Vec::new();
    if let Some(plan) = &op.file_plan {
        for member in &plan.group.members {
            if member.source == "grok" {
                owned_directories.push(plan.member_target(member)?);
            } else if member.source == "claude" && !member.agent {
                owned_directories.push(plan.member_target(member)?.with_extension(""));
                if let Some(id) = op.mapped_session("claude", &member.sid) {
                    owned_directories.push(plan.roots["claude"].join("file-history").join(id));
                }
            }
        }
    }
    let mut dependencies: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    for file in &publications {
        let native = op
            .file_plan
            .as_ref()
            .and_then(|p| p.files.iter().find(|f| f.source == file.source));
        let member = op
            .group()
            .members
            .iter()
            .find(|m| m.path == file.source)
            .or_else(|| native.and_then(|f| op.group().members.iter().find(|m| m.uid == f.owner)))
            .or_else(|| {
                op.plan
                    .files
                    .iter()
                    .find(|f| f.source == file.source)
                    .and_then(|f| {
                        op.group()
                            .members
                            .iter()
                            .find(|m| m.source == "codex" && m.sid == f.thread)
                    })
            });
        let Some(member) = member else {
            continue;
        };
        let cwd = PathBuf::from(&member.cwd);
        let mut paths = BTreeSet::new();
        if file.symlink {
            let link = fs::read_link(&file.staging)?;
            let resolved = if link.is_absolute() {
                link
            } else {
                file.target.parent().unwrap().join(link)
            };
            paths.insert(resolved.to_string_lossy().into_owned());
        } else {
            let format = native.map(|f| f.format.as_str()).unwrap_or("jsonl");
            if !matches!(format, "json" | "jsonl") {
                continue;
            }
            let raw = fs::read(&file.staging)?;
            if format == "json" {
                references(&serde_json::from_slice::<Value>(&raw)?, &mut paths);
            } else {
                for line in raw
                    .split(|b| *b == b'\n')
                    .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
                {
                    references(&serde_json::from_slice::<Value>(line)?, &mut paths);
                }
            }
        }
        for path in paths {
            let path = Path::new(&path);
            let path = if path.is_absolute() {
                path.to_owned()
            } else {
                cwd.join(path)
            };
            if crate::transfer::environment::workspace_upload(&path) {
                continue;
            }
            if member.source == "claude" && claude_task_output(&path) {
                continue;
            }
            let bundled_directory = owned_directories.iter().any(|root| path.starts_with(root))
                && bundled.iter().any(|file| file.starts_with(&path));
            if !bundled.contains(&path) && !bundled_directory {
                dependencies.entry(cwd.clone()).or_default().insert(path);
            }
        }
    }
    Ok(dependencies)
}
