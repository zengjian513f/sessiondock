//! 录制目录的只读读取器。
//!
//! 供另一进程在写入端仍在追加时，以及写入进程退出之后使用。负责找到最新
//! checkpoint，并从任意 [`Position`] 起按序读出其后的事件。
//!
//! 容忍尚未写完的尾帧、崩溃留下的截断/损坏尾部，以及已被淘汰（删除）的最旧
//! 分段。本模块只读：不修改、不删除任何文件；损坏的字节不会引发 panic。
//!
//! 每个分段文件整文件 [`std::fs::read`]（写入端保证分段有界）。帧解码只使用
//! [`crate::format::parse_segment_header`]、[`crate::format::decode_frame`]
//! （经由 [`crate::format::FrameIter`]）。

use std::io;
use std::path::Path;

use crate::format::{FrameIter, parse_segment_header};
use crate::{
    Frame, Position, SEGMENT_HEADER_LEN, SegmentHeader, SegmentInfo, parse_segment_file_name,
    segment_file_name,
};

/// 一次可回放的 checkpoint：它所在的位置、绝对时间与画面内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replay {
    /// checkpoint 帧的起始位置（段序号 + 段体偏移）。
    pub position: Position,
    /// 绝对 Unix 时间（毫秒）：所在段 `base_unix_ms + at_ms`。
    pub unix_ms: u64,
    /// 写入该 checkpoint 时的列数。
    pub cols: u16,
    /// 写入该 checkpoint 时的行数。
    pub rows: u16,
    /// 终端完整状态字节（历史行 + 当前屏 + 模式 + 光标）。
    pub state: Vec<u8>,
}

/// 读取器对外给出的一条事件（已去掉段内相对时间，改成绝对时间见 [`Stamped`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// 画面快照。只在需要重建画面时出现：本读的起点正指向段首 checkpoint，
    /// 或因缺口进入该段时的段首 checkpoint。非零偏移处的 checkpoint 从不出现。
    Checkpoint {
        /// 列数。
        cols: u16,
        /// 行数。
        rows: u16,
        /// 终端完整状态字节。
        state: Vec<u8>,
    },
    /// pty 原始输出字节。
    Output(Vec<u8>),
    /// 终端尺寸变化。
    Resize {
        /// 列数。
        cols: u16,
        /// 行数。
        rows: u16,
    },
    /// 任意 JSON 文本标记。
    Mark(String),
    /// 会话结束；载荷为 UTF-8 JSON。与其它事件一样返回，不提前结束读取。
    Exit(String),
}

/// 带绝对时间的事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamped {
    /// `segment.base_unix_ms + frame.at_ms`。
    pub unix_ms: u64,
    /// 事件本体。
    pub event: Event,
}

/// [`read_from`] 的一页结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read {
    /// 按发生顺序排列的事件。
    pub events: Vec<Stamped>,
    /// 第一个未返回的帧的位置。调用方下一次从这里继续。
    pub next: Position,
    /// 本页已计入的 Output / Checkpoint 载荷字节数（其它 kind 计 0）。
    pub payload_bytes: usize,
    /// 是否已经消费到最新分段的有效帧末尾（含最新段的不完整/损坏尾）。
    pub at_end: bool,
    /// 本页是否跳过了缺失分段、崩溃尾或损坏帧。
    pub gap: bool,
}

/// 列出目录中合法的分段文件，按段序号升序。
///
/// 只保留文件名能通过 [`parse_segment_file_name`] 解析、长度至少
/// [`SEGMENT_HEADER_LEN`] 且段头能解析的文件；其它文件忽略。
/// 目录不存在时返回 [`io::ErrorKind::NotFound`]。
pub fn list_segments(dir: &Path) -> io::Result<Vec<SegmentInfo>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        match entry.file_type() {
            Ok(file_type) if file_type.is_file() => {}
            Ok(_) => continue,
            Err(err) => return Err(err),
        }
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        let Some(index) = parse_segment_file_name(&name) else {
            continue;
        };
        let path = entry.path();
        let Some((header, bytes)) = load_segment(&path)? else {
            continue;
        };
        out.push(SegmentInfo {
            index,
            path,
            len: bytes.len() as u64,
            header,
        });
    }
    out.sort_by_key(|segment| segment.index);
    Ok(out)
}

