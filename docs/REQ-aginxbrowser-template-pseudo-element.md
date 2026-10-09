# 需求：模板金样本伪元素装饰是死的——引擎不画 content，样本在传病

提出：2026-09-28（reminder 模板线收据 #415）
对象仓：`~/Documents/aginx/aginxbrowser`（禁改仓——本件是需求文档，
不是补丁；templates/ 属仓内资产）
状态：**已闭环（样本修落地，2026-10-09 核）**

## 引擎事实（设备侧坐实）

aginxbrowser 渲染引擎**不画 `::before`/`::after` 的 `content`**。设备
`POST :8089/screenshot {url}` 判别页（data: URL）实测：

- `::before{content:"PSBL"; color:accent}` 行——强调色 **0 像素**
- 真元素同色行——强调色 **1481 像素**

Mac 侧同结论（`getComputedStyle(el,'::before').content` 回 `"normal"`，
Range API 全零）。**是引擎事实，不是字体/平台差异**；`body::before`
扫描线/辉光覆盖层同样不落墨。

## 金样本带病清单

`templates/` 里 **note / todo / reply 各带 3 条**伪元素装饰规则，poem
同病（`h1::before ">"`、`.row::before "▸"` 一族）。这些装饰**在生产
在役卡片上今天就不存在**——用户看到的 note/todo 一直在裸奔，只是
没人对着设计稿看。

## 传病实锤

本仓模板生成流水线（template-gen 助理派 codex）令「先读
reply/note/todo 学纪律」——codex 照抄伪元素写法，产出的 reminder 里
用户点名的 `!` 标题前缀**在屏上零像素**（本仓已加硬约束+出件自检堵
住自家流水线，commit 143b018；上游样本不改，病根还在）。

## 需求（二选一或并行，归 aginxbrowser 裁决）

1. **引擎修**：让 `::before/::after` 的 content 真正落墨（含
   `body::before` 覆盖层）——若渲染栈短期做不到，至少文档声明
   「伪元素 content 不渲染」为已知限制。
2. **样本修**：金样本装饰全部改真元素（`<span>`）写法，让参照系
   自身是健康的——下游一切「学纪律」的生成器（本仓 codex 流水线、
   未来他线）才不会继续继承死装饰。

**优先建议样本修**：改动小、立即可止血；引擎修是更大的渲染栈工程。

## 回执（2026-10-09 核）

**样本修已全量落地**：`templates/*.html` grep `::before/::after`——
note/todo/poem 零命中（原「各带 3 条」病已清）；weather/reply/qr 剩
5 处命中全是注释（「真 .fx div 不用 body::before——伪元素装饰旧引擎
不落墨（REQ #415）」），装饰已改真元素写法并留互链。下游「学纪律」
的生成器从此参照系健康。引擎侧「伪元素 content 不落墨」仍是事实
（渲染栈已知限制，样本继续避开即可，不再追引擎修）。

## 附注

- `▸`（U+25B8）在 macOS 宿主字体栈额外缺字形（Android Noto/Droid
  Mono 有）——跨端样本建议避开或用真元素+常规字形。
- 判渲染真相的通道：设备 `POST :8089/screenshot {url}` 出 PNG，
  `POST :8089/open` 验模板在册（收据 #415 有配方）。
