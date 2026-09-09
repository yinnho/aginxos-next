# aginx-voice

语音对话守护（M42a）：PTT=按住音量下、眼=音量上；脸写
/run/aginx-voice/face 由 aginx-term 渲染；一眼自举配网委外
`aginx-pair apply`（C4 薄化）。嘴耳优先本地三件（aginx-asr/aginx-tts/
aginx-ocr——本包 depends 全三），缺件落 brain 云。

## 验证

- voice 日志 `local=true`
- PTT 一轮：识别上脸、点名（「你说给我听」）出声

## 回滚

`aginx-pkg rollback aginx-voice`（首装无 .prev 时报 no_prev，重 sync 即回）
