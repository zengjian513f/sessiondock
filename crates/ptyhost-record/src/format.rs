//! 录制分段的字节级编解码：段头与帧。
//!
//! 段头与帧的磁盘布局见 crate 根模块文档。本模块只判断字节是否合法，不涉及 IO。
//! 多出来的尾部字节在解析段头或单帧时被忽略；[`FrameIter`] 则从段体起点逐帧前进。

use crate::{
    FRAME_HEADER_LEN, FRAME_TRAILER_LEN, FormatError, Frame, KIND_CHECKPOINT, KIND_EXIT, KIND_MARK,
    KIND_OUTPUT, KIND_RESIZE, MAGIC, MAX_PAYLOAD, SEGMENT_HEADER_LEN, SegmentHeader, Timed,
    VERSION,
};

/// IEEE 802.3 / zlib CRC-32 的 reflected 多项式。
const CRC32_POLY: u32 = 0xEDB8_8320;

/// 256 项查找表，编译期生成。
const CRC32_TABLE: [u32; 256] = make_crc32_table();

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ CRC32_POLY;
            } else {
                crc >>= 1;
            }
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// IEEE 802.3 CRC-32（与 zlib `crc32` 相同：reflected 多项式 `0xEDB88320`，
/// 初值 `0xFFFFFFFF`，最终异或 `0xFFFFFFFF`）。
///
/// `crc32(b"123456789")` 等于 `0xCBF43926`。
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF;
    for &byte in bytes {
        let index = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = CRC32_TABLE[index] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// 写入 16 字节段头：magic、[`VERSION`]、flags `0`、`base_unix_ms`，均为大端。
pub fn write_segment_header(base_unix_ms: u64) -> [u8; SEGMENT_HEADER_LEN] {
    let mut bytes = [0u8; SEGMENT_HEADER_LEN];
    bytes[0..4].copy_from_slice(&MAGIC);
    bytes[4..6].copy_from_slice(&VERSION.to_be_bytes());
    bytes[6..8].copy_from_slice(&0u16.to_be_bytes());
    bytes[8..16].copy_from_slice(&base_unix_ms.to_be_bytes());
    bytes
}

/// 解析段头。短于 [`SEGMENT_HEADER_LEN`] 或 magic 不符时返回 [`FormatError::BadMagic`]；
/// 版本不是 [`VERSION`] 时返回 [`FormatError::UnsupportedVersion`]。多余尾部字节忽略。
pub fn parse_segment_header(bytes: &[u8]) -> Result<SegmentHeader, FormatError> {
    if bytes.len() < SEGMENT_HEADER_LEN {
        return Err(FormatError::BadMagic);
    }
    if bytes[0..4] != MAGIC {
        return Err(FormatError::BadMagic);
    }
    let version = u16::from_be_bytes([bytes[4], bytes[5]]);
    if version != VERSION {
        return Err(FormatError::UnsupportedVersion(version));
    }
    let flags = u16::from_be_bytes([bytes[6], bytes[7]]);
    let base_unix_ms = u64::from_be_bytes([
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ]);
    Ok(SegmentHeader {
        version,
        flags,
        base_unix_ms,
    })
}

/// [`encode_frame`] 将追加的总字节数（帧头 + 载荷 + crc）。
pub fn encoded_len(frame: &Frame) -> usize {
    FRAME_HEADER_LEN + payload_len(frame) + FRAME_TRAILER_LEN
}

/// 将一帧追加到 `out`：`kind u8 | len u32 BE | at_ms u32 BE | payload | crc32 u32 BE`。
///
/// crc32 覆盖从 `kind` 到 payload 末尾的连续字节（不含 crc 自身）。
pub fn encode_frame(at_ms: u32, frame: &Frame, out: &mut Vec<u8>) {
    let start = out.len();
    out.push(frame.kind());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&at_ms.to_be_bytes());
    write_payload(frame, out);
    let payload_bytes = out.len() - start - FRAME_HEADER_LEN;
    let len = u32::try_from(payload_bytes).expect("frame payload fits in u32");
    out[start + 1..start + 5].copy_from_slice(&len.to_be_bytes());
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// 一帧解码结果：完整帧，或输入尚不完整。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    /// 成功解码一帧；`consumed` 为该帧占用的总字节数（头 + 载荷 + crc）。
    Frame {
        /// 带段内相对时间的帧。
        timed: Timed,
        /// 本帧占用的字节数。
        consumed: usize,
    },
    /// 输入不足以构成完整帧（且长度字段尚未表明损坏）。
    Incomplete,
}

