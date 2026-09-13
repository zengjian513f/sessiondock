# 单机切流清单：Python 节点 → Rust

本清单用于已授权的 SessionDock 切流。以当前二进制、`/api/meta` 和
[capabilities.md](capabilities.md) 为准，不沿用早期迁移批次的禁用清单。

## 1. 前置条件

1. 按 [runbook-dev.md](runbook-dev.md#1-build) 构建 `sessiondock`、`ptyhost`；使用 Hub
   时同时构建 `sessiondock-hub`。Windows 实机必须遵循
   [原生构建与滚动部署流程](deploy-windows.md)，不得在 OpenSSH 会话中调用 rustup shim。
2. 显式配置原生只读根、loopback bind、ptyhost/launcher 和需要持久化的服务目录。
   配置路径可以按 Python 部署的实际布局重叠；cwd 不是文件访问授权。
3. 运行 `sessiondock --check-config`，核对普通监听、节点监听、Hub、终端、delivery、
   lifecycle、metadata、trash、audit 与 bug-report 的有效配置。
4. 文件读取始终可用；文件写入随终端操作能力开启。正常附件上传走文件 API，不需要
   单独的迁移 stub 或 Rust 专属容量规则。
5. Metadata 使用 Python 兼容的每次重载语义。缺失、损坏或未知 schema 按空数据处理；
   不要求空目录、0700、独占锁或离线导入后才能启动。

Python 与 Rust 的变量名称不同，完整映射见 [environment.md](environment.md)。原生 CLI
历史仍只读；ptyhost 子进程可以在核对协议和实例元数据后由两端观察。

## 2. 仍未迁移或有意不同

- 普通 SessionDock 监听仍为 loopback，由反向代理提供公开认证和 TLS。
- Hub 使用独立 `sessiondock-hub`；节点注册仍限允许网络内的字面 IP，支持 HTTP 和
  系统证书验证的 HTTPS/WSS。见 [hub.md](hub.md)。
- 原生历史不由 Web 服务改写；SessionDock 自己的 metadata、delivery、lifecycle、
  trash、audit 和 bug-report 数据按各自格式持久化。
- 搜索已使用 Python 兼容正则，包括 lookaround 和 backreference。未知 source 是空筛选；
  flags 仅在值等于 `1` 时启用。没有 Rust 专属查询、编译或结果总量拒绝。
- 外部与受管实例都支持发现、确认、stop、force takeover 和随后 resume。真实执行仍以
  当前实例身份和 ptyhost 确认为准。
- 未配置其必需后端的独立功能可以返回 501；已实现的发送、retry/discard、终端输入、
  附件、文件、trash、stop 和 takeover 不再列作 stub。

## 3. 影子比对步骤

1. 对操作者授权的原生只读根分别运行 Python 与 Rust。
2. 比较 `/api/sessions`、消息分页、搜索、媒体、终端列表和 metadata 行为；随机 token
   只比较其指向的内容，不比较字面值。
3. 使用 [validation.md](validation.md) 中的 parity 套件记录差异。差异必须能由当前
   Python 源码或真实协议边界解释。
4. 对 HTTP 输入补测 Unicode/falsy 标识、未知字段、长文本、正则、附件、外部实例和
   Hub HTTPS 路径，确认没有旧 Rust-only 拒绝。

## 4. 切流步骤

1. 影子比对通过并保存结果。
2. 记录在途发送与创建操作，避免两个入口为同一请求重复入队。
3. 启动 Rust 服务并核对 `/api/meta`、`/api/nodes` 与页面 capabilities。
4. 验证已有外部/受管会话可发现，force takeover 会撤销旧页面，stop 会等待真实确认，
   resume 会绑定新的实例。
5. 验证文件读取、写入、普通附件上传、delivery outbox 以及 metadata 外部修改重载。
6. 取得用户的最终切流授权后再修改反向代理入口。

## 5. 回退步骤

1. SIGINT/SIGTERM 停止 Web 服务；不要杀独立 ptyhost 子进程。
2. 恢复 Python 入口并核对原会话仍可观察。
3. 保留 SessionDock 自己的持久数据，避免手工编辑 delivery/lifecycle 账本造成重复动作。
4. 核对 Host/Origin、节点凭据和 Hub 路由后再恢复流量。

## 6. 每一步的验收命令

完整命令和并发构建规则见 [validation.md](validation.md)。常用入口包括
`tests/check_docs_links.py`、`tests/route_ledger.py`、各 `*_parity.py`、HTTP/terminal/
lifecycle/Hub 浏览器套件和 `sessiondock --check-config`。生产路径只在明确授权的影子或
切流步骤中使用。
