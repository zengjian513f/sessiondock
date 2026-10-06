//! 服务端网格：把终端模型的画面抽成"行 → span"结构，和上一帧比较后产出 JSON 增量。
//!
//! 浏览器不再解析转义序列，只画格子。一行是若干 span，span 是属性相同、宽度相同的
//! 连续格子：`["文本", fg, bg, flags]`。颜色编码：`-1` 默认色，`0..=255` 索引色，
//! `0x1000000 | (r<<16 | g<<8 | b)` 真彩。flags 位：1 粗体、2 暗淡、4 斜体、8 下划线、
//! 16 反显、32 宽字符（该 span 每个字符占两格）。空格子输出 `" "`；行尾全为默认属性
//! 空格的 span 省略，浏览器自行补齐。行 JSON：`{"s":[...],"w":是否软换行}`。
//!
//! 只有屏幕线程调用这里（它是唯一喂模型的线程）。

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::Color;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::screen::{Responder, Screen, push_cell_text};

pub const FLAG_BOLD: u32 = 1;
pub const FLAG_DIM: u32 = 2;
pub const FLAG_ITALIC: u32 = 4;
pub const FLAG_UNDERLINE: u32 = 8;
pub const FLAG_INVERSE: u32 = 16;
pub const FLAG_WIDE: u32 = 32;
pub const FLAG_STRIKEOUT: u32 = 64;
pub const FLAG_HIDDEN: u32 = 128;

/// 快照里随附的最多历史行数；更早的行以后按需分页。
pub const SNAPSHOT_HISTORY_ROWS: usize = 2000;

/// 一帧的画面：每行已序列化成 JSON 字符串，便于逐行比较。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridState {
    pub cols: u16,
    pub rows: u16,
    pub rows_json: Vec<String>,
    /// (x, y, visible)
    pub cursor: (u16, u16, bool),
    pub modes: String,
    pub alt: bool,
    /// 捕获时的历史行数（主屏滚出的行）。
    pub history: usize,
}

fn color(c: Color) -> i64 {
    match c {
        Color::Named(named) => {
            let index = named as usize;
            if index < 16 { index as i64 } else { -1 }
        }
        Color::Indexed(i) => i64::from(i),
        Color::Spec(rgb) => {
            0x100_0000 | (i64::from(rgb.r) << 16) | (i64::from(rgb.g) << 8) | i64::from(rgb.b)
        }
    }
}

fn cell_flags(cell: &Cell) -> u32 {
    let f = cell.flags;
    let mut flags = 0;
    if f.contains(Flags::BOLD) {
        flags |= FLAG_BOLD;
    }
    if f.contains(Flags::DIM) {
        flags |= FLAG_DIM;
    }
    if f.contains(Flags::ITALIC) {
        flags |= FLAG_ITALIC;
    }
    if f.intersects(Flags::ALL_UNDERLINES) {
        flags |= FLAG_UNDERLINE;
    }
    if f.contains(Flags::INVERSE) {
        flags |= FLAG_INVERSE;
    }
    if f.contains(Flags::WIDE_CHAR) {
        flags |= FLAG_WIDE;
    }
    if f.contains(Flags::STRIKEOUT) {
        flags |= FLAG_STRIKEOUT;
    }
    if f.contains(Flags::HIDDEN) {
        flags |= FLAG_HIDDEN;
    }
    flags
}

struct Span {
    text: String,
    fg: i64,
    bg: i64,
    flags: u32,
    /// OSC 8 超链接目标；同一 span 内所有格子相同。
    link: Option<String>,
    /// 下划线颜色（SGR 58）；None 表示跟随前景色。
    ul: Option<i64>,
}

impl Span {
    fn is_blank(&self) -> bool {
        self.fg == -1
            && self.bg == -1
            && self.flags == 0
            && self.link.is_none()
            && self.ul.is_none()
            && self.text.bytes().all(|b| b == b' ')
    }

    fn same_attrs(&self, other: &Self) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.flags == other.flags
            && self.link == other.link
            && self.ul == other.ul
    }

    fn json(self) -> Value {
        let mut value = json!([self.text, self.fg, self.bg, self.flags]);
        if self.link.is_some() || self.ul.is_some() {
            let mut extra = json!({});
            if let Some(link) = self.link {
                extra["link"] = json!(link);
            }
            if let Some(ul) = self.ul {
                extra["ul"] = json!(ul);
            }
            value.as_array_mut().unwrap().push(extra);
        }
        value
    }
}

