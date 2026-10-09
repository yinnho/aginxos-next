# aginx-call

SIP 实时语音通话 CLI（M48 刀5a 入树；血统=刀1–刀3 spike 原样搬家，
rvoip-sip crates.io 钉 0.3.10）。**纯 bin 面、无 service 单元**——
**agent 外线腿**（#422 / Positioning）：由 agent 经 CLI / 频道工具 /
能力面拨号与守听；通话结束进程即退，不驻留。

历史：曾由本地 `aginx-voice` PTT 状态机「找 X 语音」零脑直拨 spawn；
该人机链已停投，本包保留为 SIP 真身。

## 面

- `aginx-call talk sip:<uri>` — 拨出对讲（env `AGINX_CALL_BIND` 钉
  本机绑定 IP；缺省走 config.local 的 127.0.0.1 腿仅供本机测试）。
- `aginx-call talk` — 无 URI 即守听+自动接听。
- `aginx-call answer` / `aginx-call dial <uri>` — 老面，调试用。
- SIGINT = 优雅 BYE 挂断。
- 媒体端口 17600–17699；编解码协商 opus/PCMA/PCMU/telephone-event。

## 名录

`/etc/aginx/call.conf` 平面文件（`name = sip:aginx@host:5060`
一行一目），刷后灌注不烤入镜像（D14）。agent 查表拨号；查无则失败回执。

## 验证

- host：`cargo build -p aginx-call`（macOS cpal 腿）
- 节点：`AGINX_CALL_BIND=<lan-ip> aginx-call talk sip:aginx@<peer>:5060`
- 中继：见 `docs/SIP-PSTN-中继备忘.md`

## 回滚

`aginx-pkg rollback aginx-call`
