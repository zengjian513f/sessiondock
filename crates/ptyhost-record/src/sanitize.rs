//! 从录制的 pty 输出里剥掉终端 QUERY 序列，其余字节原样放行。
//!
//! 只读历史回放会把录到的字节重新喂给浏览器里的 xterm.js。若查询序列也进了
//! 模拟终端，xterm.js 会当作“主机在问我”而作答：DSR/DA/DECRQM 等应答会被
//! 写进**正在进行的**会话，OSC 52 则可能改写剪贴板。录制侧在回放前滤掉这些
//! 查询，浏览器就只会看到普通输出。
//!
//! 只认 7-bit ESC（0x1B）引入的序列。C1 单字节 CSI（0x9B）及其它 C1 控制符
//! 一律当普通字节。未在下面列出的序列（含光标移动、SGR、DECSET/DECRST、
//! OSC 标题/超链接、sixel 等）必须按原始字节原样通过；跨 chunk 切分不得改变
//! 结果。流末尚未结束的序列**不是**查询，由 [`Sanitizer::finish`] 原样刷出。
//!
//! 剥离集合：
//!
//! - CSI DSR / DECXCPR：`ESC[5n`、`ESC[6n`、`ESC[?6n`
//! - CSI DA1 / DA2 / DA3：`ESC[c`、`ESC[0c`、`ESC[>c`、`ESC[>0c`、`ESC[=c`、`ESC[=0c`
//! - CSI XTVERSION：`ESC[>q`、`ESC[>0q`
//! - CSI DECRQM：参数以 `?` 或数字开头、以 `$` 结尾、终结符 `p`（如 `ESC[?2026$p`、`ESC[4$p`）
//! - CSI 窗口/标题报告：终结符 `t`，第一参数为 11、13、14、16、18、19、21
//! - OSC 52：剪贴板读/写（任意形式）
//! - OSC 4、5、10–19：颜色查询（内容最后一个非终止字节为 `?`）
//! - DCS XTGETTCAP（内容以 `+q` 开头）与 DECRQSS（内容以 `$q` 开头）

