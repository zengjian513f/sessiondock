//! 终端模型与服务端网格，宿主（ptyhost）和 Web 服务（sessiondock）共用：
//! 实时会话、attach 回放、退出时保存的最终画面及其网格渲染都经由同一个模拟器，
//! 浏览器看到的历史和实时才会一致。

pub mod grid;
pub mod screen;

pub use screen::{Pen, Responder, Screen, push_cell_text};
