# voice — 本地 ASR/TTS CLI（M42d，bionic-static）

`ag-asr` / `ag-tts`：sherpa-onnx v1.13.7 + onnxruntime 1.27.1 的自包含静态
CLI，供 voiced（`crates/voiced`）做本地优先的听/说。设备收据在
`docs/HARDWARE.md` M42d 条目。

## 形状

- `ag-asr <in.wav>` → stdout 出识别文本。sense-voice-small **int8**（239MB，
  `/var/models/asr/`），中英日韩粤，sherpa 内部重采样到 16k。
- `ag-tts <text> [out.wav]` → stdout 出 wav 路径。kokoro **int8 multi-lang
  v1_1**（`/var/models/tts/kokoro-int8-multi-lang-v1_1/`，sid=8 中文女声，
  出 24k mono——voiced 侧升采样 48k 立体声后走 snd-play）。
- `AG_ASR_DIR` / `AG_TTS_DIR` / `AG_TTS_SID` 环境变量可覆盖缺省路径。
- **线程数写死 2**：big.LITTLE 上 2T 最优（4.95s 句 2T=38.5s / 3T=40.1s /
  4T=43.1s），加线程反降。

## 构建

```
scripts/fetch-voice-models.sh   # 模型 → out/voice/models（~390MB，不入仓）
scripts/build-voice.sh          # CLI   → out/voice/bin/ag-{asr,tts}（各 26MB）
```

构建产物与工作树全在 `out/voice/`（gitignored）。删除后按上面两脚本完整再生。

## bionic-static 工艺（为什么这几个怪招都在）

1. **ORT 用预编译静态库**：csukuangfj/onnxruntime-libs v1.27.1
   `onnxruntime-android-arm64-v8a-static_lib`（sherpa 官方 CI 同源；v1.13.7
   的官配版本，见其 build-android-arm64-v8a.sh:93）。
2. **sherpa cmake 只认环境变量** `SHERPA_ONNXRUNTIME_LIB_DIR/INCLUDE_DIR`——
   `-D` 会被静默无视，cmake 走 download 分支后在 Android 上报
   "Only support Linux, macOS, and Windows"。
3. **库编 android-21、终链 android-29，两头都不能换**：
   - API 29 编库撞 `nnapi_provider_factory.h` 缺头（静态 zip 不带）；
   - API 21 终链的静态二进制机上 abort "TLS segment underaligned:
     alignment 8, needs ≥64"。
4. **android-shims.c**：NDK 不发 liblog/libandroid/libdl 的静态版——
   `__android_log_*` 转 stderr（保 sherpa 诊断日志），`AAsset*`/`dl*` 是
   死码桩（模型全走文件路径、provider=cpu 内建）。
5. **strip 必做**：裸链 ~780MB 全是 debug_info，strip 后 26MB。

## 选型淘汰记录（别再试）

- melo-tts：官方 fp32 在 v1.13.7 下静音垃圾。
- matcha-icefell：发布包无 vocoder，链不动。
- zipvoice：API 硬拒（流式接口，无 offline generate）。
- sense-voice fp32（937MB）：比 int8 无感知增益，白占 4 倍空间。