/// 从最新分段往旧扫描，返回所遇的第一个含有 checkpoint 的分段里**最后**一个
/// 有效 [`Frame::Checkpoint`]。没有任何分段含有效 checkpoint 时返回 `None`。
pub fn latest_checkpoint(dir: &Path) -> io::Result<Option<Replay>> {
    let segments = list_segments(dir)?;
    for info in segments.iter().rev() {
        let Some((header, bytes)) = load_segment(&info.path)? else {
            continue;
        };
        let body = &bytes[SEGMENT_HEADER_LEN..];
        let mut iter = FrameIter::new(body);
        let mut last: Option<Replay> = None;
        loop {
            let offset = iter.offset() as u64;
            let Some(timed) = iter.next() else {
                break;
            };
            if let Frame::Checkpoint { cols, rows, state } = timed.frame {
                last = Some(Replay {
                    position: Position {
                        segment: info.index,
                        offset,
                    },
                    unix_ms: header.base_unix_ms.saturating_add(u64::from(timed.at_ms)),
                    cols,
                    rows,
                    state,
                });
            }
        }
        if last.is_some() {
            return Ok(last);
        }
    }
    Ok(None)
}

/// 最新分段有效帧的末尾位置（耗尽 [`FrameIter`] 后的 [`FrameIter::offset`]）。
/// 没有任何分段时返回 `None`。
pub fn newest_position(dir: &Path) -> io::Result<Option<Position>> {
    let segments = list_segments(dir)?;
    let Some(info) = segments.last() else {
        return Ok(None);
    };
    let Some((_, bytes)) = load_segment(&info.path)? else {
        return Ok(Some(Position {
            segment: info.index,
            offset: 0,
        }));
    };
    let body = &bytes[SEGMENT_HEADER_LEN..];
    let mut iter = FrameIter::new(body);
    while iter.next().is_some() {}
    Ok(Some(Position {
        segment: info.index,
        offset: iter.offset() as u64,
    }))
}

