//! alacritty_terminal 之上的薄封装，提供与 Python 参考实现同语义的截屏 / 光标 / 回放。
//!
//! 只服务 capture / cursor 查询、attach 回放、录制 checkpoint 和服务端网格：
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
use alacritty_terminal::grid::Dimensions;
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
        }
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

    /// attach 回放 / 录制 checkpoint：历史文本 + 完整终端状态（含模式与光标）。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(screen: &mut Screen) -> Vec<String> {
        screen.screen_lines(false, false)
    }

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                continue;
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn lines_scroll_into_history_and_soft_wraps_join() {
        let mut screen = Screen::new(10, 3, 100);
        screen.feed(b"hello\r\nworld\r\nabcdefghijklmno");
        assert_eq!(plain(&mut screen), vec!["world", "abcdefghij", "klmno"]);
        assert_eq!(screen.cursor(), (5, 2));
        let joined = screen.scrollback_lines(100, false, true);
        assert!(joined.iter().any(|l| l.trim_end() == "hello"), "{joined:?}");
        assert!(
            joined.iter().any(|l| l.starts_with("abcdefghijklmno")),
            "软换行未合并: {joined:?}"
        );
    }

    #[test]
    fn styled_rows_carry_sgr_that_strips_back_to_plain_text() {
        let mut screen = Screen::new(20, 2, 0);
        screen.feed(b"\x1b[1;31mRED\x1b[0m ok");
        let styled = screen.screen_lines(true, false).remove(0);
        assert!(styled.contains("RED"));
        assert!(styled.contains('\x1b'), "样式行应带 SGR: {styled:?}");
        let stripped = strip_ansi(&styled);
        assert_eq!(stripped.trim_end(), "RED ok");
    }

    #[test]
    fn cursor_moves_erase_and_insert_delete() {
        let mut screen = Screen::new(10, 4, 0);
        screen.feed(b"line1\r\nline2\r\nline3\r\nline4");
        screen.feed(b"\x1b[2;1H\x1b[K");
        screen.feed(b"\x1b[1;3H\x1b[2@");
        screen.feed(b"\x1b[4;1H\x1b[2P");
        let rows: Vec<String> = plain(&mut screen)
            .iter()
            .map(|r| r.trim_end().to_string())
            .collect();
        assert_eq!(rows, vec!["li  ne1", "", "line3", "ne4"]);
        screen.feed(b"\x1b[2J\x1b[H");
        assert!(plain(&mut screen).iter().all(|r| r.trim().is_empty()));
        assert_eq!(screen.cursor(), (0, 0));
    }

    #[test]
    fn alternate_screen_restores_main_content() {
        let mut screen = Screen::new(10, 3, 10);
        screen.feed(b"main\r\n");
        screen.feed(b"\x1b[?1049h\x1b[HALT");
        assert!(screen.alt());
        assert_eq!(plain(&mut screen)[0].trim_end(), "ALT");
        screen.feed(b"\x1b[?1049l");
        assert!(!screen.alt());
        assert_eq!(plain(&mut screen)[0].trim_end(), "main");
    }

    #[test]
    fn wide_characters_take_two_columns() {
        let mut screen = Screen::new(6, 2, 0);
        screen.feed("你好x".as_bytes());
        assert_eq!(plain(&mut screen)[0].trim_end(), "你好x");
        assert_eq!(screen.cursor(), (5, 0));
    }

    #[test]
    fn modes_needed_by_the_host_are_tracked() {
        let mut screen = Screen::new(10, 3, 0);
        assert!(!screen.app_cursor());
        assert!(!screen.bracketed_paste());
        assert!(screen.cursor_visible());
        screen.feed(b"\x1b[?1h\x1b[?2004h\x1b[?25l");
        assert!(screen.app_cursor());
        assert!(screen.bracketed_paste());
        assert!(!screen.cursor_visible());
        screen.feed(b"\x1b[?1l\x1b[?2004l\x1b[?25h");
        assert!(!screen.app_cursor());
        assert!(!screen.bracketed_paste());
        assert!(screen.cursor_visible());
    }

    #[test]
    fn scrollback_spans_more_rows_than_one_screen_in_order() {
        let mut screen = Screen::new(20, 3, 100);
        for i in 1..=20 {
            screen.feed(format!("line-{i}\r\n").as_bytes());
        }
        let rows: Vec<String> = screen
            .scrollback_lines(100, false, false)
            .iter()
            .map(|r| r.trim_end().to_string())
            .filter(|r| !r.is_empty())
            .collect();
        let expected: Vec<String> = (1..=20).map(|i| format!("line-{i}")).collect();
        assert_eq!(rows, expected);
        assert_eq!(screen.history_len(), 18);
    }

    #[test]
    fn replay_reconstructs_history_screen_cursor_and_modes() {
        let mut screen = Screen::new(10, 3, 100);
        screen.feed(b"one\r\ntwo\r\nthree\r\nfour\x1b[?2004h\x1b[?25l");
        let replay = String::from_utf8_lossy(&screen.replay_bytes(100)).into_owned();
        assert!(replay.starts_with("one\r\n"), "{replay:?}");
        assert!(replay.contains("\x1b[H"));
        assert!(replay.contains("\x1b[2K"));
        assert!(replay.contains("four"));
        assert!(replay.contains("\x1b[?2004h"));
        assert!(replay.contains("\x1b[?25l"));
        assert!(
            replay.contains("\x1b[3;5H"),
            "光标应在第 3 行第 5 列: {replay:?}"
        );
        let mut again = Screen::new(10, 3, 100);
        again.feed(&screen.replay_bytes(100));
        assert_eq!(plain(&mut again), plain(&mut screen));
        assert_eq!(again.cursor(), screen.cursor());
        assert!(again.bracketed_paste());
        assert!(!again.cursor_visible());
    }

    #[test]
    fn the_model_answers_queries_and_records_the_title() {
        let mut screen = Screen::new(10, 3, 0);
        screen.feed(b"\x1b[c\x1b]2;hello\x07");
        let answer = screen.take_responses();
        assert!(answer.starts_with(b"\x1b[?"), "DA1 应答: {answer:?}");
        assert!(screen.take_responses().is_empty());
        assert_eq!(screen.title(), "hello");
    }

    #[test]
    fn synchronized_output_is_held_until_the_end_marker() {
        let mut screen = Screen::new(10, 3, 0);
        screen.feed(b"\x1b[?2026hHELLO");
        assert!(screen.sync_deadline().is_some());
        assert!(plain(&mut screen)[0].trim().is_empty());
        screen.feed(b"\x1b[?2026l");
        assert!(screen.sync_deadline().is_none());
        assert_eq!(plain(&mut screen)[0].trim_end(), "HELLO");
    }

    #[test]
    fn replaying_an_empty_screen_does_not_shift_later_output() {
        let empty = Screen::new(10, 3, 100);
        let mut again = Screen::new(10, 3, 100);
        again.feed(&empty.replay_bytes(100));
        again.feed(b"READY\r\n");
        assert_eq!(
            again.history_len(),
            0,
            "replay must not push rows into history"
        );
        assert_eq!(plain(&mut again)[0].trim_end(), "READY");
        assert_eq!(again.cursor(), (0, 1));
    }

    #[test]
    fn a_wide_character_at_the_last_column_does_not_break_the_model() {
        let mut screen = Screen::new(5, 2, 10);
        screen.feed("abcd你x".as_bytes());
        screen.resize(3, 2);
        screen.feed(b"\x1b[2J\x1b[H ok");
        assert_eq!(screen.resets(), 0);
        assert!(plain(&mut screen)[0].contains("ok"));
    }
}
