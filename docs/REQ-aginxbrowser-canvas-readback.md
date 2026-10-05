# REQ：aginxbrowser canvas 位图回读（toDataURL 恒返空图）

- 日期：2026-10-05
- 需求方：aginxos-next（淘宝扫码登录真考，收据 #445）
- 收件方：aginxbrowser 生态仓
- 性质：需求文档（本仓不碰生态仓源码；设备侧运维动作不受限）

## 现象

淘宝登录页（login.taobao.com/havanaone）扫码二维码画在 528×528 的
`<canvas>` 上。经 `/session/<id>/eval`：

- DOM 枚举稳定看到该 canvas（width=528、height=528、可见）；
- 对同一元素 `toDataURL('image/png')` **恒返回 300×150 的空白 PNG**
  ——300×150 恰是未显式设宽高的 canvas 默认尺寸；
- 多次重试一致，非时序竞态；导出的 PNG 全黑近空。

结论：引擎的 canvas 2D 位图（绘制产物）没有进 toDataURL 的回读
路径——元素几何（width/height 属性）在，位图内容丢失。

## 影响

任何「扫码登录/验证码画在 canvas」的站点，agent 无法从引擎侧取到
码图本体。当前各仓的绕法是偷 token 本地重画（XHR 钩+generate 接口），
能用但每个站点都要逆向一遍协议。

## 需求

`toDataURL`/`getImageData` 回读 canvas 实际位图；至少覆盖 2D context
的 fillRect/drawImage 常规绘制路径。若引擎 canvas 渲染走的是离层
（GPU/合成器），希望在 eval 面暴露一个受控的回读入口（哪怕显式
`sessionReadback` 一类新 API），不必完整走 2D 规范。

## 交叉引用

- 本仓收据：docs/HARDWARE.md #445（仅本地）
- 绕法配方（可复用）：eval XHR send/open 钩偷页面原生轮询体 →
  ck token → 本地画码 → 页面原生轮询完成登录
