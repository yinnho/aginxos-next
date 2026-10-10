# 需求：aginxbrowser 无头开关（/open 不上屏）— REQ-AGB-001

2026-10-10 · 提出：AginxOS（收据 #479）· 对象：aginxbrowser ≥ v0.5.9

## 背景

AginxOS 10-01 裁决为黑匣子 agent 服务器：无屏、无人机交互面。但
aginxbrowser 的面板腿仍活着——`/run/aginxbrowser/show.html` 持屏协议
（panel.rs：文件在=浏览器抢 DRM master 上屏；消失=释放）原为 term 共屏
时代设计，term 已死，**没有任何东西再删 show.html**。后果：任意本机
`POST :8089/open`（模板烟测、显示时代残留行为……）都会点亮屏幕并锁死
到重启（redfin 实案 2026-10-10：08:23 一发 weather 卡亮屏一整天，
肇事调用方至今未抓获——浏览器路由不打日志）。

OS 侧已用堵法过渡（#479）：`aginx-panel-off --hold` 灭屏后**持有 DRM
master 不放**，浏览器的 SET_MASTER 永远 EINVAL（panel.rs「master
busy」500ms 重试不设上限=2 ioctl/s 空转）。该堵法治标：master 被占着，
浏览器面板腿整段白转。

## 需求

给 aginxbrowser 一个无头模式开关，**`/open` 只渲染不上屏**：

- 形态建议：env `AGINXBROWSER_HEADLESS=1`（与既有
  `AGINXBROWSER_ALLOW_PRIVATE_NETWORK` / `AGINXBROWSER_DOWNLOAD_DIR`
  同族），单进程生命期生效；
- 语义：headless 下 `tmpl::open` **不写 show.html**（或写了 panel 线程
  也不启动/不尝试 DRM），`POST /open` 照常渲染模板并回
  `{"ok":true,...}`——模板在册验证（template-gen 烟测门）不受影响；
- `/screenshot`、`/search`、`/fetch` 等腿与面板无关，不动。

## 验收

1. `AGINXBROWSER_HEADLESS=1` 起服务：`POST /open`（任意模板）→ 200
   `ok:true`；`/run/aginxbrowser/show.html` 不出现；DSI connector 保持
   disabled；无 master 重试空转日志；
2. 不设 env：行为与现状完全一致（有屏机器不受影响）；
3. 落地后 AginxOS 侧回撤 #479 堵法（panel-off 回 off-and-exit 或直删
   持屏对抗——见 rcS 注释），unit envs 加该开关。

## 附注

- OS 侧堵法细节与浏览器侧行为实录（master busy 优雅重试、/open 200
  过）见 aginxos-next `rootfs/src/paneloff.c` 头注与收据 #479。
- 本文档安家 OS 仓（对生态件的要求书）；aginxbrowser 仓只读不碰
  （2026-10-09 裁决），实现由该线自行排期。
