# 替换完成后的运行与回退清单

生产替换已完成，旧 Python 项目与部署已退休。当前服务与能力以
`/api/meta`、[capabilities.md](capabilities.md) 和 [environment.md](environment.md)
为准。日常发布不恢复旧源码、服务、端口或代理入口。

## 1. 发布前

1. 按 [deployment.md](deployment.md) 核对舰队清单、工具链和测试环境。
2. 原生 CLI 根与 SessionDock 的 state、host、lifecycle、audit、trash 等目录
   保持当前配置；普通读取不修改原生历史。
3. 用临时 fixture 和回环监听验证改动，页面行为必须经过 Chromium 的真实操作。
4. Python oracle 差分工具已于 2026-10-06 删除；验证不依赖旧生产服务或备份源码。

## 2. 发布与验收

1. 使用 `python3 deploy/deploy.py deploy --all` 构建、验证、发布、重启并健康检查。
2. 保留 SessionDock 的状态、原生历史和所有现存 ptyhost；只重启 Web 服务。
3. 通过 `python3 deploy/fleet_status.py` 核对可达节点的版本、运行状态与部署标记。
4. 反向代理继续使用认证后的 `/sessiondock/`，不恢复退休入口。
5. 功能验收与套件选择见 [validation.md](validation.md)。

## 3. 回退

1. 使用部署工具的 `rollback` 恢复先前 SessionDock 的 bin/web，再重启 Web 服务。
2. 确认 `/api/meta`、原生会话与同一批 ptyhost 可用；不恢复旧 Python 入口。
3. 保留现行配置与持久数据，不手工改写 conversation/lifecycle 状态。
4. 发布备份、自动回退和健康检查的合同见 [deployment.md](deployment.md)。

## 4. 退休清理的范围

删除旧源码 checkout、旧 Web/tmux 服务与重启/兼容清理脚本，移除旧代理路由，
将现行链接和运维指令改为 SessionDock。共用的 ptyhost 和原生 CLI 数据独立于
旧 Web 部署，不能随旧服务清理。运行中的会话工作目录按用户选择单独处理。

全局引用审计仅检查源码、文档、脚本和服务/代理配置，排除数据、原生会话、日志、
缓存、备份及凭据。登记证据与历史摘录保留原文；数据库历史列名不是旧服务依赖，
不能为了消除字符串而改写数据库或破坏已登记的会话关系。
