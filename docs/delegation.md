# 委派本机 grok CLI（grok-4.6）做简单任务

用户已授权：在本仓库的开发过程中，可以把**边界清楚、可机械验证、独立成文件**的
任务交给本机 `grok` CLI 以 headless 方式完成（全局调用法：`~/.claude/grok-cli.md`）。
这是委派编码工作，模型自定；grok 产出仍须人工审阅并按批次记账。
不得让 grok 触碰生产服务、原 Python 仓库、凭据或运行数据。

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
  `config.rs`、`legacy-web/app.js`、`tests/history_parity.py`）。

不适合：

- 发送账本、授权/路径安全、原生分支语义、ptyhost 协议等正确性敏感模块。
- 需要在共享文件上打补丁、需要和其他代理协调顺序的改动。
- 需要跑长时间 `cargo test/build`（会和并行代理抢 `target/` 锁）或需要浏览器
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

1. `git status --short`：产出是否只落在指定文件；有越界改动直接还原。
2. 通读文件：无网络访问、无写仓库外路径、无 `subprocess` 调用未授权命令、
   无新增依赖；风格与相邻脚本一致。
3. 跑任务书里的验证命令，再跑一个真实子集（如 `--only node_contracts`）。
4. 在提交或 PR 说明中注明"grok-4.6 headless 产出，人工审阅"；只有确实尚未完成的
   后续工作才写入根目录 `TODO.md`。
5. 出现 `Memory flush started` 之类 grok 自身日志属正常；`GROK_EXIT` 非 0 或
   看门狗报停滞时读日志尾部判断是卡在权限还是任务本身。

## 已委派记录

