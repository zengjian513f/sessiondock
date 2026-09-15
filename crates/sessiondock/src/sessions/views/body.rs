//! Byte rendering of message batches: the `/api/messages` document and the
//! history pages with `messages` spliced from the view's retained
//! serialization (docs/read-model.md "视图字节缓存"). The document is
//! byte-for-byte what `serde_json::to_vec` of the `Value` renderer produces:
//! every cacheable message is that message's exact `to_vec` output, and the
//! fields around the array are serialized by serde_json from the same
//! `json!` values.

use serde_json::{Value, json};

use super::{Selected, Selection, SessionError, Slot, ViewSnapshot, media_projection, pages};
use crate::{files::FileService, media::MediaStore, sessions::PageStore};

/// One `/api/messages` (or SSE) document, complete except for its closing
/// brace so the handler can append the per-request `prompt` field.
pub struct MessageBody {
    open: Vec<u8>,
    end: u64,
    head: String,
    anchor: String,
    /// Non-status positions of the messages in the document, for the
    /// Claude prompt check (`bridge::live`) that scans the batch's messages.
    positions: Vec<usize>,
}

impl MessageBody {
    /// The complete document, with `"prompt": <prompt>` appended when given.
    pub fn finish(mut self, prompt: Option<&Value>) -> Vec<u8> {
        if let Some(prompt) = prompt {
            self.open.extend_from_slice(b",\"prompt\":");
            serde_json::to_writer(&mut self.open, prompt).expect("serde_json::Value serializes");
        }
        self.open.push(b'}');
        self.open
    }
    /// The finished document's size without a `prompt` field.
    pub fn size(&self) -> usize {
        self.open.len() + 1
    }
    pub fn end(&self) -> u64 {
        self.end
    }
    pub fn head(&self) -> &str {
        &self.head
    }
    pub fn anchor(&self) -> &str {
        &self.anchor
    }
    #[cfg(test)]
    pub fn message_count(&self) -> usize {
        self.positions.len()
    }
    /// Whether one of the batch's messages answers Claude tool call
    /// `tool_id` (an `answer`/`tool_result` with that `call_id`).
    pub fn answers(&self, snapshot: &ViewSnapshot, tool_id: &str) -> bool {
        if tool_id.is_empty() {
            return false;
        }
        let mut events = snapshot
            .view
            .events()
            .filter(|event| event.message["role"] != "status");
        let mut position = 0;
        for &wanted in &self.positions {
            let Some(event) = events.nth(wanted - position) else {
                return false;
            };
            position = wanted + 1;
            let message = &event.message;
            if message["call_id"].as_str() == Some(tool_id)
                && matches!(message["role"].as_str(), Some("answer" | "tool_result"))
            {
                return true;
            }
        }
        false
    }
}

/// Render one selected batch (`select_batch`) as bytes.
pub(super) fn message_body(
    snapshot: &ViewSnapshot,
    selection: Selection<'_>,
    media: &MediaStore,
    files: Option<&FileService>,
    pages: &PageStore,
) -> Result<MessageBody, SessionError> {
    let (head, tail) = super::batch_fields(snapshot, &selection);
    let mut open = Vec::new();
    serde_json::to_writer(&mut open, &head).map_err(serialize_error)?;
    // `head` is a non-empty object: drop its brace, continue the object.
    debug_assert_eq!(open.last(), Some(&b'}'));
    open.pop();
    open.extend_from_slice(b",\"messages\":");
    write_messages(
        snapshot,
        &selection.selected,
        media,
        files,
        pages,
        &mut open,
    )?;
    for (key, value) in tail.as_object().expect("object above") {
        open.push(b',');
        serde_json::to_writer(&mut open, key).map_err(serialize_error)?;
        open.push(b':');
        serde_json::to_writer(&mut open, value).map_err(serialize_error)?;
    }
    let parsed = &snapshot.view.parsed;
    Ok(MessageBody {
        open,
        end: parsed.committed as u64,
        head: parsed.head(parsed.committed),
        anchor: snapshot.anchor.clone(),
        positions: selection
            .selected
            .iter()
            .map(|selected| selected.index)
            .collect(),
    })
}

