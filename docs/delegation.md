# 委派本机 grok CLI（grok-4.6）做简单任务

用户已授权：在本仓库的开发过程中，可以把**边界清楚、可机械验证、独立成文件**的
任务交给本机 `grok` CLI 以 headless 方式完成（全局调用法：`/lyra-share/agent-rules/references/grok-cli.md`）。
默认使用 `grok-4.6`，用户明确指定时按本次指令；产出须人工审阅并按批次记账。
不得让 grok 触碰生产服务、仓库外源码、凭据或运行数据。

## 调用方式

```sh
timeout 1200 ~/.local/bin/grok \
  -p "$(cat task.md)" \
  -m grok-4.6 \
  --always-approve \
  --cwd "$PWD" \
  --max-turns 60 \
  --output-format plain \
  --disable-web-search \
  > grok_task.log 2>&1
```

- `-p/--single`：单轮 headless，输出到 stdout 后退出；任务正文写在单独的
  `task.md` 里再注入，避免引号转义问题。
- `--always-approve`：无人值守，所以任务书必须把"只能新建/修改哪些文件、不许
  跑什么"写死（见模板）。
- `--max-turns`：防止无限试错；60 轮足够写一个 ≤300 行的脚本。
- `--disable-web-search`：任务所需信息全在仓库里时关闭——省 turn、避免把网上做法
  （如自动 `playwright install`）带进来、也不把仓库路径/文件名外发给搜索。需要
  查外部库最新 API 的任务再打开。
- 一律后台运行 + `timeout` 兜底 + 看门狗（日志 5 分钟无增长即报告）；不要前台等。
- 模型 ID 以 `~/.grok/models_cache.json` 为准（当前 `grok-4.6`，支持
  `--reasoning-effort low|medium|high|xhigh`，默认 high）。

## 什么任务适合派给 grok

适合：

- 新建一个独立文件（脚本、fixture 生成器、汇总工具、文档），不改现有代码。
- 规格可以写完整，且验证是机械的（`--list` 输出、固定命令、精确断言）。
- 只依赖标准库或仓库已有依赖，≤300 行，不需要理解多个 Rust 模块之间的约束。
- 不与正在并行工作的子代理共享文件（尤其 `api/mod.rs`、`state.rs`、
  `config.rs`、`web/app.js`、`tests/history_fixtures.py`）。

不适合：

- 发送账本、授权/路径安全、原生分支语义、ptyhost 协议等正确性敏感模块。
- 需要在共享文件上打补丁、需要和其他代理协调顺序的改动。
- 需要跑长时间 `cargo build`（会和并行代理抢 `target/` 锁）或需要浏览器
  联调才能判断对错的任务。
- 需要产品语义判断（legacy UI 行为、Python 差异是 bug 还是安全差异）。

## 任务书模板

```text
You are working in the Rust repository at the current directory. Read AGENTS.md
first. Create exactly ONE new file, <path>, and modify nothing else (no other files,
no git commands, no cargo builds, no commits).

Goal: <一句话>. Requirements:
1. <语言/依赖/行数上限/文件头说明>
2. <精确的功能清单，含默认值与边界>
3. <CLI 参数或接口>
4. <输出格式/退出码>
5. Verify with: <只允许的、秒级的验证命令>. Do NOT run <慢命令>.

When finished, print a short summary of the file's structure and the exact output
of the verification commands.
```

要点：文件路径、禁止事项、验证命令三者缺一不可；规格里的数字（超时、上限、
默认值）写死，不留给模型选择。

## 审阅清单（人工，必做）

1. `git status --short`：产出是否只落在指定文件；越界改动先确认归属，不回滚其他会话的修改。
2. 通读文件：无网络访问、无写仓库外路径、无 `subprocess` 调用未授权命令、
   无新增依赖；风格与相邻脚本一致。
3. 跑任务书里的验证命令，再按改动选择文档检查或覆盖该路径的浏览器套件；
   仓库不含单元测试，也不新增。
4. 在提交或 PR 说明中注明"grok-4.6 headless 产出，人工审阅"；只有确实尚未完成的
   后续工作才写入根目录 `TODO.md`。
5. 出现 `Memory flush started` 之类 grok 自身日志属正常；`GROK_EXIT` 非 0 或
   看门狗报停滞时读日志尾部判断是卡在权限还是任务本身。

## 产出归属

每批在提交或 PR 说明记录实际客户端/模型、产出范围、主审修正与验证结果。
Grok 产出使用“grok-4.6 headless 产出，人工审阅”；用户指定其他模型时记录实际值。
开发指南只记录委派流程与边界。验证和发布遵循 [AGENTS.md](../AGENTS.md)。

待评估工具只记录在 [TODO](../TODO.md)。