| 日期 | 任务 | 结果 |
| --- | --- | --- |
| 2026-09-18 | `legacy-web/grid/facade.js`（xterm.js 兼容的 `GridTerm` 外观层）与 `tests/grid_facade_contract.mjs` | grok-4.6 headless 产出，人工审阅。691 s，rc=0，11 例合同测试通过；人工补充 `onClipboard`（OSC 52 经网格协议转发）。主页面 term.js 接线、设置项、录制网格回放、分页与超链接由主审实现。 |
| 2026-09-18 | 服务端网格（wp-record）8 本任务书：`legacy-web/grid/{wire,model,render,input}.js` 四个 ES 模块与两份 node 合同测试、`legacy-web/grid.{html,js,css}` 页面、`crates/ptyhost/tests/host_grid.rs`、`tests/terminal_grid_browser.py`、`docs/terminal-grid.md` | grok-4.6 headless 产出，人工审阅。8/8 rc=0，单任务 450–1531 s（浏览器套件因滚动区行数期望反复调试最久）；人工审阅未改动模块与页面代码；协议、宿主接线、ptyhost-client `AttachMode`、sessiondock 透传、alacritty_terminal 模型切换由主审实现。 |
| 2026-09-18 | 终端录制（wp-record）9 本任务书：`ptyhost-record` 的 format / sanitize / store / reader 四个模块、`legacy-web/records.{html,js,css}` 回放页、`crates/ptyhost/tests/host_record.rs`、`tests/term_records_http_suite.py`、`tests/terminal_records_browser.py`、`docs/terminal-records.md` | grok-4.6 headless 产出，人工审阅。9/9 rc=0，单任务 420–900 s，最多 3 路并行；人工修正：records.css 删去抄自 files.css 的无关选择器、records.js 的节点前缀改写以免路由台账误判、ptyhost-record 按 clippy 清理 5 处风格提示；grok 自行纠正了 sanitize 任务书里一处期望值笔误（普通字节 `b` 应保留）。ptyhost 录制接线、sessiondock 路由/流式回放、主页面入口由主审实现。 |
| 2026-09-17 | 3 路：输入状态画面夹具、前端契约脚本、输入状态文档 | grok-4.6 headless 产出，人工审阅。修正夹具两处暂态预期、契约脚本对未规定默认码的断言及文档错误码说明；补充乱序响应回归。核心状态判定与发送改动由主审实现。 |
| 2026-09-12 | `tests/run_validation.py` 串行验证运行器 | 约 6 分钟，233 行，一次通过，人工审阅未改动；`--list` 解析 35 套 |
| 2026-09-12 | 第三～五波共 48 个任务（探针、走查套件、生成式参考文档、模块文档、HTTP 合同套件、替换辅助工具）| 全部 rc=0；每波审阅后分别提交（`ad853b6`、`20ded2c`、`5adf7a3`、`1d1abcd`）；仅两处人工修正（能力键比对读 lib.rs、`route_ledger` 识别 delete 路由）。载荷最高时 10 路并行，单任务 165–748 秒 |
| 2026-09-12 | q80 `tests/send_http_suite.py`（第三十一/三十二批四条发送路由的 HTTP 合同，两家假 CLI，10 场景）| 1219 s 后 **max turns**（rc=1）：文件已写到 260 行、前 3 场景通过；人工收尾两处——假 Claude CLI 在 `--resume` 时改为从文件末尾记录续 `parentUuid`（否则第二个根不在活动时间线上、永不确认），以及 retry 对已确认行按实际契约期望 404 并补 `_build`；之后 10/10 通过。教训：10 个场景 + 两家 CLI 超出 60 轮预算，应拆成两本 |
| 2026-09-12 | q79 `tests/check_config_suite.py`（`sessiondock --check-config` 的 18 例启动校验矩阵：bind、空/非目录根、私有目录重叠/嵌套、delivery/file roots/codex index 规则、symlink 别名在 validate 层放行）| 364 s，237 行，rc=0，一次通过；人工审阅未改动 |
| 2026-09-12 | q78 `tests/session_stop_http_suite.py`（`session/stop` 的 HTTP 合同：能力、404/501/400、graceful < 2.4 s、request_id 重放/冲突、already_exited、live、无 lifecycle 501）| 398 s，215 行，rc=0，9 场景一次通过；人工审阅未改动 |
| 2026-09-12 | q77 `docs/validation.md` 套件表按 `run_validation.py --list` 重生成（68 → 表 67 行，去掉一行并行子代理的辅助脚本）| 380 s，rc=0；命令列与 `--list` 无差异，人工只删一行 |
| 2026-09-12 | q75 `tests/meta_import.py`（元数据离线转换并用临时服务验证）、q76 `tests/cutover_preflight.py`（封装 `sessiondock --check-config`） | 837 s / 747 s；rc=0；人工修正缺时间戳的 `starred:true`；见 replacement checklist §4 |
| 2026-09-12 | q74 `tests/shadow_compare.py`（M8 生产只读影子比对工具，操作者手动运行；接力波最后一个任务）| 978 秒，260 行（任务书上限），rc=0；grok 自己把 336 行压到限内；合成语料 10 PASS/3 DELTA/0 DIFF、无 flag exit 2 复验通过；人工审阅未改动，记入 `docs/replacement-checklist.md` §3 |
| 2026-09-12 | 8 个并行任务：`tests/bench_summary.py`、`tests/migration_history_status.py`（原 `plan_status.py`）、`docs/validation.md`、`tests/check_docs_links.py`、`tests/legacy_asset_diff.py`、`tests/route_ledger.py`、`tests/rss_watch.py`、`run_validation.py --json/--rerun-failed/--dry-run` | 165–454 秒各自完成，8/8 一次通过验证命令；人工审阅只改了 `docs/validation.md` 里对临时样例路径的引用。bench_summary 复算结果与第十九批手工表一致；check_docs_links 对仓库 89 个链接 0 坏链并能识别人工坏链 |

## 并行经验（8 路）

- 8 个 headless 进程同时跑没有问题：主要在等 API，CPU/内存可忽略；每个任务
  书独立、产出文件互不相同，`git status` 审阅时不需要拆分。
- 任务书里把样例输入放到 gitignored 的 `target/grok-samples/` 下，验证命令就
  能是秒级且确定的；但文档类产出不要引用这些临时路径（审阅时要改）。
- 用一个 `run_all.sh` 逐任务后台启动并写 `status.log`，配一个看门狗监视
  "日志无增长 10 分钟"即可；全部 8 个在 3–8 分钟内完成。

## 候选任务（待派）

- `tests/fixture_stats.py`：统计各 Python 套件生成的合成语料规模（会话数、
  记录数、字节），供文档引用。
- 将 `docs/*.md` 中重复的预算数字（32 MiB、256 项等）抽成一张
  `docs/budgets.md` 总表并由脚本核对源码常量。