/// 把网格的一行抽成 JSON 行（`line` 可为负：历史行）。
pub fn row_json(term: &Term<Responder>, line: Line) -> String {
    let grid = term.grid();
    let cols = grid.columns();
    let row = &grid[line];
    let mut spans: Vec<Span> = Vec::new();
    for col in 0..cols {
        let cell = &row[Column(col)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let mut text = String::new();
        push_cell_text(&mut text, cell);
        let span = Span {
            text,
            fg: color(cell.fg),
            bg: color(cell.bg),
            flags: cell_flags(cell),
            link: cell.hyperlink().map(|link| link.uri().to_string()),
            ul: cell.underline_color().map(color),
        };
        match spans.last_mut() {
            Some(last) if last.same_attrs(&span) => last.text.push_str(&span.text),
            _ => spans.push(span),
        }
    }
    while spans.last().is_some_and(Span::is_blank) {
        spans.pop();
    }
    // 行尾默认属性的空格也裁掉；浏览器按列宽补齐。
    if let Some(last) = spans.last_mut().filter(|last| {
        last.fg == -1
            && last.bg == -1
            && last.flags == 0
            && last.link.is_none()
            && last.ul.is_none()
    }) {
        let trimmed = last.text.trim_end_matches(' ').len();
        last.text.truncate(trimmed);
    }
    let wrapped = row[Column(cols.saturating_sub(1))]
        .flags
        .contains(Flags::WRAPLINE);
    let spans: Vec<Value> = spans.into_iter().map(Span::json).collect();
    json!({"s": spans, "w": wrapped}).to_string()
}

fn modes_json(term: &Term<Responder>) -> String {
    let mode = term.mode();
    let mouse = if mode.contains(TermMode::MOUSE_MOTION) {
        "any_motion"
    } else if mode.contains(TermMode::MOUSE_DRAG) {
        "button_motion"
    } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
        "press_release"
    } else {
        "none"
    };
    let encoding = if mode.contains(TermMode::SGR_MOUSE) {
        "sgr"
    } else if mode.contains(TermMode::UTF8_MOUSE) {
        "utf8"
    } else {
        "default"
    };
    json!({
        "alt": mode.contains(TermMode::ALT_SCREEN),
        "app_cursor": mode.contains(TermMode::APP_CURSOR),
        "app_keypad": mode.contains(TermMode::APP_KEYPAD),
        "bracketed_paste": mode.contains(TermMode::BRACKETED_PASTE),
        "focus_events": mode.contains(TermMode::FOCUS_IN_OUT),
        "mouse": mouse,
        "mouse_encoding": encoding,
    })
    .to_string()
}

/// 捕获当前画面。
pub fn capture(screen: &Screen) -> GridState {
    let history = screen.history_len();
    let term = screen.term();
    let (cols, rows) = (term.columns() as u16, term.screen_lines() as u16);
    let rows_json = (0..rows as i32)
        .map(|row| row_json(term, Line(row)))
        .collect();
    let (x, y) = screen.cursor();
    GridState {
        cols,
        rows,
        rows_json,
        cursor: (x, y, screen.cursor_visible()),
        modes: modes_json(term),
        alt: screen.alt(),
        history,
    }
}

/// 绝对历史行 `[from, to)` 的 JSON 行（0 = 最旧）；越界部分截掉。
pub fn history_rows(screen: &Screen, from: usize, to: usize) -> Vec<String> {
    let to = to.min(screen.history_len());
    if from >= to {
        return Vec::new();
    }
    (from..to)
        .map(|abs| row_json(screen.term(), screen.line_at(abs)))
        .collect()
}

/// 自上次调用以来从主屏滚进历史、现在仍留着的行（最旧在前），按推入历史的
/// 行数计，历史满了（长度不再增长）也照样给出。备用屏幕期间为空，计数留到
/// 回到主屏。每次捕获都要调用一次以消耗计数；用不上的结果（resize 快照、
/// 新客户端）直接丢弃。
pub fn take_scrolled_rows(screen: &mut Screen) -> Vec<String> {
    let Some(count) = screen.take_scrolled() else {
        return Vec::new();
    };
    let total = screen.history_len();
    history_rows(screen, total - count, total)
}

