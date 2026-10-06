//! 宿主与客户端之间的本地协议，与 Python 参考实现逐字节兼容。
//!
//! 控制请求：一行 JSON 请求，一行 JSON 应答，一次连接一条请求。
//! attach 请求应答后连接进入帧模式：1 字节类型 + 4 字节大端长度 + 载荷。

use std::io::{self, Read, Write};

pub const FRAME_DATA: u8 = 1;
pub const FRAME_RESIZE: u8 = 2;
pub const FRAME_EXIT: u8 = 3;
pub const MAX_LINE: usize = 4 * 1024 * 1024;

pub fn pack_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// 从缓冲区尽量多地切出完整帧，保留残余字节。
pub fn read_frames(buffer: &mut Vec<u8>) -> Vec<(u8, Vec<u8>)> {
    let mut frames = Vec::new();
    let mut pos = 0usize;
    while buffer.len() - pos >= 5 {
        let kind = buffer[pos];
        let len = u32::from_be_bytes([
            buffer[pos + 1],
            buffer[pos + 2],
            buffer[pos + 3],
            buffer[pos + 4],
        ]) as usize;
        if buffer.len() - pos - 5 < len {
            break;
        }
        frames.push((kind, buffer[pos + 5..pos + 5 + len].to_vec()));
        pos += 5 + len;
    }
    if pos > 0 {
        buffer.drain(..pos);
    }
    frames
}

pub fn send_json<W: Write>(out: &mut W, value: &serde_json::Value) -> io::Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

/// 读到一行 JSON 为止；buffer 保留多读的字节（帧模式紧随其后）。
pub fn recv_json<R: Read>(reader: &mut R, buffer: &mut Vec<u8>) -> io::Result<serde_json::Value> {
    loop {
        if let Some(at) = buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=at).collect();
            let value: serde_json::Value = serde_json::from_slice(&line[..line.len() - 1])
                .map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("非法 JSON: {e}"))
                })?;
            if !value.is_object() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "请求必须是对象"));
            }
            return Ok(value);
        }
        if buffer.len() > MAX_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "请求过大"));
        }
        let mut chunk = [0u8; 65536];
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "连接已关闭"));
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
}

/// tmux 风格键名 → 字节序列。未知键名按字面文本发送，与参考实现一致。
pub fn key_bytes(key: &str, app_cursor: bool) -> Vec<u8> {
    let arrow = |c: char| -> Vec<u8> {
        let prefix: &str = if app_cursor { "\x1bO" } else { "\x1b[" };
        format!("{prefix}{c}").into_bytes()
    };
    match key {
        "Up" => return arrow('A'),
        "Down" => return arrow('B'),
        "Right" => return arrow('C'),
        "Left" => return arrow('D'),
        _ => {}
    }
    let named: &[(&str, &str)] = &[
        ("Enter", "\r"),
        ("Escape", "\x1b"),
        ("Tab", "\t"),
        ("BTab", "\x1b[Z"),
        ("BSpace", "\x7f"),
        ("Space", " "),
        ("DC", "\x1b[3~"),
        ("IC", "\x1b[2~"),
        ("Home", "\x1b[H"),
        ("End", "\x1b[F"),
        ("PPage", "\x1b[5~"),
        ("NPage", "\x1b[6~"),
        ("F1", "\x1bOP"),
        ("F2", "\x1bOQ"),
        ("F3", "\x1bOR"),
        ("F4", "\x1bOS"),
        ("F5", "\x1b[15~"),
        ("F6", "\x1b[17~"),
        ("F7", "\x1b[18~"),
        ("F8", "\x1b[19~"),
        ("F9", "\x1b[20~"),
        ("F10", "\x1b[21~"),
        ("F11", "\x1b[23~"),
        ("F12", "\x1b[24~"),
    ];
    if let Some((_, value)) = named.iter().find(|(name, _)| *name == key) {
        return value.as_bytes().to_vec();
    }
    let ctrl_special: &[(&str, u8)] = &[
        ("@", 0x00),
        ("[", 0x1b),
        ("\\", 0x1c),
        ("]", 0x1d),
        ("^", 0x1e),
        ("_", 0x1f),
        ("?", 0x7f),
        ("Space", 0x00),
    ];
    let rest = if let Some(r) = key.strip_prefix("C-") {
        Some(r)
    } else {
        key.strip_prefix('^')
    };
    if let Some(rest) = rest
        && !rest.is_empty()
    {
        if let Some((_, byte)) = ctrl_special.iter().find(|(name, _)| *name == rest) {
            return vec![*byte];
        }
        let mut chars = rest.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && c.is_ascii_alphabetic()
        {
            return vec![(c.to_ascii_lowercase() as u8) & 0x1f];
        }
    }
    if let Some(rest) = key.strip_prefix("M-")
        && !rest.is_empty()
    {
        let mut out = vec![0x1b];
        out.extend_from_slice(&key_bytes(rest, app_cursor));
        return out;
    }
    if let Some(rest) = key.strip_prefix("S-") {
        let shifted = match rest {
            "Up" => Some('A'),
            "Down" => Some('B'),
            "Right" => Some('C'),
            "Left" => Some('D'),
            _ => None,
        };
        if let Some(c) = shifted {
            return format!("\x1b[1;2{c}").into_bytes();
        }
    }
    key.as_bytes().to_vec()
}
