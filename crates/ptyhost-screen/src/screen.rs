//! alacritty_terminal 之上的薄封装，提供与 Python 参考实现同语义的截屏 / 光标 / 回放。
//!
//! 只服务 capture / cursor 查询、attach 回放、最终画面和服务端网格：
//! 实时字节由读线程直接转发，不经过这里。
//!
//! 模型会自己应答终端查询（DA、DECRQM、XTGETTCAP、颜色查询……），应答字节先攒在
//! [`Screen::take_responses`] 里，由会话决定写不写回 pty：有 xterm.js 连着时由它答，
//! 只有网格客户端时由模型答。DSR 仍在读线程被截下、由宿主按模型光标应答。
//!
//! 模型内部的 panic 在这里拦下：模型只是画面的副本，坏了可以从头重建，
//! 但绝不能让宿主里的锁因此失效、把 attach 和 pty 读线程一起拖死。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Row, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};

/// 模型发出的事件：应答字节攒起来，标题记下来，其余忽略。
#[derive(Clone, Default)]
pub struct Responder {
    inner: Arc<Mutex<ResponderState>>,
}

#[derive(Default)]
struct ResponderState {
    responses: Vec<u8>,
    title: String,
    /// OSC 52 写入的剪贴板文本（已解码），等网格客户端取走。
    clipboard: Vec<String>,
}

/// 颜色查询的应答用一套固定的深色盘：浏览器主题不在宿主手里，
/// 给一个合理值总比让应用等到超时好。
fn palette(index: usize) -> Rgb {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    let (r, g, b) = match index {
        0..=15 => BASE[index],
        16..=231 => {
            let i = index - 16;
            let step = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            (step(i / 36), step((i / 6) % 6), step(i % 6))
        }
        232..=255 => {
            let v = (8 + (index - 232) * 10) as u8;
            (v, v, v)
        }
        257 => (0, 0, 0),
        _ => (229, 229, 229),
    };
    Rgb { r, g, b }
}

impl EventListener for Responder {
    fn send_event(&self, event: Event) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            Event::PtyWrite(text) => state.responses.extend_from_slice(text.as_bytes()),
            Event::ColorRequest(index, format) => state
                .responses
                .extend_from_slice(format(palette(index)).as_bytes()),
            Event::Title(title) => state.title = title,
            Event::ResetTitle => state.title.clear(),
            Event::ClipboardStore(_, text) => state.clipboard.push(text),
            _ => {}
        }
    }
}

struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Screen {
    term: Term<Responder>,
    parser: Processor,
    responder: Responder,
    cols: u16,
    rows: u16,
    history: usize,
    resets: u64,
    /// 滚动计数（见 [`Screen::take_scrolled`]）：上次取数后是否把主屏的
    /// display_offset 设成了 1，以及当时的历史行数和最新一行历史。
    scroll_armed: bool,
    scroll_seen: usize,
    scroll_mark: Option<Row<Cell>>,
    /// 见 [`Screen::take_resize_moved`]：上次取数后 resize 前后进出主屏历史的行数；
    /// 期间有过宽度重排、备用屏幕上的 resize 或模型重建时为 None。
    resize_moved: Option<i64>,
}

fn new_term(cols: u16, rows: u16, history: usize, responder: Responder) -> Term<Responder> {
    let config = Config {
        scrolling_history: history,
        ..Config::default()
    };
    Term::new(
        config,
        &Size {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        },
        responder,
    )
}