/// 从 `from` 起按序读取事件，可跨后续分段，直到累计载荷达到 `max_payload_bytes`
/// 或没有更多数据。
///
/// 使累计值跨过上限的那一条事件仍包含在本页中。只要还有可返回的事件，至少
/// 返回一条（即使 `max_payload_bytes` 为 0）。
///
/// Output 计入其字节数；Checkpoint 计入磁盘 payload 长度（`4 + state.len()`）；
/// Resize / Mark / Exit 计 0。
///
/// `from.segment` 不存在但有更大序号的分段时，从那个最小更大序号的段首开始，
/// 并设 `gap = true`。`from.segment` 不存在且没有更大序号时，返回空页：
/// `next = from`、`at_end = true`、`gap = false`。
///
/// `from.offset` 超出该段有效帧时视为该段已结束，不报错。
///
/// 最新段上不完整的尾帧会使本页在该帧起点停止（`at_end = true`），供调用方
/// 稍后轮询；若后面还有分段，则把该尾帧当作崩溃残留，跳到下一段并设 `gap`。
/// 损坏帧同样：有后续分段则跳过并设 `gap`，否则停在损坏帧起点且 `at_end`。
///
/// 段与段之间序号不连续时设 `gap`。段首（偏移 0）的 Checkpoint 仅在带着缺口
/// 进入该段、或 `from` 正指向它时作为事件返回；干净轮转时跳过，因为字节流
/// 在轮转处是连续的。非零偏移的 Checkpoint 一律跳过。
pub fn read_from(dir: &Path, from: Position, max_payload_bytes: usize) -> io::Result<Read> {
    let segments = list_segments(dir)?;
    let Some(start) = find_start(&segments, from) else {
        return Ok(Read {
            events: Vec::new(),
            next: from,
            payload_bytes: 0,
            at_end: true,
            gap: false,
        });
    };

    let mut vec_index = start.vec_index;
    let mut offset = start.offset;
    let mut gap = start.gap;
    let mut entered_with_gap = start.gap;
    let mut events: Vec<Stamped> = Vec::new();
    let mut payload_bytes: usize = 0;
    let mut next = Position {
        segment: segments[vec_index].index,
        offset,
    };

    loop {
        let is_last_listed = vec_index + 1 >= segments.len();
        let index = segments[vec_index].index;
        let path = segments[vec_index].path.clone();

        let loaded = load_segment(&path)?;
        let Some((header, bytes)) = loaded else {
            match next_segment(&segments, vec_index, index, true) {
                Some((next_vec, next_entered_gap)) => {
                    gap = true;
                    entered_with_gap = next_entered_gap;
                    vec_index = next_vec;
                    offset = 0;
                    next = Position {
                        segment: segments[vec_index].index,
                        offset: 0,
                    };
                    continue;
                }
                None => {
                    return Ok(Read {
                        events,
                        next,
                        payload_bytes,
                        at_end: true,
                        gap,
                    });
                }
            }
        };

        let body = &bytes[SEGMENT_HEADER_LEN..];
        let start_in_body = match usize::try_from(offset) {
            Ok(start) if start <= body.len() => start,
            _ => {
                next = Position {
                    segment: index,
                    offset: body.len() as u64,
                };
                match continue_after_clean_end(&segments, vec_index, index, &mut gap) {
                    Some((next_vec, next_entered_gap)) => {
                        entered_with_gap = next_entered_gap;
                        vec_index = next_vec;
                        offset = 0;
                        next = Position {
                            segment: segments[vec_index].index,
                            offset: 0,
                        };
                        continue;
                    }
                    None => {
                        return Ok(Read {
                            events,
                            next,
                            payload_bytes,
                            at_end: true,
                            gap,
                        });
                    }
                }
            }
        };

        let mut iter = FrameIter::new(&body[start_in_body..]);
        loop {
            let body_offset = start_in_body + iter.offset();
            next = Position {
                segment: index,
                offset: body_offset as u64,
            };

            let Some(timed) = iter.next() else {
                if iter.stop().is_some() {
                    match next_segment(&segments, vec_index, index, true) {
                        Some((next_vec, next_entered_gap)) => {
                            gap = true;
                            entered_with_gap = next_entered_gap;
                            vec_index = next_vec;
                            offset = 0;
                            break;
                        }
                        None => {
                            return Ok(Read {
                                events,
                                next,
                                payload_bytes,
                                at_end: true,
                                gap,
                            });
                        }
                    }
                }
                if iter.truncated() {
                    if is_last_listed {
                        return Ok(Read {
                            events,
                            next,
                            payload_bytes,
                            at_end: true,
                            gap,
                        });
                    }
                    match next_segment(&segments, vec_index, index, true) {
                        Some((next_vec, next_entered_gap)) => {
                            gap = true;
                            entered_with_gap = next_entered_gap;
                            vec_index = next_vec;
                            offset = 0;
                            break;
                        }
                        None => {
                            return Ok(Read {
                                events,
                                next,
                                payload_bytes,
                                at_end: true,
                                gap,
                            });
                        }
                    }
                }
                match continue_after_clean_end(&segments, vec_index, index, &mut gap) {
                    Some((next_vec, next_entered_gap)) => {
                        entered_with_gap = next_entered_gap;
                        vec_index = next_vec;
                        offset = 0;
                        break;
                    }
                    None => {
                        return Ok(Read {
                            events,
                            next,
                            payload_bytes,
                            at_end: true,
                            gap,
                        });
                    }
                }
            };

            let after_offset = start_in_body + iter.offset();
            let emit = should_emit_frame(&timed.frame, body_offset, entered_with_gap, from, index);
            if !emit {
                continue;
            }

            let event = event_from_frame(timed.frame);
            let added = event_payload_bytes(&event);
            events.push(Stamped {
                unix_ms: header.base_unix_ms.saturating_add(u64::from(timed.at_ms)),
                event,
            });
            payload_bytes = payload_bytes.saturating_add(added);
            next = Position {
                segment: index,
                offset: after_offset as u64,
            };

            if payload_bytes >= max_payload_bytes {
                let at_end = if is_last_listed {
                    !has_emitable_rest(&mut iter)
                } else {
                    false
                };
                return Ok(Read {
                    events,
                    next,
                    payload_bytes,
                    at_end,
                    gap,
                });
            }
        }
    }
}

