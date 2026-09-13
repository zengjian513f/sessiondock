//! Window selection precedes file opens; authority uses the complete branch.
use super::{Event, SessionError, ViewSnapshot};
use crate::{
    files::{FileError, FileService},
    media::{self, MediaStore, PreparedImage},
};
use serde_json::{Value, json};
use std::ops::Range;

/// Typed images projected inline per message. Further images of the same
/// message are reachable only through `project_range` (media pages).
pub(super) const DISPLAY_LIMIT: usize = 16;

/// Inline projection: the first `DISPLAY_LIMIT` typed images of every selected
/// event, followed by discovered text/file references as before.
pub(super) fn project(
    snapshot: &ViewSnapshot,
    selected: &[&Event],
    media: &MediaStore,
    files: Option<&FileService>,
) -> Result<Vec<Vec<Value>>, SessionError> {
    let ranges = selected
        .iter()
        .map(|event| (*event, 0..event.media.len().min(DISPLAY_LIMIT)))
        .collect::<Vec<_>>();
    project_ranges(snapshot, &ranges, media, files, true)
}

/// One sub-range of a single event's typed images, registered through the same
/// descriptor path as the inline projection. Text discovery is not repeated:
/// those items already accompanied the message itself.
pub(super) fn project_range(
    snapshot: &ViewSnapshot,
    event: &Event,
    range: Range<usize>,
    media: &MediaStore,
    files: Option<&FileService>,
) -> Result<Vec<Value>, SessionError> {
    if range.start > range.end || range.end > event.media.len() {
        return Err(SessionError::new(409, "图片分页范围已变化，请重新载入会话"));
    }
    let mut output = project_ranges(snapshot, &[(event, range)], media, files, false)?;
    Ok(output.pop().unwrap_or_default())
}

fn project_ranges(
    snapshot: &ViewSnapshot,
    selected: &[(&Event, Range<usize>)],
    media: &MediaStore,
    files: Option<&FileService>,
    discover: bool,
) -> Result<Vec<Vec<Value>>, SessionError> {
    let native_count = selected.iter().map(|(_, range)| range.len()).sum::<usize>();
    if native_count > media::MAX_ITEMS {
        return Err(SessionError::new(413, "单次消息投影超过 256 张图片上限"));
    }
    let mut discovery_remaining = media::MAX_ITEMS;
    let discovered = selected
        .iter()
        .map(|(event, _)| {
            if !discover || discovery_remaining == 0 {
                return Vec::new();
            }
            let mut items = media::discover(&event.message);
            items.truncate(discovery_remaining);
            discovery_remaining -= items.len();
            items
        })
        .collect::<Vec<_>>();
    let needs_files = selected.iter().any(|(event, range)| {
        event.media[range.clone()]
            .iter()
            .any(|image| image.file_ref().is_some())
    }) || discovered.iter().any(|items| !items.is_empty());
    let scope = if needs_files {
        Some(
            files
                .ok_or_else(|| {
                    FileError::new(
                        501,
                        "media_files_disabled",
                        "未配置图片读取目录；不会自动访问磁盘",
                    )
                })
                .and_then(|files| snapshot.media_scope(files)),
        )
    } else {
        None
    };
    let mut output = selected
        .iter()
        .map(|(_, range)| vec![Value::Null; range.len()])
        .collect::<Vec<_>>();
    let mut prepared = Vec::new();
    let mut positions = Vec::new();
    // Register private sources only. Decoded-byte admission belongs to GET;
    // optional file descriptors must not crowd out embedded source retention.
    for (index, (event, range)) in selected.iter().enumerate() {
        for (position, image) in event.media[range.clone()].iter().enumerate() {
            if image.file_ref().is_some() {
                continue;
            }
            let image = if image.native_span().is_some() {
                PreparedImage::native_span(
                    image,
                    snapshot.view.meta["uid"].as_str().unwrap_or(""),
                    snapshot.view.meta["agent_id"].as_str().unwrap_or(""),
                )
            } else {
                PreparedImage::embedded(image)
            }
            .map_err(session_error)?;
            positions.push((index, position, None));
            prepared.push(image);
        }
    }
    for (index, (event, range)) in selected.iter().enumerate() {
        let native =
            event.media[range.clone()]
                .iter()
                .enumerate()
                .filter_map(|(position, image)| {
                    image
                        .file_ref()
                        .map(|reference| (reference, None, Some(position)))
                });
        let text = discovered[index]
            .iter()
            .map(|image| (image.reference.as_str(), Some(image.gallery), None));
        for (reference, gallery, native_position) in native.chain(text) {
            let result = scope
                .as_ref()
                .expect("file request has scope result")
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|scope| PreparedImage::file(scope, reference))
                .and_then(|image| {
                    if prepared.len() >= media::MAX_ITEMS {
                        return Err(FileError::new(
                            413,
                            "media_budget",
                            "本次图片超过描述符数量预算；文字仍可查看",
                        ));
                    }
                    Ok(image)
                });
            let position = native_position.unwrap_or_else(|| {
                output[index].push(Value::Null);
                output[index].len() - 1
            });
            match result {
                Ok(image) => {
                    positions.push((
                        index,
                        position,
                        gallery.map(|gallery| (reference.to_owned(), gallery)),
                    ));
                    prepared.push(image);
                }
                // Python registers nothing for a text reference that does not
                // resolve to an image file: keep the text, project no
                // placeholder. Typed native references keep their error slot.
                Err(error) if native_position.is_none() && media::silent_failure(&error) => {
                    output[index].pop();
                }
                Err(error) => {
                    let mut value = media::failure(error.status, error.code, &error.message);
                    decorate(
                        &mut value,
                        gallery
                            .map(|gallery| (reference.to_owned(), gallery))
                            .as_ref(),
                    );
                    output[index][position] = value;
                }
            }
        }
    }
    let projected = match media.register_prepared(&prepared) {
        Ok(value) => value,
        Err(error @ (media::MediaError::Busy | media::MediaError::Limit))
            if prepared.iter().any(PreparedImage::is_file) =>
        {
            let mut fallback =
                vec![
                    media::failure(error.status(), "media_budget", &error.to_string());
                    prepared.len()
                ];
            let mut embedded = Vec::new();
            let mut indexes = Vec::new();
            for (index, image) in prepared.into_iter().enumerate() {
                if !image.is_file() {
                    indexes.push(index);
                    embedded.push(image);
                }
            }
            for (index, value) in indexes
                .into_iter()
                .zip(media.register_prepared(&embedded).map_err(session_error)?)
            {
                fallback[index] = value;
            }
            fallback
        }
        Err(error) => return Err(session_error(error)),
    };
    for ((index, position, decoration), mut value) in positions.into_iter().zip(projected) {
        decorate(&mut value, decoration.as_ref());
        output[index][position] = value;
    }
    Ok(output)
}
fn decorate(value: &mut Value, decoration: Option<&(String, bool)>) {
    if let Some((reference, gallery)) = decoration {
        value["ref"] = json!(reference);
        value["gallery"] = json!(gallery);
    }
}
fn session_error(error: media::MediaError) -> SessionError {
    SessionError::new(error.status(), error.to_string())
}