/// 从 `bytes[0]` 起解码一帧。检查顺序：
///
/// 1. 短于 [`FRAME_HEADER_LEN`] → [`Decoded::Incomplete`]
/// 2. 未知 kind → [`FormatError::UnknownKind`]
/// 3. `len > MAX_PAYLOAD` → [`FormatError::PayloadTooLarge`]（即使缓冲不足也报告，
///    以免损坏的长度被当成不完整帧）
/// 4. 不够容纳 payload + trailer → [`Decoded::Incomplete`]
/// 5. crc 不符 → [`FormatError::BadCrc`]
/// 6. 载荷与 kind 不符 → [`FormatError::BadPayload`]
///
/// 成功时 `consumed` 为整帧长度；`bytes` 中多出来的尾部忽略。
pub fn decode_frame(bytes: &[u8]) -> Result<Decoded, FormatError> {
    if bytes.len() < FRAME_HEADER_LEN {
        return Ok(Decoded::Incomplete);
    }

    let kind = bytes[0];
    if !is_known_kind(kind) {
        return Err(FormatError::UnknownKind(kind));
    }

    let len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
    let at_ms = u32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);

    if len > MAX_PAYLOAD {
        return Err(FormatError::PayloadTooLarge(len));
    }

    let total = FRAME_HEADER_LEN + len + FRAME_TRAILER_LEN;
    if bytes.len() < total {
        return Ok(Decoded::Incomplete);
    }

    let covered_end = FRAME_HEADER_LEN + len;
    let expected_crc = crc32(&bytes[..covered_end]);
    let got_crc = u32::from_be_bytes([
        bytes[covered_end],
        bytes[covered_end + 1],
        bytes[covered_end + 2],
        bytes[covered_end + 3],
    ]);
    if expected_crc != got_crc {
        return Err(FormatError::BadCrc);
    }

    let payload = &bytes[FRAME_HEADER_LEN..covered_end];
    let frame = frame_from_payload(kind, payload)?;
    Ok(Decoded::Frame {
        timed: Timed { at_ms, frame },
        consumed: total,
    })
}

/// 段体（16 字节段头之后）上的帧迭代器。
///
/// 遇到第一帧 [`Decoded::Incomplete`] 或解码错误时停止，此后一直返回 `None`，
/// [`Self::offset`] 停在该帧起点。
#[derive(Debug)]
pub struct FrameIter<'a> {
    body: &'a [u8],
    offset: usize,
    stop: Option<FormatError>,
    done: bool,
}

impl<'a> FrameIter<'a> {
    /// 从段体字节构造迭代器。
    pub fn new(body: &'a [u8]) -> Self {
        Self {
            body,
            offset: 0,
            stop: None,
            done: false,
        }
    }

    /// 已消费字节数，即下一帧（或使迭代停止的那一帧）的起始偏移。
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// 因损坏帧而停止时返回该错误；干净结束或尾部不完整时为 `None`。
    pub fn stop(&self) -> Option<&FormatError> {
        self.stop.as_ref()
    }

    /// 体中 [`Self::offset`] 之后仍有字节（因损坏或不完整而提前停止）。
    pub fn truncated(&self) -> bool {
        self.offset < self.body.len()
    }
}

impl Iterator for FrameIter<'_> {
    type Item = Timed;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let rest = &self.body[self.offset..];
        match decode_frame(rest) {
            Ok(Decoded::Frame { timed, consumed }) => {
                self.offset += consumed;
                Some(timed)
            }
            Ok(Decoded::Incomplete) => {
                self.done = true;
                None
            }
            Err(err) => {
                self.stop = Some(err);
                self.done = true;
                None
            }
        }
    }
}

fn is_known_kind(kind: u8) -> bool {
    matches!(
        kind,
        KIND_OUTPUT | KIND_RESIZE | KIND_CHECKPOINT | KIND_MARK | KIND_EXIT
    )
}

fn payload_len(frame: &Frame) -> usize {
    match frame {
        Frame::Output(bytes) => bytes.len(),
        Frame::Resize { .. } => 4,
        Frame::Checkpoint { state, .. } => 4 + state.len(),
        Frame::Mark(text) | Frame::Exit(text) => text.len(),
    }
}

fn write_payload(frame: &Frame, out: &mut Vec<u8>) {
    match frame {
        Frame::Output(bytes) => out.extend_from_slice(bytes),
        Frame::Resize { cols, rows } => {
            out.extend_from_slice(&cols.to_be_bytes());
            out.extend_from_slice(&rows.to_be_bytes());
        }
        Frame::Checkpoint { cols, rows, state } => {
            out.extend_from_slice(&cols.to_be_bytes());
            out.extend_from_slice(&rows.to_be_bytes());
            out.extend_from_slice(state);
        }
        Frame::Mark(text) | Frame::Exit(text) => out.extend_from_slice(text.as_bytes()),
    }
}

fn frame_from_payload(kind: u8, payload: &[u8]) -> Result<Frame, FormatError> {
    match kind {
        KIND_OUTPUT => Ok(Frame::Output(payload.to_vec())),
        KIND_RESIZE => {
            if payload.len() != 4 {
                return Err(FormatError::BadPayload(
                    "resize payload must be exactly 4 bytes",
                ));
            }
            let cols = u16::from_be_bytes([payload[0], payload[1]]);
            let rows = u16::from_be_bytes([payload[2], payload[3]]);
            Ok(Frame::Resize { cols, rows })
        }
        KIND_CHECKPOINT => {
            if payload.len() < 4 {
                return Err(FormatError::BadPayload(
                    "checkpoint payload must be at least 4 bytes",
                ));
            }
            let cols = u16::from_be_bytes([payload[0], payload[1]]);
            let rows = u16::from_be_bytes([payload[2], payload[3]]);
            let state = payload[4..].to_vec();
            Ok(Frame::Checkpoint { cols, rows, state })
        }
        KIND_MARK => {
            let text = std::str::from_utf8(payload)
                .map_err(|_| FormatError::BadPayload("mark payload is not valid UTF-8"))?;
            Ok(Frame::Mark(text.to_owned()))
        }
        KIND_EXIT => {
            let text = std::str::from_utf8(payload)
                .map_err(|_| FormatError::BadPayload("exit payload is not valid UTF-8"))?;
            Ok(Frame::Exit(text.to_owned()))
        }
        other => Err(FormatError::UnknownKind(other)),
    }
}
