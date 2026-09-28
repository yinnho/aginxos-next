# 需求：aginxbrowser 搜索直连超时瘦身 + 引擎腿并行；panic 死因落盘

提出：2026-09-28（晨报 09-28 08:00 点火失败收据 #414）
对象仓：`~/Documents/aginx/aginxbrowser`（禁改仓——本件是需求文档，不是补丁）
状态：待裁决

## 事故背景

晨报 cron（0 8 * * * Asia/Shanghai）09-28 首次无人值守点火整轮失败。
母体侧 web_search 单调用实测 2m08s–2m18s，300s 预算内跑不完多轮取材；
且 aginxbrowser 进程于取材中途死亡重启（死因不可见）。完整时间线见
`docs/HARDWARE.md` #414。

## 需求一：搜索延迟（结构性）

**现象**（aginxbrowser.log 2026-09-28T00:00–00:04，设备侧）：

- 每个查询打 4–6 个引擎；墙内直连腿全部等满超时才转代理：
  - `huggingface.co` 直连 `<- operation timed out` → retrying via proxy
  - `html.duckduckgo.com` 直连 `<- operation timed out` → retrying via proxy
  - `www.bing.com` 直连返回 geo-substitute 页（"Object moved" → cn.bing.com）
    被 validation 判废 → retrying via proxy
- 结果：单次 /search 调用 2 分钟+（设备日志：00:00:17→00:02:35 一调用
  2m18s；00:02:35→00:04:43 一调用 2m08s 后连接层失败）。

**需求**：

1. 直连超时瘦身：探测性直连腿把超时降到 3–5s（现在是等满 reqwest
   默认/10s 级）；或直连与代理并发竞速取先到。
2. 引擎腿并行化：同查询的多引擎直连腿并行发（现在是逐引擎串行
   超时堆叠）。
3. 失败学习（可选）：域名 N 连败后本次会话直接走代理，不再每查询
   重付直连超时。

**量化目标**：墙内环境单 /search 调用 ≤30s（现 120s+）。

## 需求二：panic/异常死亡落盘

**现象**：00:04:43 aginxbrowser 进程死亡、svc respawn（第 19 次启动）。
三方全无死因记录：

- dmesg 无 OOM kill、无 segfault 痕迹；
- aginxbrowser.log 无 ERROR/panic（最后几行是正常的 search INFO）；
- svc daemon 侧无子进程退出码/信号记录（OS 侧已知缺口，另有本仓
  侧修复并行）。

**需求**：装 panic hook（或 stderr 持久化），进程死亡时 backtrace/
最后 stderr 落盘（如 `{data}/crash.log`），respawn 后可查死因。
