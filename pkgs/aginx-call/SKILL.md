# aginx-call

SIP 实时语音通话 CLI（M48 刀5a 入树；血统=刀1–刀3 spike 原样搬家，
rvoip-sip crates.io 钉 0.3.10）。**纯 bin 面、无 service 单元**——
由 aginx-voice 状态机按需 spawn（零脑直拨，宪法 09-22），通话结束
进程即退，不驻留。

## 面

- `aginx-call talk sip:<uri>` — 拨出对讲（env `AGINX_CALL_BIND` 钉
  本机绑定 IP；缺省走 config.local 的 127.0.0.1 腿仅供本机测试）。
- `aginx-call talk` — 无 URI 即守听+自动接听（Mac 守听腿用）。
- `aginx-call answer` / `aginx-call dial <uri>` — 老面，调试用。
- SIGINT（音量键由父进程转发）= 优雅 BYE 挂断。
- 媒体端口 17600–17699；编解码协商 opus/PCMA/PCMU/telephone-event。

## 名录

`/etc/aginx/call.conf` 平面文件（`mac = sip:aginx@192.168.3.26:5060`
一行一目），刷后灌注不烤入镜像（D14）。语音侧查表，查无此人=板话
「没找到 X 的号码」。

## 验证

- host：`cargo build -p aginx-call`（macOS cpal 腿）
- 设备：`AGINX_CALL_BIND=<lan-ip> aginx-call talk sip:aginx@<mac>:5060`
- 全环：对手机说「找 Mac 语音」→ 拨通对讲 → 音量键挂断（刀5c 收据）

## 回滚

`aginx-pkg rollback aginx-call`