fn cursor_json(cursor: (u16, u16, bool)) -> Value {
    json!({"x": cursor.0, "y": cursor.1, "visible": cursor.2})
}

fn raw_rows(rows: &[String]) -> Vec<Value> {
    rows.iter()
        .map(|row| serde_json::from_str(row).unwrap_or(Value::Null))
        .collect()
}

/// 完整快照：可见行、光标、模式，以及最近的历史行。`reset` 为真表示新模型
/// （首次 attach / 重连），浏览器要用 `history` 替换自己的回滚区；为假（resize 后）
/// 只换视口。
pub fn snapshot_json(
    state: &GridState,
    history: &[String],
    history_total: usize,
    seq: u64,
    reset: bool,
) -> String {
    let mut value = json!({
        "t": "snapshot",
        "seq": seq,
        "reset": reset,
        "cols": state.cols,
        "rows": state.rows,
        "grid": raw_rows(&state.rows_json),
        "history": raw_rows(history),
        "history_total": history_total,
        "cursor": cursor_json(state.cursor),
    });
    value["modes"] = serde_json::from_str(&state.modes).unwrap_or(Value::Null);
    value.to_string()
}

/// 两帧之间的增量；`scrolled` 是这期间从主屏滚出的历史行。没有任何变化时返回 None。
/// 带 `scrolled` 时同时带上 `history_total`：历史满了以后它不再增长，浏览器据此
/// 知道宿主从最旧一端挤掉了多少行。
pub fn diff_json(
    prev: &GridState,
    next: &GridState,
    scrolled: &[String],
    title: Option<&str>,
    seq: u64,
) -> Option<String> {
    let mut rows: Vec<Value> = Vec::new();
    for (y, row) in next.rows_json.iter().enumerate() {
        if prev.rows_json.get(y) != Some(row) {
            rows.push(json!([
                y,
                serde_json::from_str::<Value>(row).unwrap_or(Value::Null)
            ]));
        }
    }
    let cursor_changed = prev.cursor != next.cursor;
    let modes_changed = prev.modes != next.modes;
    if rows.is_empty()
        && scrolled.is_empty()
        && !cursor_changed
        && !modes_changed
        && title.is_none()
    {
        return None;
    }
    let mut value = json!({"t": "diff", "seq": seq});
    if !scrolled.is_empty() {
        value["scrolled"] = Value::Array(raw_rows(scrolled));
        value["history_total"] = json!(next.history);
    }
    if !rows.is_empty() {
        value["rows"] = Value::Array(rows);
    }
    if cursor_changed {
        value["cursor"] = cursor_json(next.cursor);
    }
    if modes_changed {
        value["modes"] = serde_json::from_str(&next.modes).unwrap_or(Value::Null);
    }
    if let Some(title) = title {
        value["title"] = json!(title);
    }
    Some(value.to_string())
}

/// 静默阈值：最后一个字节之后这么久没有新输出就发。
pub const QUIET: Duration = Duration::from_millis(1);
/// 延迟上限：持续输出时至少每隔这么久发一次。
pub const CAP: Duration = Duration::from_millis(8);

/// 增量发送时机：不按固定定时器，而是"队列排空后静默 1 ms，或距首个未发变化 8 ms"。
#[derive(Default)]
pub struct FlushPolicy {
    first_dirty: Option<Instant>,
    last_change: Option<Instant>,
}

impl FlushPolicy {
    pub fn note(&mut self, now: Instant) {
        self.first_dirty.get_or_insert(now);
        self.last_change = Some(now);
    }

    pub fn dirty(&self) -> bool {
        self.first_dirty.is_some()
    }

    pub fn reset(&mut self) {
        self.first_dirty = None;
        self.last_change = None;
    }

    pub fn due(&self, now: Instant, queue_empty: bool) -> bool {
        let (Some(first), Some(last)) = (self.first_dirty, self.last_change) else {
            return false;
        };
        now.duration_since(first) >= CAP || (queue_empty && now.duration_since(last) >= QUIET)
    }

    /// 距下一次可能到期还要等多久；不脏时 None。
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        let (first, last) = (self.first_dirty?, self.last_change?);
        let cap = CAP.saturating_sub(now.duration_since(first));
        let quiet = QUIET.saturating_sub(now.duration_since(last));
        Some(cap.min(quiet))
    }
}