/// 先取 [`latest_checkpoint`]，再从该 checkpoint **之后**的帧开始 [`read_from`]，
/// 避免把 checkpoint 既放在 [`Replay`] 里又作为事件重复发出。没有 checkpoint
/// 时返回 `None`。
pub fn replay(dir: &Path, max_payload_bytes: usize) -> io::Result<Option<(Replay, Read)>> {
    let Some(checkpoint) = latest_checkpoint(dir)? else {
        return Ok(None);
    };
    let after = position_after_checkpoint(dir, &checkpoint)?;
    let read = read_from(dir, after, max_payload_bytes)?;
    Ok(Some((checkpoint, read)))
}

/// 整文件读入一个分段。文件缺失、过短或段头无法解析时返回 `Ok(None)`。
fn load_segment(path: &Path) -> io::Result<Option<(SegmentHeader, Vec<u8>)>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if bytes.len() < SEGMENT_HEADER_LEN {
        return Ok(None);
    }
    match parse_segment_header(&bytes) {
        Ok(header) => Ok(Some((header, bytes))),
        Err(_) => Ok(None),
    }
}

struct Start {
    vec_index: usize,
    offset: u64,
    gap: bool,
}

/// 确定 `from` 落在已列出分段中的何处。没有任何可开始的分段时返回 `None`。
fn find_start(segments: &[SegmentInfo], from: Position) -> Option<Start> {
    if segments.is_empty() {
        return None;
    }
    if let Some(vec_index) = segments.iter().position(|s| s.index == from.segment) {
        return Some(Start {
            vec_index,
            offset: from.offset,
            gap: false,
        });
    }
    if let Some(vec_index) = segments.iter().position(|s| s.index > from.segment) {
        return Some(Start {
            vec_index,
            offset: 0,
            gap: true,
        });
    }
    None
}

/// 列出的下一段。`force_gap` 为真时（损坏、截断尾、文件消失）进入下一段算缺口。
fn next_segment(
    segments: &[SegmentInfo],
    vec_index: usize,
    current_index: u32,
    force_gap: bool,
) -> Option<(usize, bool)> {
    let next_vec = vec_index + 1;
    if next_vec >= segments.len() {
        return None;
    }
    let entered_with_gap = force_gap || segments[next_vec].index != current_index.saturating_add(1);
    Some((next_vec, entered_with_gap))
}

/// 当前段干净耗尽后是否继续到下一段；序号不连续则把 `gap` 置真。
fn continue_after_clean_end(
    segments: &[SegmentInfo],
    vec_index: usize,
    current_index: u32,
    gap: &mut bool,
) -> Option<(usize, bool)> {
    let (next_vec, entered_with_gap) = next_segment(segments, vec_index, current_index, false)?;
    if entered_with_gap {
        *gap = true;
    }
    Some((next_vec, entered_with_gap))
}

fn event_from_frame(frame: Frame) -> Event {
    match frame {
        Frame::Output(bytes) => Event::Output(bytes),
        Frame::Resize { cols, rows } => Event::Resize { cols, rows },
        Frame::Checkpoint { cols, rows, state } => Event::Checkpoint { cols, rows, state },
        Frame::Mark(text) => Event::Mark(text),
        Frame::Exit(text) => Event::Exit(text),
    }
}

fn event_payload_bytes(event: &Event) -> usize {
    match event {
        Event::Output(bytes) => bytes.len(),
        Event::Checkpoint { state, .. } => 4 + state.len(),
        Event::Resize { .. } | Event::Mark(_) | Event::Exit(_) => 0,
    }
}