fn serialize_error(_: serde_json::Error) -> SessionError {
    SessionError::new(500, "消息序列化失败")
}

/// Write `[m0,m1,…]` for `selected`. Runs of consecutive positions inside
/// one event list whose projection is a pure function of the event are
/// copied from the retained bytes in one piece; typed-media and discovered-
/// reference messages are projected per request exactly as the `Value`
/// renderer does (descriptor registration, `media_more` grants, file
/// tokens), and lists without retained bytes serialize straight from the
/// event, without a clone.
pub(crate) fn write_messages(
    snapshot: &ViewSnapshot,
    selected: &[Selected<'_>],
    media: &MediaStore,
    files: Option<&FileService>,
    pages: &PageStore,
    out: &mut Vec<u8>,
) -> Result<(), SessionError> {
    let view = &snapshot.view;
    // Per-request projection for the messages that need it, in one batch
    // (one descriptor registration, like the whole-batch projection).
    let special = selected
        .iter()
        .enumerate()
        .filter(|(_, selected)| {
            view.entry(selected.index, selected.event)
                .is_none_or(|entry| entry.special)
        })
        .map(|(index, selected)| (index, selected.event))
        .collect::<Vec<_>>();
    let projected = media_projection::project(
        snapshot,
        &special.iter().map(|(_, event)| *event).collect::<Vec<_>>(),
        media,
        files,
    )?;
    let mut projected = special
        .iter()
        .map(|(index, _)| *index)
        .zip(projected)
        .peekable();
    out.push(b'[');
    let mut run: Option<(Slot, Slot)> = None;
    let mut first = true;
    let flush = |run: &mut Option<(Slot, Slot)>, out: &mut Vec<u8>| {
        if let Some((from, to)) = run.take() {
            let bytes = match (from, to) {
                (Slot::Inherited(a), Slot::Inherited(b)) => {
                    view.inherited_encoded.messages(a..b + 1)
                }
                (Slot::Leaf(a), Slot::Leaf(b)) => view.parsed.encoded.messages(a..b + 1),
                _ => None,
            };
            out.extend_from_slice(bytes.expect("runs cover retained contiguous slots"));
        }
    };
    for (index, item) in selected.iter().enumerate() {
        let slot = view.slot(item.index);
        let special = projected.peek().is_some_and(|(next, _)| *next == index);
        // Retained bytes are used only for the very event the position
        // maps to (`View::entry` proves identity); anything else is
        // serialized from the event itself.
        let retained = view.entry(item.index, item.event).is_some()
            && match slot {
                Slot::Inherited(local) => view.inherited_encoded.message(local).is_some(),
                Slot::Leaf(local) => view.parsed.encoded.message(local).is_some(),
                Slot::Rename => false,
            };
        if !special && retained {
            match run {
                Some((from, Slot::Inherited(last))) if slot == Slot::Inherited(last + 1) => {
                    run = Some((from, slot));
                }
                Some((from, Slot::Leaf(last))) if slot == Slot::Leaf(last + 1) => {
                    run = Some((from, slot));
                }
                Some(_) => {
                    flush(&mut run, out);
                    out.push(b',');
                    run = Some((slot, slot));
                }
                None => {
                    if !first {
                        out.push(b',');
                    }
                    run = Some((slot, slot));
                }
            }
            first = false;
            continue;
        }
        flush(&mut run, out);
        if !first {
            out.push(b',');
        }
        first = false;
        if special {
            let (_, images) = projected.next().expect("peeked");
            let mut message = item.event.message.clone();
            if !images.is_empty() {
                message["media"] = json!(images);
            }
            if let Some(more) = pages::media_more(snapshot, item, Some(pages))? {
                message["media_more"] = more;
            }
            serde_json::to_writer(&mut *out, &message).map_err(serialize_error)?;
        } else {
            serde_json::to_writer(&mut *out, &item.event.message).map_err(serialize_error)?;
        }
    }
    flush(&mut run, out);
    out.push(b']');
    Ok(())
}