impl Screen {
    pub fn new(cols: u16, rows: u16, history: usize) -> Self {
        let responder = Responder::default();
        Self {
            term: new_term(cols, rows, history, responder.clone()),
            parser: Processor::new(),
            responder,
            cols: cols.max(1),
            rows: rows.max(1),
            history,
            resets: 0,
            scroll_armed: false,
            scroll_seen: 0,
            scroll_mark: None,
            resize_moved: Some(0),
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        let ok = catch_unwind(AssertUnwindSafe(|| {
            self.parser.advance(&mut self.term, data);
        }))
        .is_ok();
        if !ok {
            self.recover();
            return;
        }
        self.expire_sync();
    }

    /// 同步输出（DEC 2026）在 vte 里缓冲；`?2026l` 迟迟不来时到期强制应用。
    /// 返回是否应用了缓冲内容。
    pub fn expire_sync(&mut self) -> bool {
        match self.parser.sync_timeout().sync_timeout() {
            Some(deadline) if Instant::now() >= deadline => {
                let ok = catch_unwind(AssertUnwindSafe(|| {
                    self.parser.stop_sync(&mut self.term);
                }))
                .is_ok();
                if !ok {
                    self.recover();
                }
                true
            }
            _ => false,
        }
    }

    /// 正在进行的同步输出的到期时刻；没有则 None。
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let (old_cols, old_rows) = (self.cols, self.rows);
        // 先结清 resize 前已滚进历史、还没被取走的行：resize 会挪动滚动计数，
        // 而 resize 之后的那帧快照不带 scrolled。
        let pending = self.take_scrolled();
        let alt = self.alt();
        let before = self.history_len();
        let cursor_line = self.term.grid().cursor.point.line.0.max(0) as usize;
        self.cols = cols;
        self.rows = rows;
        let ok = catch_unwind(AssertUnwindSafe(|| {
            self.term.resize(Size {
                cols: cols as usize,
                rows: rows as usize,
            });
        }))
        .is_ok();
        if !ok {
            self.recover();
            return;
        }
        if alt || cols != old_cols {
            // 宽度变化整体重排，备用屏幕上的主屏也会被重排：算不出搬了哪些行。
            self.resize_moved = None;
        } else if let Some(moved) = self.resize_moved {
            // 变矮：光标以上放不下的行被推进历史（历史满时最旧的行同时被挤掉，
            // 行数照样计入）；变高：历史最后几行被拉回屏幕顶部。
            let pushed = (cursor_line + 1).saturating_sub(rows as usize);
            let pulled = if rows > old_rows {
                before.saturating_sub(self.history_len())
            } else {
                0
            };
            self.resize_moved =
                Some(moved + pending.unwrap_or(0) as i64 + pushed as i64 - pulled as i64);
        }
        // resize 之后重新起算滚动计数。
        let _ = self.take_scrolled();
    }

    /// 上次取数以来 resize 前后进入（正）或离开（负）主屏历史的行数，包括 resize
    /// 前已滚进历史、尚未取走的输出行。宿主历史满了以后 history_total 不再变化，
    /// 浏览器靠它在 resize 快照里补上这些行。有过宽度重排等算不出的情况时为 None。
    pub fn take_resize_moved(&mut self) -> Option<i64> {
        self.resize_moved.replace(0)
    }

    /// 模型因 panic 被重建的次数；capture 应答里带上，调用方可据此判断画面是否可信。
    pub fn resets(&self) -> u64 {
        self.resets
    }

