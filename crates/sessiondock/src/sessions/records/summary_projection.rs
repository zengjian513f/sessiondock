//! JSONL fields consumed by index/summary, including shared provider predicates.
//! This projection is private to list summaries. Native history, search text,
//! stop-evidence scans and the independent latest-model/turn scan keep their
//! existing decoders. Summary sidecars also retain their existing decoder.
use serde_json::Value;

use super::scanner::{self, Projection, ScanError};
use Projection::{All, Fields};

const PAYLOAD: Projection = Fields(&[
    ("type", All),
    ("role", All),
    ("name", All),
    ("error", All),
    ("model", All),
    // Content stays complete: title envelopes classify native image shapes,
    // and flatten_text must still report malformed text/content fields.
    ("content", All),
    ("internal_chat_message_metadata_passthrough", All),
    // session_meta identity, inheritance and subagent ownership/labels.
    // source/history_base are also exposed to downstream index consumers.
    ("id", All),
    ("session_id", All),
    ("timestamp", All),
    ("cwd", All),
    ("thread_source", All),
    ("source", All),
    ("history_base", All),
    ("forked_from_id", All),
    ("parent_thread_id", All),
    ("agent_path", All),
    ("agent_nickname", All),
    ("agent_role", All),
]);

const ROOT: &[(&str, Projection)] = &[
    ("type", All),
    ("ordinal", All),
    ("timestamp", All),
    ("ts", All),
    ("sessionId", All),
    ("cwd", All),
    ("gitBranch", All),
    ("customTitle", All),
    ("aiTitle", All),
    ("continuedInSessionId", All),
    ("isSidechain", All),
    ("isMeta", All),
    ("isCompactSummary", All),
    ("isApiErrorMessage", All),
    ("interruptedMessageId", All),
    ("subtype", All),
    // The shared compact predicate only tests whether this is an object.
    ("compactMetadata", Fields(&[])),
    (
        "message",
        Fields(&[("content", All), ("model", All), ("stop_reason", All)]),
    ),
    ("payload", PAYLOAD),
    ("attachment", Fields(&[("type", All)])),
    (
        "toolUseResult",
        Fields(&[("taskId", All), ("backgroundTaskId", All), ("task_id", All)]),
    ),
    // Grok shape validation, Claude queue notices and Agy title/skip rules.
    ("content", All),
    ("media", All),
    // OpenCode's fallback user title.
    ("data", Fields(&[("text", All)])),
];

pub(in crate::sessions) fn decode(line: &[u8]) -> Result<Value, ScanError> {
    // Keep the established serde-first path for ordinary small records. This
    // threshold only selects a decoder; larger records remain fully readable.
    if line.len() <= super::SPAN_THRESHOLD {
        return super::decode_record(line);
    }
    let value = scanner::scan_projected(line, Fields(ROOT))?;
    // session_meta uses payload truthiness to choose the first metadata.
    // An object containing only unrecognized fields is still truthy. Re-read
    // that rare shape in full rather than inventing a sentinel field or losing
    // its effect on identity selection. The scanner still checks all bytes.
    if value["type"] == "session_meta"
        && value["payload"]
            .as_object()
            .is_some_and(|payload| payload.is_empty())
    {
        return scanner::scan_projected(line, All);
    }
    Ok(value)
}
