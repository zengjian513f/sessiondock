# ptyhost 开发说明

源码由原项目 `host-rs` 原样导入，当前 Web 服务尚未与它连接。

```sh
cargo build -p ptyhost --locked
cargo test -p ptyhost --locked
```

**注意：原默认目录仍属于旧 AgentHub。** 开发时必须显式指定隔离目录，
不要执行不带 `--dir` 的 list / attach / send / kill 等命令。

```sh
# 从仓库根目录执行；只查看新项目的开发目录。
cargo run -p ptyhost -- --dir .runtime/ptyhost list
```

宿主每会话一个进程；控制连接使用单行 JSON 请求/响应，attach 后使用
`1 字节类型 + 4 字节大端长度 + payload` 帧。该协议不是浏览器 WebSocket。

`src/protocol.rs` 定义协议与帧；`src/transport.rs` 实现 Unix socket 和
Windows loopback TCP；`src/session.rs` 负责 PTY 生命周期。
导入不等于重新完成跨平台验证，需分别在 Linux / Windows / macOS 验证。
