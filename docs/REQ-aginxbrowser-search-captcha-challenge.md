# REQ：aginxbrowser 搜索腿 CAPTCHA / 挑战页 / bing_news 验证噪声

日期：2026-10-13　提出方：AginxOS 运维（redfin 研究引擎在役）
对象仓：`~/Documents/aginx/aginxbrowser`（**禁改仓**——本件是需求文档，不是补丁）
设备现状：aginxbrowser **0.5.41**（已含海外超时/代理路径）
相关：`docs/REQ-aginxbrowser-search-latency-panic.md`（直连超时堆叠，未覆盖本条）
状态：待生态仓裁决

## 边界

源码改动只属 aginxbrowser 维护方。AginxOS / aginxresearch 会话**不**改
browser 仓（见 Claude memory `feedback-do-not-touch-aginxbrowser`）。
设备侧仅可运维换钉、写本 REQ。

## 现象（redfin `/var/log/aginx-svc/aginxbrowser.log`，2026-10-12 夜～10-13）

### 1. 搜狗 / 百度 CAPTCHA → 整腿挂起

```
sogou_wechat: reqwest search hit CAPTCHA
engine sogou_wechat suspended for 3600s (CAPTCHA #1)
engine sogou suspended for 300s (CAPTCHA #1)
engine baidu suspended for 300s (CAPTCHA #1)
```

研究循环每题多引擎搜索；CAPTCHA 后该引擎长时间不可用，有效命中面变窄，
逼引擎更依赖其它腿（并放大下面 bing 噪声）。

### 2. bing_news：直连 geo 替代 + 代理仍 Object moved / challenge

```
direct body failed validation … Object moved … (geo-substituted?)
retrying via proxy
proxy body failed validation … Object moved to https://www.bing.com/news/...
engine bing_news transient error: proxy: body failed validation too (challenge/portal on both paths)
```

`cn.bing.com/news/infinitescrollajax` 在直连与代理路径均返回
`Object moved` / 挑战门，validation 双杀 → 该引擎对研究几乎恒废，但仍
占超时与日志噪声（与 latency REQ 的「直连等满再转代理」叠症）。

### 3. 海外 / SOCKS 抖动（0.5.41 已有 auto-proxy，仍见握手失败）

```
direct skipped: overseas auto-proxy or recent-failure list
proxy: … SOCKS error: io error during SOCKS handshake
```

代理路径存在时仍偶发握手失败；与 CAPTCHA 叠在一起时，单次 /search
有效引擎数可掉到很少。

### 4. 下游研究侧可见效应

`/home/research/logs/*.log`：材料条数靠其它引擎硬撑；偶发抓到明显
噪声页（如 Microsoft 文件资源管理器、`Port seems to be open` 扫描页）——
引擎侧可做 URL 黑名单（另改 aginxresearch），**不能代替** browser
搜索腿变干净。

## 期望行为

1. **CAPTCHA 策略可配**：挂起时长、是否在 CAPTCHA 后本会话对该引擎
   降权/跳过而非固定 300s/3600s；或暴露 `/status` 字段让调用方知道
   「当前可用引擎集合」。
2. **bing_news**：对 `Object moved` / infinitescrollajax 挑战页快速
   失败（≤2s），或墙内默认禁用该腿 / 改用可解析的 bing 网页搜索；
   避免双路径都 validation 失败仍打满超时。
3. **挑战页分类**：validation 失败原因区分 geo-substitute / captcha /
   portal，便于调用方与日志聚合（现多为同一句 `body failed validation`）。
4. **量化**：研究负载下（4 实例轮磨），任意 10 分钟窗口内 CAPTCHA 挂起
   占比与 bing_news 双路径失败率有可观测计数（不必立刻根治反爬）。

## 非目标

- 不要求破解 CAPTCHA / 打码。
- 不要求 AginxOS 会话改 browser 源码或私自出 fork 钉。

## 验收建议（生态仓）

- 复现：redfin 上对研究高频查询打 `/search`，对照本 REQ 日志模式。
- 改后：同查询下 sogou/baidu CAPTCHA 挂起可观测且有退避说明；
  bing_news 要么出结果、要么快速声明 unavailable，不再刷屏 Object moved。
