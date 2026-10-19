# 附属关系核验范围

现行附属关系只有 `nest_parent`；原生 subagent 归属仍由原生历史提供。
自动初始化不覆盖用户显式挂载/解除决定，整组操作保留依赖闭包与阻碍项。
当前数据合同见[元数据](metadata.md)，范围与原生文件见[克隆](session-clone.md)、
[移动](session-move.md)。下表是相关浏览器路径索引，不是某次执行通过记录。

| 范围 | 对应实现与浏览器路径 |
| --- | --- |
| 本地 exec / headless CLI 不再自动附属 | `codex_exec_nest_browser.py` 验证 detached dispatcher、后台发现、扫描、退出和重启 |
| OpenCode 不再识别启动附属 | `opencode_spawn_browser.py` 验证两类发起方、歧义目录、原生子代理和恢复旧会话 |
| SSH 远程 CLI 不再写入附属关系 | 同时恢复 resource-agent 与兼容采集路径，只初始化尚未决定的父关系；`process_links_browser.py` 两种模式验证跨节点、同 SID 隔离、解除后重采样与重启 |
| 整组操作静默忽略缺失或远程父会话 | 以 `nest_parent` 双向计算闭包，恢复阻碍项；`session_mixed_clone_browser.py` 从按钮发起两种失败计划 |
| 部分移动清理漏掉新附属会话 | 恢复组外 `nest_parent` 引用检查，使用被捕获的身份，即使父文件已经进入回收站；`session_bundle_browser.py --peer lyra --move --preserve` 在三种来源的部分清理后新增附属会话，再从页面重试 |
| 旧错误父关系不再按原生时间修复 | 旧推断迁移为待校验的单一父关系，采样时排除比孩子更晚的父；`spawn_chronology_browser.py` 保留真实错误夹具，验证修复、星标和显式用户挂载 |
| 同一对话的续写可能被当成新附属 | 初始化时排除 `continued_in` 后继，修复旧推断；`nest_tree_browser.py` 验证续写、隐藏父级、历史读取与树形展示 |
| 导入丢失解除附属决定 | 转换器携带私有初始化决定；`codex_exec_nest_browser.py` 实际运行转换器，再启动拥有进程证据的服务并点击导入会话，验证扫描和重启不重新挂载 |
| 复制与移动丢失解除附属决定 | 同机重写、跨机导入及接收端合并携带私有初始化决定；`session_mixed_clone_browser.py` 与 `session_mixed_bundle_browser.py --peer lyra` 验证十四会话混合组、两种身份模式、源数据及目标关系 |
| 前缀导入可以覆盖目标已有父关系 | 恢复目标父关系冲突检查；`session_prefix_browser.py --peer lyra` 用真实页面确认，验证三种来源都保留目标父关系和原历史 |

`nest_initialized` 是私有决定标记，不是第二套父关系，API 不返回它。
显式挂载/解除以及已初始化关系为 true；旧推断待原生时间/续写校验时为 false。
缺省不补造用户决定，已丢失的历史操作不按目录或时间猜回。
验证必须保留自动附属、错误时间及用户决定断言，不以放宽预期替代修复。
