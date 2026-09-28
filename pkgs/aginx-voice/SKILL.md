# aginx-voice

语音对话守护（M42a）：PTT=按住音量下、眼=音量上；脸写
/run/aginx-voice/face 由 aginx-term 渲染；一眼自举配网委外
`aginx-pair apply`（C4 薄化）。嘴耳优先本地三件（aginx-asr/aginx-tts/
aginx-ocr——本包 depends 全三），缺件落 brain 云。

刀5b（v0.2.2，M48 刀5）：语音直拨——「找X语音 / 打给X / 呼叫X」
零脑直拨（封闭词表本地，不进 brain），查 /etc/aginx/call.conf 名录后
spawn `/var/bin/aginx-call talk <uri>`（通话中音量上/下=挂断）。
aginx-call 不在 depends：需另 opt-in；缺包说「呼叫失败」。
名录一行一目（`mac = sip:aginx@192.168.3.26:5060`），刷后灌注。

## 验证

- voice 日志 `local=true`
- PTT 一轮：识别上脸、点名（「你说给我听」）出声
- 直拨：说「找mac语音」→ 日志 `call mac -> sip:...`、call.log 有 rvoip 走线

## 回滚

`aginx-pkg rollback aginx-voice`（首装无 .prev 时报 no_prev，重 sync 即回）
