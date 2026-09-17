//! ptyhost 会话录制（record）：每个会话一个目录，若干只追加的分段文件。
//!
//! 事实源是 pty 的原始输出字节；resize、checkpoint、退出等事件按发生顺序穿插其中。
//! 宿主（ptyhost）只写；Web 服务（sessiondock）只读，宿主进程退出后仍可读。
//! 这个 crate 不依赖任何外部库，不含线程、socket 或协议代码。
//!
//! 磁盘布局：
//!
//! ```text
//! <record dir>/00000001.seg
//! <record dir>/00000002.seg
//! ...
//! ```
//!
//! 分段文件 = 16 字节段头 + 若干帧。每个新分段的第一帧必须是 [`Frame::Checkpoint`]，
//! 因此淘汰最旧的分段后，剩下最旧分段仍能独立重建画面。
//!
//! 段头（大端）：`magic "SDRC"(4) | version u16(2) | flags u16(2) | base_unix_ms u64(8)`。
//!
//! 帧（大端）：`kind u8(1) | len u32(4) | at_ms u32(4) | payload[len] | crc32 u32(4)`，
//! `at_ms` 是相对该段 `base_unix_ms` 的毫秒；crc32（IEEE，与 zlib 相同多项式）覆盖
//! `kind..payload` 这段连续字节。

pub mod format;
pub mod reader;
pub mod sanitize;
pub mod store;

/// 段头魔数。
pub const MAGIC: [u8; 4] = *b"SDRC";
/// 当前格式版本。
pub const VERSION: u16 = 1;
/// 段头字节数。
pub const SEGMENT_HEADER_LEN: usize = 16;
/// 帧头（kind + len + at_ms）字节数。
pub const FRAME_HEADER_LEN: usize = 9;
/// 帧尾 crc 字节数。
pub const FRAME_TRAILER_LEN: usize = 4;
/// 单帧载荷上限；超过即视为损坏。
pub const MAX_PAYLOAD: usize = 16 << 20;

pub const KIND_OUTPUT: u8 = 1;
pub const KIND_RESIZE: u8 = 2;
pub const KIND_CHECKPOINT: u8 = 3;
pub const KIND_MARK: u8 = 4;
pub const KIND_EXIT: u8 = 5;

/// 一条录制事件（不含时间）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// pty 原始输出字节，已由宿主剥离 DSR 查询之外的内容原样保留。
    Output(Vec<u8>),
    /// 终端尺寸变化；payload 为 `cols u16 | rows u16`（大端）。
    Resize { cols: u16, rows: u16 },
    /// 画面快照：写这一帧时终端的完整状态（历史行 + 当前屏 + 模式 + 光标），
    /// 与 ptyhost attach 回放字节同构。payload 为 `cols u16 | rows u16 | state[..]`。
    Checkpoint {
        cols: u16,
        rows: u16,
        state: Vec<u8>,
    },
    /// 任意 JSON 文本标记（attach/detach、注释等）。payload 为 UTF-8 JSON。
    Mark(String),
    /// 会话结束；payload 为 UTF-8 JSON（与宿主 exit 帧同构）。
    Exit(String),
}

impl Frame {
    pub fn kind(&self) -> u8 {
        match self {
            Self::Output(_) => KIND_OUTPUT,
            Self::Resize { .. } => KIND_RESIZE,
            Self::Checkpoint { .. } => KIND_CHECKPOINT,
            Self::Mark(_) => KIND_MARK,
            Self::Exit(_) => KIND_EXIT,
        }
    }
}

/// 带段内相对时间的帧。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timed {
    /// 相对所在段 `base_unix_ms` 的毫秒数。
    pub at_ms: u32,
    pub frame: Frame,
}

/// 已解析的段头。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentHeader {
    pub version: u16,
    pub flags: u16,
    /// 段起始的 Unix 时间（毫秒）。
    pub base_unix_ms: u64,
}

/// 录制目录中的一个位置：段序号 + 段内字节偏移（帧起始处，段头之后）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Position {
    pub segment: u32,
    pub offset: u64,
}

/// 目录中一个分段文件的概况。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentInfo {
    pub index: u32,
    pub path: std::path::PathBuf,
    /// 文件总长度（含段头）。
    pub len: u64,
    pub header: SegmentHeader,
}

/// 格式层错误：只描述字节是否合法，不涉及 IO。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    BadMagic,
    UnsupportedVersion(u16),
    UnknownKind(u8),
    PayloadTooLarge(usize),
    BadCrc,
    /// 载荷内容与 kind 不符（例如 Resize 不是 4 字节、Mark 不是 UTF-8）。
    BadPayload(&'static str),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "bad segment magic"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported record version {v}"),
            Self::UnknownKind(k) => write!(f, "unknown frame kind {k}"),
            Self::PayloadTooLarge(n) => write!(f, "frame payload too large: {n}"),
            Self::BadCrc => write!(f, "frame crc mismatch"),
            Self::BadPayload(why) => write!(f, "bad frame payload: {why}"),
        }
    }
}

impl std::error::Error for FormatError {}

/// 段文件名：`{index:08}.seg`。
pub fn segment_file_name(index: u32) -> String {
    format!("{index:08}.seg")
}

/// 从文件名解析段序号；非本格式文件名返回 `None`。
pub fn parse_segment_file_name(name: &str) -> Option<u32> {
    let stem = name.strip_suffix(".seg")?;
    if stem.len() != 8 || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    stem.parse().ok()
}
