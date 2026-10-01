# 交接：主仓会话归属（2026-09-26）

给下一棒：**会话一律开在 `~/Documents/aginxos-next`**（本仓）。一代仓
`~/Documents/aginxos` 已于 2026-09-08 封仓，但它一直是历史会话的默认
目录——本文件终结这个错位。

## 裁决

- 会话启动目录 = 本仓。一代仓只剩两个用途：`.factory` 整机回滚真源、
  灾难回滚收据。它的 `docs/ARCHIVED.md` 是资产地图。
- **漏账警示**：SIP/网络电话线三件 `docs(sip)` 提交（62e25d0 / e14e7b2 /
  f8fc867）落进了一代仓，违「封仓零提交」。历史不搬；**此后 SIP/电话线
  收据写本仓 `docs/HARDWARE.md`**。
- Claude 的 auto-memory 已于 2026-09-26 从一代仓的 project key 复制到
  本仓 key（`~/.claude/projects/-Users-sophiehe-Documents-aginxos-next/`），
  两边同源；以本仓侧为准续写。

## 在役快照（2026-09-26；10-01 追记重定位）

- **redfin**：#388 镜像（结构四刀+刀5，裸 L0 首启自带出厂树）；
  六单元 aginx/aginx-gateway/aginx-secretd/aginx-voice/aginxbrowser/net-watch；
  aginxbrowser v0.5.8（manifest 换钉）；gateway id=redfin、relay 通道活
  （Mac `agc agent://redfin.relay.aginx.net/me` 裸跑即可，agc 6eef4d3 起读
  配置腿）。
- **enchilada**：E5+E6 折叠镜像在役（wifi 自动连 + modem）；回网后须推
  刷新签名 manifest。
- **生态仓** `~/Documents/aginx/`：aginx（5658af7 spool 丢件柜台）、
  agc（6eef4d3 配置腿）、aginx-relay（ff1a949）、aginx-carrier。
- **10-01 重新定位（#422）**：AginxOS=黑匣子 agent 服务器——人机
  交互产品线推翻；语音降 SIP 内件、摄像头冻结、无屏维持；**SIP/PSTN
  升格 agent 对外腿**（backlog=talk 腿 RTP 看门狗+拨号腿 15s CANCEL+
  PSTN 真呼验收，任务 #61）。真源=AGENTS.md Positioning 段。同日
  HARDWARE.md 瘦身：正文只留活窗（滚动纪律在文件头），旧收据归档
  `docs/HARDWARE-ARCHIVE-2026-09{a..d}.md`。Mac 对讲测试台已拆；
  redfin voice 单元下次碰机 down（任务 #65）。

## 账四摊（2026-09-28 追记）

1. **GitHub 未推——已清（09-26 晚）**：aginx-carrier `b3a42c7`/`8c958fd`/
   `97b855e` 上 GitHub（d64f2fb..97b855e）+ 86quan deploy 腿同步
   （9e8ddd2..97b855e 快进 33 件）；aginx-relay GitHub 仓**新建私仓**
   `yinnho/aginx-relay` 并推 `ff1a949`（deploy 腿本已在 ff1a949）。
2. **本仓未推——已清（09-28 二批）**：09-26 后新积的 24 件代码+文档
   （v0.1.8 后至 aginx-pair v0.1.1，收据 #397–#418 期间的码）经第二次
   历史重排整批上推（`398efc5..7dc055f`）；24 件 docs(hardware) 收据
   （#397–#418）按纪律永不上推，重排后**垫在本地 master 顶**（本地
   ahead 24=纯收据堆）。推送姿势不变：
   `git push origin <代码尖>:master`。**教训：重排后落收据须等价垫顶
   ——09-26 后新工作直接在 master 交错落码与收据，推送窗又被顶回
   重排；此后收据仍按「先码后收据」顺序落**。（10-01 又一轮重排：
#421 两件+#422+账本瘦身四件垫顶，代码尖 4295cc9 已推，本地
ahead 4=纯收据堆。）
3. **一代仓工作树——已清（09-26 晚）**：~~ag-asr.c zh 修~~（已搬本仓
   v0.1.1）原件在一代仓封存落账（10c6aa9）；~~204 件 target/ 删除~~
   已提交（2fc2cd3）。「封仓零提交」由用户 09-26 裁决破例收尾件——
   **仍不推送**（local only）；21 件 untracked 散件（apk/dmg/ARCH.md/
   SCIS 设计稿）为他线工作件，不碰。
4. **运行面（09-28 现况）**：镜像三件上架（aginx v0.1.13/gateway
   v0.2.0/pair v0.1.1）+redfin manifest 换钉+pair 升 v0.1.1（收据
   #417）；enchilada 不在线（relay 5 天零注册，收据 #418）回网再
   换装；明晨 08:00 晨报=降级卡真考（#414 三堵口后首验）。

## 指针

宪法=`AGENTS.md`+`docs/FS.md`；实验账=`docs/HARDWARE.md`（只本地提交）；
外部观察=`docs/WATCHLIST.md`；各线交接=`docs/HANDOFF-*.md`。
