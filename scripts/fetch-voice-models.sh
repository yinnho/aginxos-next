#!/usr/bin/env bash
# M42d/bake#14: 语音模型再取 —— 从 k2-fsa/sherpa-onnx releases 经 gh 拉取，
# 摆成烤盘形状 out/voice/models/{asr,tts}。
#   asr = sense-voice-small int8（只留 model.int8.onnx + tokens.txt；
#         发布包含 fp32 937MB 与 test_wavs，刻意不取）
#   tts = kokoro-int8-multi-lang-v1_1 全目录（lexicon/fst/espeak-ng-data 全要）
# 模型不入仓（390MB）；out/ 已 gitignore。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
V="${ROOT}/out/voice"
DL="${V}/dl"
M="${V}/models"
command -v gh >/dev/null || { echo "gh CLI required" >&2; exit 1; }

mkdir -p "${DL}" "${M}/asr" "${M}/tts"

# ---- ASR: sense-voice small int8 ----
if [ ! -s "${M}/asr/model.int8.onnx" ]; then
  gh release download asr-models --repo k2-fsa/sherpa-onnx \
    --pattern 'sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2' \
    --dir "${DL}"
  tar -xjf "${DL}/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2" \
    -C "${DL}"
  cp "${DL}/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/model.int8.onnx" "${M}/asr/"
  cp "${DL}/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/tokens.txt" "${M}/asr/"
fi

# ---- TTS: kokoro int8 multi-lang v1_1 ----
if [ ! -s "${M}/tts/kokoro-int8-multi-lang-v1_1/model.int8.onnx" ]; then
  gh release download tts-models --repo k2-fsa/sherpa-onnx \
    --pattern 'kokoro-int8-multi-lang-v1_1.tar.bz2' \
    --dir "${DL}"
  tar -xjf "${DL}/kokoro-int8-multi-lang-v1_1.tar.bz2" -C "${M}/tts"
fi

# ---- TTS 小模型线（M42e：kokoro 暖合成 7 字句 4.1–4.9s RTF≈2.5 是延迟地板，
# vits-melo ~163MB zh+en 单说话人 / matcha-baker 纯中文。质量由用户听判，
# 默认仍 kokoro，AG_TTS_KIND=vits|matcha 切换）----
if [ ! -s "${M}/tts/vits-melo-tts-zh_en/model.onnx" ]; then
  gh release download tts-models --repo k2-fsa/sherpa-onnx \
    --pattern 'vits-melo-tts-zh_en.tar.bz2' \
    --dir "${DL}"
  tar -xjf "${DL}/vits-melo-tts-zh_en.tar.bz2" -C "${M}/tts"
fi
if [ ! -s "${M}/tts/matcha-icefall-zh-baker/model-steps-3.onnx" ]; then
  gh release download tts-models --repo k2-fsa/sherpa-onnx \
    --pattern 'matcha-icefall-zh-baker.tar.bz2' \
    --dir "${DL}"
  tar -xjf "${DL}/matcha-icefall-zh-baker.tar.bz2" -C "${M}/tts"
fi

rm -rf "${DL}"
du -sh "${M}/asr" "${M}/tts/kokoro-int8-multi-lang-v1_1"
echo "models ready under ${M}"
