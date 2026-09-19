# ptyhost 开发说明

源码最初由原项目 `host-rs` 导入；当前 SessionDock Web 服务通过
`ptyhost-client` 连接宿主，launcher 负责创建宿主进程。

```sh
cargo build -p ptyhost --locked
```

**默认目录属于仍在运行的前身服务。** 开发命令必须指定私有 `--dir`。

```sh
# 从仓库根目录执行；只查看新项目的开发目录。
cargo run -p ptyhost -- --dir .runtime/ptyhost list
```

宿主每会话一个进程；控制连接使用单行 JSON 请求/响应，attach 后使用
`1 字节类型 + 4 字节大端长度 + payload` 帧。该协议不是浏览器 WebSocket。

`src/protocol.rs` 定义协议与帧；`src/transport.rs` 实现 Unix socket 和
Windows loopback TCP；`src/session.rs` 负责 PTY 生命周期；`src/screen.rs`
是终端模型（alacritty_terminal）的封装，负责截屏、光标、attach 回放与录制
checkpoint 的序列化，并收集模型对终端查询（DA、DECRQM、颜色查询等）的应答——
有 xterm.js 客户端连着时由客户端应答，否则由模型应答写回 pty；DSR 始终由宿主
在读线程应答。
导入不等于重新完成跨平台验证，需分别在 Linux / Windows / macOS 验证。
