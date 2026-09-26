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

## 在役快照（2026-09-26）

- **redfin**：#388 镜像（结构四刀+刀5，裸 L0 首启自带出厂树）；
  六单元 aginx/aginx-gateway/aginx-secretd/aginx-voice/aginxbrowser/net-watch；
  aginxbrowser v0.5.8（manifest 换钉）；gateway id=redfin、relay 通道活
  （Mac `agc agent://redfin.relay.aginx.net/me` 裸跑即可，agc 6eef4d3 起读
  配置腿）。
- **enchilada**：E5+E6 折叠镜像在役（wifi 自动连 + modem）；回网后须推
  刷新签名 manifest。
- **生态仓** `~/Documents/aginx/`：aginx（5658af7 spool 丢件柜台）、
  agc（6eef4d3 配置腿）、aginx-relay（ff1a949）、aginx-carrier。

## 账四摊（2026-09-26 清点）

1. **GitHub 未推**：aginx-relay `ff1a949`（shared secret 认证+TOCTOU）；
   aginx-carrier `b3a42c7`/`8c958fd`/`97b855e`（aginx-web 改姓、dup 读免钥、
   dup push 双形状）。
2. **本仓 6 件本地未推**：5× docs(hardware) 按纪律永不上推；`fa26fd3`
   （manifest 换钉 v0.5.8）夹在中间——推它必带 4 件文档，破例连推或留
   本地，待裁决。
3. **一代仓工作树**：`tools/voice/ag-asr.c` zh 默认修（09-18，完整未提交）；
   204 件 `legacy/aginx-os/target/` 删除未提交；untracked 散件（apk/dmg/
   ARCH.md/SCIS 设计稿）为他线工作件。
4. **运行面**：服务器腿（86quan）router 换装状态未知（晨报三杀两杀在
   服务器；Mac 本机腿已换，`~/.aginx/spool/me` 有落件）；明早 08:00 晨报
   复验；enchilada manifest 挂账同上。

## 指针

宪法=`AGENTS.md`+`docs/FS.md`；实验账=`docs/HARDWARE.md`（只本地提交）；
外部观察=`docs/WATCHLIST.md`；各线交接=`docs/HANDOFF-*.md`。
