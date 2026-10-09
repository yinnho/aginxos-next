# 刀5 章程：语音接线（历史稿 · 人机 PTT 帧）

> **2026-10-10 换挂说明（#422 / Positioning）**：本稿目标「对着手机说
> 『找 Mac 语音』」是**已停投的人机对话框**。现役真源：SIP/PSTN 是
> **agent 外线**（`aginx-call` 由 agent 拨/接）；asr/tts/voice 是该线
> 的嘴耳内件，不再扩本地 PTT/上脸产品。刀5a 入树的 `crates/call` 与
> SIP 资产**保留**。下文原文留作收据，勿按「对手机说」继续开刀。
>
> 现役对接见 `docs/SIP-PSTN-中继备忘.md`、`AGENTS.md` Positioning。

---

# （原文）刀5 章程：语音接线——对手机说「找 Mac 语音」即通（M48 收官）

提出：2026-09-28 晚（用户裁决「做」）
前置：M48 刀1–刀3 全收据（rvoip/Opus 选型、redfin 音频环、
LAN redfin↔Mac 真人对讲 64s 用户认可）；spike 源在
`out/sip-spike/aginx-call/`（gitignored，1194 行）。
状态：待批

## 目标与验收

对着手机说「**找 Mac 语音**」→ 手机直拨 Mac 守听腿 → 接通即对讲
→ 通话中按音量键挂断。全环真人收据（HARDWARE.md）。

不做：真号码/PSTN（M48 边界）、刀4 服务器中继（多机器再开）、
通话录音/转写（后续刀）。

## 架构裁决

- **零脑直拨**：通话是确定性机器动作，同 WiFi join 先例——走
  aginx-voice 封闭词表状态机（protocol.rs）新意图 → `Act::Call`
  直接 spawn aginx-call，**不过 brain**（宪法 09-22「系统即智能体·
  最直接运行」：盒内零 hop，间接只许在真边界）。脑线（call 工具桥）
  明确不做。
- **两端腿**：手机腿=拨出（`aginx-call talk sip:aginx@<mac>:5060`，
  env AGINX_CALL_BIND=本机 LAN IP）；Mac 腿=守听自动接
  （`aginx-call talk` 无 URI 即 listen+answer——spike 已有此面）。
- **挂断**=通话中按音量键（上/下皆可）→ voice 向子进程发 SIGINT
  （spike 的 tokio ctrl_c 臂即 SIGINT，优雅 BYE 挂断）；Mac 侧
  Ctrl+C 同律。
- **名录**：`/etc/aginx/call.conf` 平面文件（`mac = sip:aginx@192.168.3.26:5060`
  一行一目）——机器即数据（D14 精神），刷后灌注不烤个人数据；
  说「找 X」按 X 查表，查无此人落地板话「没找到 X 的号码」。

## 刀法

- **刀5a 入树出包**：spike 搬家成 `crates/call`（main/alsa/dsp/
  talk_dev 原样，macOS cpal 腿保留 cfg 分叉）；依赖 rvoip-sip 走
  crates.io 钉 0.3.x（vendor 回收仅当 crates.io 不可达）；
  `pkgs/aginx-call` 四件套（纯 bin 面+aginxmd sidecar，**无 service
  单元**——CLI 由语音按需拉起，不驻留）。musl 30M static 预期不变。
  check.sh 绿即 commit。
- **刀5b 语音接线**：protocol.rs 新意图（「找 X 语音 / 打给 X / 呼叫
  X」）；Act::Call { target }——查 call.conf、TTS「正在呼叫 X」+
  face 提示、spawn 子进程、通话中音量键=挂断、退出后回 idle 落地
  板话（接通/超时/失败三态，15s 拨号窗内建）。麦克风争用：PTT 采集
  完即放（voice 现行为），aginx-call 裸 ALSA 独占——刀2/3 已证
  可行。voice 包 bump 出包。
- **刀5c Mac 守听腿**：host `cargo build -p aginx-call`（cpal 腿）
  + launchd 常驻 plist（**装前用户点头**：Mac 属用户机）+ 真人全环
  收据：说→拨→对讲→挂断。

## 风险与兜底

- **DHCP 漂移**（Mac .26 非绑定）：v0 手维护 call.conf；失败=拨号
  超时落地板话「没拨通」。后续可接 mDNS 或母体名录。
- **rvoip crates.io 可达性**：不可达则 vendor 入树（同 quircs/
  libjpeg 先例），只 vendor 用到的 sip 面。
- **语音包回归**：protocol.rs 是产品面命脉——刀5b 必须带状态机
  测试（新意图三态+旧意图零扰动）。

## 账

- 刀5a/5b 代码入本仓（check.sh 全绿才 commit）；刀5c 收据入
  HARDWARE.md（只本地）。
- aginx-call 包首次上架镜像随下次窗口（v0.1.0）。
