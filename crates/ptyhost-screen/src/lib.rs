//! 终端模型与服务端网格，宿主（ptyhost）和 Web 服务（sessiondock）共用：
//! 实时会话、attach 回放、录制 checkpoint 和录制的网格回放都经由同一个模拟器，
//! 浏览器看到的历史和实时才会一致。

pub mod grid;
pub mod screen;

pub use screen::{Pen, Responder, Screen, push_cell_text};