    /// 模型攒下的查询应答（DA、DECRQM、颜色查询等），取走后清空。
    pub fn take_responses(&mut self) -> Vec<u8> {
        let mut state = self
            .responder
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut state.responses)
    }

    /// OSC 52 写入的剪贴板内容（按发生顺序），取走后清空。
    pub fn take_clipboard(&mut self) -> Vec<String> {
        let mut state = self
            .responder
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut state.clipboard)
    }

    pub fn title(&self) -> String {
        self.responder
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .title
            .clone()
    }

    /// 模型内部越界：丢掉旧模型，从空白重建。历史与画面都不可恢复。
    fn recover(&mut self) {
        let (cols, rows, history) = (self.cols, self.rows, self.history);
        self.term = new_term(cols, rows, history, self.responder.clone());
        self.parser = Processor::new();
        self.resets += 1;
        self.resize_moved = None;
        eprintln!(
            "screen model reset #{} ({}x{}, blank)",
            self.resets, cols, rows
        );
    }

    pub fn term(&self) -> &Term<Responder> {
        &self.term
    }

    /// (x, y)：列在前，行在后，均 0 起算。
    pub fn cursor(&self) -> (u16, u16) {
        let point = self.term.grid().cursor.point;
        (point.column.0 as u16, point.line.0.max(0) as u16)
    }

    pub fn cursor_visible(&self) -> bool {
        self.term.mode().contains(TermMode::SHOW_CURSOR)
    }

    pub fn alt(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn app_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// 历史行数（主屏已滚出视口的行）。
    pub fn history_len(&self) -> usize {
        self.term.grid().history_size()
    }

    /// 自上次调用以来主屏推入历史的行数，不超过当前历史长度：历史满了以后
    /// 每进一行就从最旧一端挤掉一行，长度不再变化，但新行照样计入。返回的行
    /// 就是历史最后那么多行。备用屏幕期间返回 None，计数留到回到主屏。
    ///
    /// alacritty 不公开滚动计数，这里借用主屏的 display_offset：它不为 0 时，
    /// 每次把行滚进历史都会加上滚动的行数（封顶于历史上限）。取数后把它放回 1；
    /// 宿主只按绝对行号读网格（[`Screen::line_at`]），不依赖 display_offset。
    /// 清空历史（ED 3、RIS）和模型重建会把它清零，此时现有历史都是之后滚进的。
    /// resize 也会挪动它，调用方在 resize 后的那次捕获里丢弃结果即可。
    pub fn take_scrolled(&mut self) -> Option<usize> {
        if self.alt() {
            return None;
        }
        let total = self.history_len();
        let offset = self.term.grid().display_offset();
        let count = if !self.scroll_armed {
            total.saturating_sub(self.scroll_seen)
        } else if offset == 0 {
            total
        } else if offset < self.history {
            offset - 1
        } else {
            // 封顶：至少滚了 history-1 行（此时历史一定是满的）。恰好 history-1 行时，
            // 上次最新的历史行正好成了最旧一行；否则保留的历史全是新行。
            let oldest = &self.term.grid()[Line(-(total as i32))];
            if total > 0 && self.scroll_mark.as_ref() == Some(oldest) {
                total - 1
            } else {
                total
            }
        };
        self.scroll_seen = total;
        self.scroll_armed = self.history >= 2 && total >= 1;
        if self.scroll_armed {
            self.scroll_mark = Some(self.term.grid()[Line(-1)].clone());
            self.term
                .grid_mut()
                .scroll_display(Scroll::Delta(1 - offset as i32));
        } else {
            self.scroll_mark = None;
        }
        Some(count.min(total))
    }

    /// 绝对行号 → 网格行：历史为 0..history，可见屏接在后面。
    pub fn line_at(&self, abs: usize) -> Line {
        Line(abs as i32 - self.history_len() as i32)
    }

    /// 当前可见屏的各行；join 为真时把软换行合并成逻辑行。
    pub fn screen_lines(&self, styled: bool, join: bool) -> Vec<String> {
        let rows = (0..self.rows as i32)
            .map(|row| self.row_text(Line(row), styled))
            .collect();
        finish(rows, join)
    }

    /// 历史最后 limit 行加上可见屏，对应 capture-pane -S -limit。
    pub fn scrollback_lines(&self, limit: usize, styled: bool, join: bool) -> Vec<String> {
        let available = self.history_len();
        let from = available - limit.min(available);
        let end = available + usize::from(self.rows);
        let rows = (from..end)
            .map(|abs| self.row_text(self.line_at(abs), styled))
            .collect();
        finish(rows, join)
    }

    /// attach 回放 / 最终画面：历史文本 + 完整终端状态（含模式与光标）。
    pub fn replay_bytes(&self, history: usize) -> Vec<u8> {
        let available = self.history_len();
        let from = available - history.min(available);
        let mut out = Vec::new();
        if from < available {
            let rows = (from..available)
                .map(|abs| self.row_text(self.line_at(abs), true))
                .collect();
            for line in finish(rows, true) {
                out.extend_from_slice(line.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(b"\x1b[0m");
        }
        out.extend_from_slice(&self.state_bytes());
        out
    }

    /// 一行的文本（styled 时带 SGR）与软换行标记。行尾默认属性的空格裁掉。
    fn row_text(&self, line: Line, styled: bool) -> (String, bool) {
        let grid = self.term.grid();
        let cols = grid.columns();
        let row = &grid[line];
        let wrapped = row[Column(cols.saturating_sub(1))]
            .flags
            .contains(Flags::WRAPLINE);
        let mut text = String::new();
        let mut pen = Pen::default();
        // 只在遇到非空格子时才把之前攒下的空白写出去，实现行尾裁剪。
        let mut pending_blank = String::new();
        for col in 0..cols {
            let cell = &row[Column(col)];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let attrs = Pen::of(cell);
            let blank = cell.c == ' ' && cell.zerowidth().is_none() && attrs == Pen::default();
            if blank {
                pending_blank.push(' ');
                continue;
            }
            if !pending_blank.is_empty() {
                if styled && pen != Pen::default() {
                    text.push_str("\x1b[0m");
                    pen = Pen::default();
                }
                text.push_str(&pending_blank);
                pending_blank.clear();
            }
            if styled && attrs != pen {
                text.push_str(&attrs.sgr());
                pen = attrs;
            }
            push_cell_text(&mut text, cell);
        }
        if styled && pen != Pen::default() {
            text.push_str("\x1b[0m");
        }
        (text, wrapped)
    }

    /// 逐行重画当前屏（绝对定位），再恢复光标、笔属性、各项模式。
    /// 软换行标记随之丢失，只影响客户端 resize 时对可见屏的重新折行。
    fn state_bytes(&self) -> Vec<u8> {
        let mode = self.term.mode();
        let mut out = Vec::new();
        if mode.contains(TermMode::ALT_SCREEN) {
            out.extend_from_slice(b"\x1b[?1049h");
        }
        // 不用 ED 2（`ESC[2J`）：alacritty 会把被清掉的可见行推进回滚区，
        // 回放就多出空行。逐行定位 + 擦除整行，历史区不受影响。
        out.extend_from_slice(b"\x1b[0m\x1b[H");
        for row in 0..self.rows as i32 {
            let (text, _) = self.row_text(Line(row), true);
            out.extend_from_slice(format!("\x1b[{};1H\x1b[2K", row + 1).as_bytes());
            out.extend_from_slice(text.as_bytes());
        }
        let (x, y) = self.cursor();
        out.extend_from_slice(format!("\x1b[0m\x1b[{};{}H", y + 1, x + 1).as_bytes());
        let pen = Pen::of(&self.term.grid().cursor.template);
        if pen != Pen::default() {
            out.extend_from_slice(pen.sgr().as_bytes());
        }
        fn flag<'a>(on: bool, set: &'a [u8], reset: &'a [u8]) -> &'a [u8] {
            if on { set } else { reset }
        }
        out.extend_from_slice(flag(
            mode.contains(TermMode::APP_CURSOR),
            b"\x1b[?1h",
            b"\x1b[?1l",
        ));
        out.extend_from_slice(flag(
            mode.contains(TermMode::APP_KEYPAD),
            b"\x1b=",
            b"\x1b>",
        ));
        out.extend_from_slice(flag(
            mode.contains(TermMode::BRACKETED_PASTE),
            b"\x1b[?2004h",
            b"\x1b[?2004l",
        ));
        out.extend_from_slice(flag(
            mode.contains(TermMode::FOCUS_IN_OUT),
            b"\x1b[?1004h",
            b"\x1b[?1004l",
        ));
        out.extend_from_slice(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l");
        if mode.contains(TermMode::MOUSE_MOTION) {
            out.extend_from_slice(b"\x1b[?1003h");
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            out.extend_from_slice(b"\x1b[?1002h");
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            out.extend_from_slice(b"\x1b[?1000h");
        }
        out.extend_from_slice(flag(
            mode.contains(TermMode::SGR_MOUSE),
            b"\x1b[?1006h",
            b"\x1b[?1006l",
        ));
        out.extend_from_slice(flag(
            mode.contains(TermMode::UTF8_MOUSE),
            b"\x1b[?1005h",
            b"\x1b[?1005l",
        ));
        out.extend_from_slice(flag(
            mode.contains(TermMode::SHOW_CURSOR),
            b"\x1b[?25h",
            b"\x1b[?25l",
        ));
        out
    }
}

/// 把一个格子的字符（含零宽附加字符）追加到文本。
pub fn push_cell_text(text: &mut String, cell: &Cell) {
    text.push(cell.c);
    if let Some(extra) = cell.zerowidth() {
        text.extend(extra.iter());
    }
}

/// 一组 SGR 属性；用于比较相邻格子并生成最短的转义序列。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pen {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strikeout: bool,
    pub hidden: bool,
}

/// 颜色是否就是终端默认色。
fn is_default(color: Color, foreground: bool) -> bool {
    match color {
        Color::Named(NamedColor::Foreground) => foreground,
        Color::Named(NamedColor::Background) => !foreground,
        _ => false,
    }
}

impl Pen {
    pub fn of(cell: &Cell) -> Self {
        let flags = cell.flags;
        Self {
            fg: (!is_default(cell.fg, true)).then_some(cell.fg),
            bg: (!is_default(cell.bg, false)).then_some(cell.bg),
            bold: flags.contains(Flags::BOLD),
            dim: flags.contains(Flags::DIM),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            inverse: flags.contains(Flags::INVERSE),
            strikeout: flags.contains(Flags::STRIKEOUT),
            hidden: flags.contains(Flags::HIDDEN),
        }
    }

    /// 从默认状态出发设置这组属性的 SGR（先 `0` 复位再逐项设置）。
    pub fn sgr(&self) -> String {
        let mut params: Vec<String> = vec!["0".into()];
        if self.bold {
            params.push("1".into());
        }
        if self.dim {
            params.push("2".into());
        }
        if self.italic {
            params.push("3".into());
        }
        if self.underline {
            params.push("4".into());
        }
        if self.inverse {
            params.push("7".into());
        }
        if self.hidden {
            params.push("8".into());
        }
        if self.strikeout {
            params.push("9".into());
        }
        if let Some(fg) = self.fg {
            params.push(color_sgr(fg, true));
        }
        if let Some(bg) = self.bg {
            params.push(color_sgr(bg, false));
        }
        format!("\x1b[{}m", params.join(";"))
    }
}

fn color_sgr(color: Color, foreground: bool) -> String {
    let base = if foreground { 30 } else { 40 };
    match color {
        Color::Named(named) => {
            let index = named as usize;
            if index < 8 {
                (base + index).to_string()
            } else if index < 16 {
                (base + 60 + index - 8).to_string()
            } else if foreground {
                "39".into()
            } else {
                "49".into()
            }
        }
        Color::Indexed(index) => format!("{};5;{index}", base + 8),
        Color::Spec(rgb) => format!("{};2;{};{};{}", base + 8, rgb.r, rgb.g, rgb.b),
    }
}

/// 软换行合并：与参考实现一致，wrapped 行与下一行拼成同一条逻辑行。
fn finish(rows: Vec<(String, bool)>, join: bool) -> Vec<String> {
    if !join {
        return rows.into_iter().map(|(text, _)| text).collect();
    }
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut joining = false;
    for (text, wrapped) in rows {
        if joining {
            buf.push_str(&text);
        } else {
            buf = text;
        }
        if wrapped {
            joining = true;
            continue;
        }
        out.push(std::mem::take(&mut buf));
        joining = false;
    }
    if joining {
        out.push(buf);
    }
    out
}