/// 只读历史查看器在回放结束后追加的复位序列。
///
/// 关闭鼠标跟踪、焦点报告、括号粘贴、DECCKM，恢复数字键盘与光标可见，
/// 关闭同步输出，并复位 SGR。查看器不得把这些字节写进正在录制的会话。
pub const VIEWER_RESET: &[u8] =
    b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1004l\x1b[?2004l\x1b[?1l\x1b>\x1b[?25h\x1b[?2026l\x1b[0m";

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const ST_SLASH: u8 = b'\\';

/// OSC / DCS 在尚未看到终止符时允许累计的最大内容字节数（不含 ESC 与引入符）。
/// 超过则整段当作普通字节放行，避免无终止符的垃圾把过滤器卡死。
const MAX_STRING: usize = 65536;

/// 过滤器所处的显式状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// 普通字节；只扫描下一个 ESC。
    #[default]
    Ground,
    /// 刚吃进 ESC，等待引入符 `[` / `]` / `P` 或其它跟随字节。
    Esc,
    /// CSI：`ESC [` 之后，正在收集 0x20..=0x3F，等待 0x40..=0x7E。
    Csi,
    /// OSC：`ESC ]` 之后，等待 BEL 或 ST（ESC \）。
    Osc,
    /// OSC 内刚看到 ESC，若下一字节是 `\` 则构成 ST。
    OscEsc,
    /// DCS：`ESC P` 之后，等待 ST（ESC \）。
    Dcs,
    /// DCS 内刚看到 ESC，若下一字节是 `\` 则构成 ST。
    DcsEsc,
}

/// 跨 chunk 的流式查询过滤器。
///
/// 可能构成查询、但还没看到终止符的字节会暂存在内部；后续 [`Self::push`]
/// 或 [`Self::finish`] 再决定放行或丢弃。同一输入无论怎样切分，输出字节
/// 序列都相同。
#[derive(Default, Debug)]
pub struct Sanitizer {
    state: State,
    /// ESC 引入后尚未判定去向的字节，始终从 ESC 本身开始。
    held: Vec<u8>,
    stripped: u64,
}

impl Sanitizer {
    /// 构造空过滤器：Ground 状态，尚未剥离任何序列。
    pub fn new() -> Self {
        Self::default()
    }

    /// 把 `input` 喂进过滤器，把已经可以确定的非查询字节追加到 `out`。
    ///
    /// 可能是查询前缀的字节不会在这一次调用里写出；它们留在内部，直到序列
    /// 完整（丢弃或原样放出）或调用方 [`Self::finish`]。
    pub fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        let mut i = 0;
        while i < input.len() {
            match self.state {
                State::Ground => {
                    let rest = &input[i..];
                    match rest.iter().position(|&b| b == ESC) {
                        None => {
                            out.extend_from_slice(rest);
                            return;
                        }
                        Some(rel) => {
                            out.extend_from_slice(&rest[..rel]);
                            self.held.clear();
                            self.held.push(ESC);
                            self.state = State::Esc;
                            i += rel + 1;
                        }
                    }
                }
                State::Esc => {
                    let byte = input[i];
                    i += 1;
                    self.held.push(byte);
                    match byte {
                        b'[' => self.state = State::Csi,
                        b']' => self.state = State::Osc,
                        b'P' => self.state = State::Dcs,
                        _ => {
                            // ESC 后不是 CSI/OSC/DCS 引入符：两字节原样放行。
                            self.emit_held(out);
                            self.state = State::Ground;
                        }
                    }
                }
                State::Csi => {
                    let byte = input[i];
                    i += 1;
                    self.push_csi_byte(byte, out);
                }
                State::Osc => {
                    let byte = input[i];
                    i += 1;
                    self.push_osc_byte(byte, out);
                }
                State::OscEsc => {
                    let byte = input[i];
                    i += 1;
                    self.push_osc_esc_byte(byte, out);
                }
                State::Dcs => {
                    let byte = input[i];
                    i += 1;
                    self.push_dcs_byte(byte, out);
                }
                State::DcsEsc => {
                    let byte = input[i];
                    i += 1;
                    self.push_dcs_esc_byte(byte, out);
                }
            }
        }
    }

    /// 把尚未结束的暂存字节**原样**追加到 `out`，并回到 Ground。
    ///
    /// 流末的残缺序列（单独的 ESC、未写完的 `ESC[6`、无终止符的 OSC/DCS）
    /// 不是一次完整查询，必须保留。剥离计数不清零。
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        self.emit_held(out);
        self.state = State::Ground;
    }

    /// 到目前为止丢弃的完整查询序列个数。
    pub fn stripped(&self) -> u64 {
        self.stripped
    }

    fn push_csi_byte(&mut self, byte: u8, out: &mut Vec<u8>) {
        if byte == ESC {
            // ESC 在 CSI 内开始一段新的转义；未完成的 CSI 不是查询。
            self.emit_held(out);
            self.held.push(ESC);
            self.state = State::Esc;
            return;
        }
        if byte < 0x20 {
            // C0 在 CSI 内立即执行：已收集字节与该 C0 一律原样放行。
            self.held.push(byte);
            self.emit_held(out);
            self.state = State::Ground;
            return;
        }
        if (0x20..=0x3F).contains(&byte) {
            self.held.push(byte);
            return;
        }
        if (0x40..=0x7E).contains(&byte) {
            self.held.push(byte);
            if self.held_csi_is_query() {
                self.drop_held();
            } else {
                self.emit_held(out);
            }
            self.state = State::Ground;
            return;
        }
        // 既非参数/中间字节也非终结符（DEL 或 8-bit）：中止并原样放行。
        self.held.push(byte);
        self.emit_held(out);
        self.state = State::Ground;
    }

    fn push_osc_byte(&mut self, byte: u8, out: &mut Vec<u8>) {
        if byte == BEL {
            self.held.push(byte);
            self.finish_osc(out);
            return;
        }
        if byte == ESC {
            self.held.push(byte);
            self.state = State::OscEsc;
            self.abort_string_if_too_long(out);
            return;
        }
        self.held.push(byte);
        self.abort_string_if_too_long(out);
    }

    fn push_osc_esc_byte(&mut self, byte: u8, out: &mut Vec<u8>) {
        if byte == ST_SLASH {
            self.held.push(byte);
            self.finish_osc(out);
            return;
        }
        // 前一个 ESC 不是 ST 的一部分，当作 OSC 内容，当前字节继续按 OSC 处理。
        self.state = State::Osc;
        self.push_osc_byte(byte, out);
    }

    fn push_dcs_byte(&mut self, byte: u8, out: &mut Vec<u8>) {
        if byte == ESC {
            self.held.push(byte);
            self.state = State::DcsEsc;
            self.abort_string_if_too_long(out);
            return;
        }
        self.held.push(byte);
        self.abort_string_if_too_long(out);
    }

    fn push_dcs_esc_byte(&mut self, byte: u8, out: &mut Vec<u8>) {
        if byte == ST_SLASH {
            self.held.push(byte);
            self.finish_dcs(out);
            return;
        }
        self.state = State::Dcs;
        self.push_dcs_byte(byte, out);
    }

    fn finish_osc(&mut self, out: &mut Vec<u8>) {
        if self.held_osc_is_query() {
            self.drop_held();
        } else {
            self.emit_held(out);
        }
        self.state = State::Ground;
    }

    fn finish_dcs(&mut self, out: &mut Vec<u8>) {
        if self.held_dcs_is_query() {
            self.drop_held();
        } else {
            self.emit_held(out);
        }
        self.state = State::Ground;
    }

    fn abort_string_if_too_long(&mut self, out: &mut Vec<u8>) {
        let collected = self.held.len().saturating_sub(2);
        if collected >= MAX_STRING {
            self.emit_held(out);
            self.state = State::Ground;
        }
    }

    fn emit_held(&mut self, out: &mut Vec<u8>) {
        if !self.held.is_empty() {
            out.extend_from_slice(&self.held);
            self.held.clear();
        }
    }

    fn drop_held(&mut self) {
        self.held.clear();
        self.stripped += 1;
    }

    /// `held` 为完整 CSI：`ESC` `[` 参数/中间字节 终结符。
    fn held_csi_is_query(&self) -> bool {
        if self.held.len() < 3 {
            return false;
        }
        if self.held[0] != ESC || self.held[1] != b'[' {
            return false;
        }
        let final_byte = self.held[self.held.len() - 1];
        let params = &self.held[2..self.held.len() - 1];
        match final_byte {
            b'n' => matches!(params, b"5" | b"6" | b"?6"),
            b'c' => matches!(params, b"" | b"0" | b">" | b">0" | b"=" | b"=0"),
            b'q' => matches!(params, b">" | b">0"),
            b'p' => csi_params_are_decrqm(params),
            b't' => csi_params_are_window_report(params),
            _ => false,
        }
    }

    /// `held` 为完整 OSC，以 BEL 或 ST 结尾。
    fn held_osc_is_query(&self) -> bool {
        let Some(content) = string_content(&self.held) else {
            return false;
        };
        let Some(ps) = first_numeric_param(content) else {
            return false;
        };
        if ps == 52 {
            return true;
        }
        if osc_ps_is_color_query(ps) {
            return content.last() == Some(&b'?');
        }
        false
    }

    /// `held` 为完整 DCS，以 ST 结尾。
    fn held_dcs_is_query(&self) -> bool {
        let Some(content) = string_content(&self.held) else {
            return false;
        };
        content.starts_with(b"+q") || content.starts_with(b"$q")
    }
}

/// 一次性过滤：`new` + `push` + `finish`。
pub fn sanitize(input: &[u8]) -> Vec<u8> {
    let mut sanitizer = Sanitizer::new();
    let mut out = Vec::new();
    sanitizer.push(input, &mut out);
    sanitizer.finish(&mut out);
    out
}

/// CSI DECRQM：参数非空，以 `?` 或数字开头，以 `$` 结尾。
fn csi_params_are_decrqm(params: &[u8]) -> bool {
    match params.split_first() {
        None => false,
        Some((first, _)) => {
            let starts_ok = *first == b'?' || first.is_ascii_digit();
            let ends_ok = *params.last().expect("split_first 保证非空") == b'$';
            starts_ok && ends_ok
        }
    }
}

/// CSI 窗口/文字区域/标题报告：第一参数为指定的 Ps。
fn csi_params_are_window_report(params: &[u8]) -> bool {
    matches!(
        first_numeric_param(params),
        Some(11 | 13 | 14 | 16 | 18 | 19 | 21)
    )
}

fn osc_ps_is_color_query(ps: u32) -> bool {
    ps == 4 || ps == 5 || (10..=19).contains(&ps)
}

/// OSC/DCS 在引入符与终止符之间的内容。
///
/// `held` 形如 `ESC` + 引入符 + content + `BEL`，或 `ESC` + 引入符 + content + `ESC \`。
fn string_content(held: &[u8]) -> Option<&[u8]> {
    if held.len() < 3 || held[0] != ESC {
        return None;
    }
    let last = held[held.len() - 1];
    if last == BEL {
        return Some(&held[2..held.len() - 1]);
    }
    if last == ST_SLASH && held.len() >= 4 && held[held.len() - 2] == ESC {
        return Some(&held[2..held.len() - 2]);
    }
    None
}

/// 取第一个参数：`;` 之前（或到末尾）的十进制数字。非纯数字则没有参数。
fn first_numeric_param(bytes: &[u8]) -> Option<u32> {
    let prefix = match bytes.iter().position(|&b| b == b';') {
        Some(idx) => &bytes[..idx],
        None => bytes,
    };
    if prefix.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for &b in prefix {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u32::from(b - b'0'))?;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIXED: &[u8] = b"a\x1b[6nb\x1b[31mc\x1b]52;c;aGVsbG8=\x07d\x1bP+q544e\x1b\\e";
    // DSR / OSC 52 / XTGETTCAP 被剥掉后剩下 a, b, SGR, c, d, e。
    const MIXED_OUT: &[u8] = b"ab\x1b[31mcde";

    /// 含较长 OSC 8 超链接的流；整段都应原样通过。
    const OSC8: &[u8] = b"pre\x1b]8;id=long-hyperlink-identifier;https://example.com/path/to/resource?foo=bar&baz=qux#section\x07visible-text\x1b]8;;\x07post";

    fn run(chunks: &[&[u8]]) -> (Vec<u8>, u64) {
        let mut sanitizer = Sanitizer::new();
        let mut out = Vec::new();
        for chunk in chunks {
            sanitizer.push(chunk, &mut out);
        }
        sanitizer.finish(&mut out);
        (out, sanitizer.stripped())
    }

    fn assert_dropped(seq: &[u8]) {
        let (out, stripped) = run(&[seq]);
        assert_eq!(out, b"", "expected a complete drop for {seq:?}");
        assert_eq!(stripped, 1, "expected stripped() == 1 for {seq:?}");
        assert_eq!(sanitize(seq), b"", "sanitize() should also drop {seq:?}");
    }

    fn assert_passthrough(seq: &[u8]) {
        let (out, stripped) = run(&[seq]);
        assert_eq!(out, seq, "expected byte-for-byte passthrough for {seq:?}");
        assert_eq!(stripped, 0, "expected stripped() == 0 for {seq:?}");
        assert_eq!(sanitize(seq), seq);
    }

    fn assert_chunk_independent(input: &[u8], expected: &[u8]) {
        let oneshot = sanitize(input);
        assert_eq!(oneshot, expected, "one-shot result");

        for i in 0..=input.len() {
            let (out, _) = run(&[&input[..i], &input[i..]]);
            assert_eq!(
                out,
                expected,
                "two-push split at {i} of {} should match one-shot",
                input.len()
            );
        }

        let mut sanitizer = Sanitizer::new();
        let mut out = Vec::new();
        for &byte in input {
            sanitizer.push(&[byte], &mut out);
        }
        sanitizer.finish(&mut out);
        assert_eq!(out, expected, "byte-by-byte should match one-shot");
    }

    #[test]
    fn each_dropped_csi_dsr_is_removed() {
        assert_dropped(b"\x1b[5n");
        assert_dropped(b"\x1b[6n");
        assert_dropped(b"\x1b[?6n");
    }

    #[test]
    fn each_dropped_csi_da_is_removed() {
        assert_dropped(b"\x1b[c");
        assert_dropped(b"\x1b[0c");
        assert_dropped(b"\x1b[>c");
        assert_dropped(b"\x1b[>0c");
        assert_dropped(b"\x1b[=c");
        assert_dropped(b"\x1b[=0c");
    }

    #[test]
    fn each_dropped_csi_xtversion_is_removed() {
        assert_dropped(b"\x1b[>q");
        assert_dropped(b"\x1b[>0q");
    }

    #[test]
    fn each_dropped_csi_decrqm_is_removed() {
        assert_dropped(b"\x1b[?2026$p");
        assert_dropped(b"\x1b[4$p");
    }

    #[test]
    fn each_dropped_csi_window_report_is_removed() {
        assert_dropped(b"\x1b[11t");
        assert_dropped(b"\x1b[13t");
        assert_dropped(b"\x1b[14t");
        assert_dropped(b"\x1b[16t");
        assert_dropped(b"\x1b[18t");
        assert_dropped(b"\x1b[19t");
        assert_dropped(b"\x1b[21t");
    }

    #[test]
    fn each_dropped_osc_clipboard_and_color_query_is_removed() {
        assert_dropped(b"\x1b]52;c;aGVsbG8=\x07");
        assert_dropped(b"\x1b]52;c;?\x07");
        assert_dropped(b"\x1b]52;c;aGVsbG8=\x1b\\");
        assert_dropped(b"\x1b]4;1;?\x07");
        assert_dropped(b"\x1b]5;0;?\x07");
        assert_dropped(b"\x1b]10;?\x07");
        assert_dropped(b"\x1b]11;?\x07");
        assert_dropped(b"\x1b]12;?\x07");
        assert_dropped(b"\x1b]13;?\x07");
        assert_dropped(b"\x1b]14;?\x07");
        assert_dropped(b"\x1b]15;?\x07");
        assert_dropped(b"\x1b]16;?\x07");
        assert_dropped(b"\x1b]17;?\x07");
        assert_dropped(b"\x1b]18;?\x07");
        assert_dropped(b"\x1b]19;?\x07");
        assert_dropped(b"\x1b]10;?\x1b\\");
    }

    #[test]
    fn each_dropped_dcs_is_removed() {
        assert_dropped(b"\x1bP+q544e\x1b\\");
        assert_dropped(b"\x1bP$q q\x1b\\");
        assert_dropped(b"\x1bP$q\"p\x1b\\");
    }

    #[test]
    fn listed_passthrough_examples_are_unchanged() {
        assert_passthrough(b"\x1b[1;1H");
        assert_passthrough(b"\x1b[1C");
        assert_passthrough(b"\x1b[31m");
        assert_passthrough(b"\x1b[0m");
        assert_passthrough(b"\x1b[?25h");
        assert_passthrough(b"\x1b[?25l");
        assert_passthrough(b"\x1b[?1000h");
        assert_passthrough(b"\x1b[?2004l");
        assert_passthrough(b"\x1b[1;24r");
        assert_passthrough(b"\x1b[0n");
        assert_passthrough(b"\x1b[7n");
        assert_passthrough(b"\x1b[16n");
        assert_passthrough(b"\x1b[?5n");
        assert_passthrough(b"\x1b[>1c");
        assert_passthrough(b"\x1b[1c");
        assert_passthrough(b"\x1b]0;title\x07");
        assert_passthrough(b"\x1b]2;window title\x07");
        assert_passthrough(b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07");
        assert_passthrough(b"\x1b]7;file://host/tmp\x07");
        assert_passthrough(b"\x1b]4;1;#aabbcc\x07");
        assert_passthrough(b"\x1b]10;#ffffff\x07");
        assert_passthrough(b"\x1b]11;rgb:0000/0000/0000\x07");
        assert_passthrough(b"\x1bPq#0;2;0;0;0\x1b\\");
        assert_passthrough(b"\x1bP0;1q#1;2;100;100;100\x1b\\");
        assert_passthrough(b"hello world\n");
        assert_passthrough(b"\x1bOA");
        assert_passthrough(b"\x1b=");
        assert_passthrough(OSC8);
    }

    #[test]
    fn mixed_stream_strips_three_queries() {
        let (out, stripped) = run(&[MIXED]);
        assert_eq!(out, MIXED_OUT);
        assert_eq!(stripped, 3);
        assert_eq!(sanitize(MIXED), MIXED_OUT);
    }

    #[test]
    fn chunk_boundaries_do_not_change_output() {
        assert_chunk_independent(MIXED, MIXED_OUT);
        assert_chunk_independent(OSC8, OSC8);

        let (out, stripped) = run(&[MIXED]);
        assert_eq!(stripped, 3);
        assert_eq!(out, MIXED_OUT);
    }

    #[test]
    fn trailing_esc_is_emitted_unchanged_by_finish() {
        let mut sanitizer = Sanitizer::new();
        let mut out = Vec::new();
        sanitizer.push(b"\x1b", &mut out);
        assert_eq!(out, b"", "lone ESC must be held back");
        sanitizer.finish(&mut out);
        assert_eq!(out, b"\x1b");
        assert_eq!(sanitizer.stripped(), 0);
        assert_eq!(sanitize(b"\x1b"), b"\x1b");
    }

    #[test]
    fn trailing_unterminated_csi_is_emitted_unchanged_by_finish() {
        let mut sanitizer = Sanitizer::new();
        let mut out = Vec::new();
        sanitizer.push(b"\x1b[6", &mut out);
        assert_eq!(out, b"", "unterminated CSI must be held back");
        sanitizer.finish(&mut out);
        assert_eq!(out, b"\x1b[6");
        assert_eq!(sanitizer.stripped(), 0);
        assert_eq!(sanitize(b"\x1b[6"), b"\x1b[6");
        assert_eq!(sanitize(b"a\x1b[6"), b"a\x1b[6");
    }

    #[test]
    fn c0_inside_csi_aborts_and_passes_through() {
        assert_passthrough(b"\x1b[6\n");
        assert_passthrough(b"\x1b[6\x08n");
        assert_passthrough(b"x\x1b[1;31\x07y");

        // C0 中止之后，随后的完整查询仍应被剥掉。
        let (out, stripped) = run(&[b"\x1b[6\n\x1b[6n"]);
        assert_eq!(out, b"\x1b[6\n");
        assert_eq!(stripped, 1);
    }

    #[test]
    fn oversized_unterminated_osc_passes_through() {
        let mut input = vec![ESC, b']'];
        input.resize(2 + MAX_STRING + 1, b'x');
        assert!(
            input.len() > 2 + MAX_STRING,
            "fixture must be longer than 65536 collected bytes"
        );

        let mut sanitizer = Sanitizer::new();
        let mut out = Vec::new();
        sanitizer.push(&input, &mut out);
        sanitizer.finish(&mut out);
        assert_eq!(out, input);
        assert_eq!(sanitizer.stripped(), 0);
        assert_eq!(sanitize(&input), input);

        let mut with_tail = input.clone();
        with_tail.extend_from_slice(b"tail");
        assert_eq!(sanitize(&with_tail), with_tail);
    }

    #[test]
    fn c1_byte_0x9b_is_an_ordinary_byte() {
        assert_passthrough(b"\x9b6n");
        assert_passthrough(b"hello\x9b[6nworld");
        assert_passthrough(&[0x9B]);

        // 0x9B 不能把后面真正的 7-bit DSR 一起吞掉。
        let (out, stripped) = run(&[b"\x9b\x1b[6n"]);
        assert_eq!(out, b"\x9b");
        assert_eq!(stripped, 1);
    }

    #[test]
    fn viewer_reset_matches_the_specified_bytes() {
        assert_eq!(
            VIEWER_RESET,
            b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1004l\x1b[?2004l\x1b[?1l\x1b>\x1b[?25h\x1b[?2026l\x1b[0m"
        );
        assert_passthrough(VIEWER_RESET);
    }

    #[test]
    fn new_starts_with_zero_stripped() {
        assert_eq!(Sanitizer::new().stripped(), 0);
        assert_eq!(Sanitizer::default().stripped(), 0);
        assert_eq!(sanitize(b""), b"");
    }
}
