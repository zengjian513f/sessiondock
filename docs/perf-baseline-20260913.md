# 基准基线 2026-09-13 08:30（修复包落地前，生产实例 8741 vs Python 8710，真实数据只读）

API（api_bench.py，n=3 中位数）：列表 8.5/49 ms；live 11/178；trash 1/15；messages 中位 3.5/29、p90 16MB 139/697、49MB 311/2040、385MB 800/7758；append 0.9/10；term/list 62/38；搜索 36–38 s / 1–2 s；8 并发列表 Rust 4×503（100 ms）vs Python 全 200（928 ms）；8 并发 p90 详情 Rust 4×503（203 ms）vs Python 全 200（8.2 s）。
浏览器端到端（browser_bench.py，1st/2nd）：页面→列表 478/573 vs 2360/2173 ms；打开中位 757/598 vs 453/510；p90 727/648 vs 678/594；49MB 974/813 vs 829/1059；搜索 38.2/39.3 s vs 2.0/1.3 s。
内存：Rust 冷启 87 MB → 列表 113 → p90 210 → 49MB 533 → 385MB 1226 → 385MB 热读 1397 → 一次搜索后 1762 MB；monkey 风暴后 2450 MB。Python 运行 9.5 h：1.4 GB。
空闲 CPU（页面停在活动会话 60 s）：Rust 25% 一核（tokio 工作线程；40 s 内 ~120 请求），Python 48%（含它同时服务的其它标签页）。
JS 采样：打开中位会话 Rust `layoutSessionHead` 113 ms / `renderSide` 21 ms vs Python 48 / 10 ms——列表 808 行 vs 391 行（debug-run 未过滤）。