/// 段首 checkpoint 仅在带着缺口进入该段，或 `from` 正指向它时发出；
/// 非零偏移的 checkpoint 一律跳过。
fn should_emit_frame(
    frame: &Frame,
    body_offset: usize,
    entered_with_gap: bool,
    from: Position,
    segment_index: u32,
) -> bool {
    match frame {
        Frame::Checkpoint { .. } if body_offset == 0 => {
            entered_with_gap || (from.segment == segment_index && from.offset == 0)
        }
        Frame::Checkpoint { .. } => false,
        _ => true,
    }
}

/// 当前段剩余有效帧里是否还有会被发出的事件（非 checkpoint）。
fn has_emitable_rest(iter: &mut FrameIter<'_>) -> bool {
    for timed in iter.by_ref() {
        if !matches!(timed.frame, Frame::Checkpoint { .. }) {
            return true;
        }
    }
    false
}

/// checkpoint 帧之后的位置，供 [`replay`] 从下一条事件开始读。
pub fn position_after_checkpoint(dir: &Path, checkpoint: &Replay) -> io::Result<Position> {
    let path = dir.join(segment_file_name(checkpoint.position.segment));
    let Some((_, bytes)) = load_segment(&path)? else {
        return Ok(checkpoint.position);
    };
    let body = &bytes[SEGMENT_HEADER_LEN..];
    let start = match usize::try_from(checkpoint.position.offset) {
        Ok(start) if start <= body.len() => start,
        _ => return Ok(checkpoint.position),
    };
    let mut iter = FrameIter::new(&body[start..]);
    match iter.next() {
        Some(_) => Ok(Position {
            segment: checkpoint.position.segment,
            offset: (start + iter.offset()) as u64,
        }),
        None => Ok(checkpoint.position),
    }
}

/// 不晚于 `unix_ms` 的最近一个有效 checkpoint（用于按时间 seek）。
/// 只看 `base_unix_ms <= unix_ms` 的分段，从新到旧；段内取最后一个
/// `base + at_ms <= unix_ms` 的 checkpoint。没有则 None。
pub fn checkpoint_before(dir: &Path, unix_ms: u64) -> io::Result<Option<Replay>> {
    let segments = list_segments(dir)?;
    for info in segments.iter().rev() {
        if info.header.base_unix_ms > unix_ms {
            continue;
        }
        let Some((header, bytes)) = load_segment(&info.path)? else {
            continue;
        };
        let body = &bytes[SEGMENT_HEADER_LEN..];
        let mut iter = FrameIter::new(body);
        let mut last: Option<Replay> = None;
        loop {
            let offset = iter.offset() as u64;
            let Some(timed) = iter.next() else {
                break;
            };
            let at = header.base_unix_ms.saturating_add(u64::from(timed.at_ms));
            if at > unix_ms {
                break;
            }
            if let Frame::Checkpoint { cols, rows, state } = timed.frame {
                last = Some(Replay {
                    position: Position {
                        segment: info.index,
                        offset,
                    },
                    unix_ms: at,
                    cols,
                    rows,
                    state,
                });
            }
        }
        if last.is_some() {
            return Ok(last);
        }
    }
    Ok(None)
}

/// 录制的时间范围：最旧分段的 `base_unix_ms` 到最新有效帧的时间。没有分段时 None。
pub fn bounds(dir: &Path) -> io::Result<Option<(u64, u64)>> {
    let segments = list_segments(dir)?;
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else {
        return Ok(None);
    };
    let start = first.header.base_unix_ms;
    let mut end = last.header.base_unix_ms;
    if let Some((header, bytes)) = load_segment(&last.path)? {
        let body = &bytes[SEGMENT_HEADER_LEN..];
        for timed in FrameIter::new(body) {
            end = end.max(header.base_unix_ms.saturating_add(u64::from(timed.at_ms)));
        }
    }
    Ok(Some((start, end.max(start))))
}
