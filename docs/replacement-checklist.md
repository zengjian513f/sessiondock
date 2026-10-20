# 运行与回退清单

服务能力以 `/api/meta`、[capabilities.md](capabilities.md) 和
[environment.md](environment.md) 为准。交付范围与完成条件以
[AGENTS.md](../AGENTS.md#scope-and-boundaries) 为准。

## 1. 发布前

1. 按 [deployment.md](deployment.md) 核对完整目标集合、工具链和测试环境。
2. 原生 CLI 根与 SessionDock 的 state、host、lifecycle、audit、trash 等目录
   保持当前配置；普通读取不修改原生历史。
3. 用临时 fixture 和回环监听验证改动，页面行为经过 Chromium 的实际操作。
4. 用 `deploy --dry-run` 核对产物、目标和测试计划，发布同一已提交快照。

## 2. 发布与验收

1. 使用 `python3 deploy/deploy.py deploy --targets <全部相关目标>`；纯前端加
   `--web-only`。纯规则、文档、测试或部署脚本变更不发布未变化的运行产物。
2. 保留 SessionDock 状态、原生历史和现存 ptyhost；只重启受影响服务。
3. 对完整范围逐项记录结果。共享前端/服务发布至少有一个节点和 Hub 部署、重启并健康；
   其他离线或失败项明确记为待补发，不以最低成功数量缩减目标。
4. 用 `python3 deploy/fleet_status.py` 只读核对版本、运行状态与部署标记。
5. 反向代理保持认证后的 `/sessiondock/`；功能验收见 [validation.md](validation.md)。

## 3. 回退

1. 使用部署工具的 `rollback` 恢复备份产物并重启相关服务。
2. 确认健康接口、原生会话与同一批 ptyhost 可用。
3. 保留现行配置与持久数据，不手工改写 conversation/lifecycle 状态。
4. 发布备份、自动回退和健康检查的合同见 [deployment.md](deployment.md)。

## 4. 服务退役与代理变更

服务退役或改变反向代理流量须用户明确授权。共用的 ptyhost、原生 CLI 数据和运行中
会话的工作目录分别核对范围，不能随 Web 服务清理。引用审计检查源码、文档、脚本及
服务/代理配置，排除运行数据、会话、日志、缓存、备份和凭据；不为清理字符串改写数据库。
