# aginx-voice

SIP 线嘴耳内件（M42 资产 · #422 换挂）：本地 ASR/TTS 优先，缺件可落
brain 云。硬依赖只有 `aginx-asr` + `aginx-tts`——opt-in 本包自动带装
这两件。

**不再**硬依赖：`aginx-ocr` / `aginx-qr` / `aginx-pair`（冻结相机 /
扫码配网脸线）。需要眼/扫码时单独 `aginx-pkg opt-in`；缺件则相关
路径失败，不拖垮嘴耳。

`aginx-call` 不在 depends：SIP 外线另 opt-in；agent 直接调
`aginx-call talk …` 拨号（见 `docs/SIP-PSTN-中继备忘.md`）。

历史人机面（PTT 音量键、脸写 `/run/aginx-voice/face`、term 渲染、
「找 X 语音」零脑直拨）已停投，二进制保留供 SIP 内件与回归，勿按
上脸产品扩需求。

## 验证

- voice 日志 `local=true`（asr/tts 在）
- SIP 路径：opt-in `aginx-call` 后 agent 拨通对讲（不以 PTT 验收）

## 回滚

`aginx-pkg rollback aginx-voice`（首装无 .prev 时报 no_prev，重 sync 即回）
