#!/usr/bin/env bash
# M42d: 语音栈一键构建 —— 铺 out/voice 工作树并产出 out/voice/bin/ag-{asr,tts}。
# 依赖：gh（GitHub 访问唯一通道）、NDK 27.0.12077973、cmake。
# 首次跑全量构建 ~10 分钟；树已在则各步幂等快道。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
V="${ROOT}/out/voice"
S="${V}/sherpa-onnx-src"

NDK="$HOME/Library/Android/sdk/ndk/27.0.12077973"
test -d "${NDK}" || { echo "NDK 27.0.12077973 not found at ${NDK}" >&2; exit 1; }
command -v gh >/dev/null || { echo "gh CLI required (GitHub access route)" >&2; exit 1; }

# 1. sherpa-onnx v1.13.7 源树（ORT 1.27.1 的官配版——build-android-arm64-v8a.sh:93）
if [ ! -d "${S}/.git" ]; then
  mkdir -p "${V}"
  gh repo clone k2-fsa/sherpa-onnx -- --depth 1 --branch v1.13.7 "${S}"
fi

# 2. 预编译 onnxruntime android arm64 静态库（csukuangfj/onnxruntime-libs，
#    sherpa 官方 CI 同源；v1.13.7 配 1.27.1）
ORT_DIR="${S}/ort/onnxruntime-android-arm64-v8a-static_lib-1.27.1"
if [ ! -d "${ORT_DIR}" ]; then
  mkdir -p "${S}/ort"
  (cd "${S}/ort" \
    && gh release download v1.27.1 --repo csukuangfj/onnxruntime-libs \
       --pattern 'onnxruntime-android-arm64-v8a-static_lib-1.27.1.zip' \
    && unzip -q onnxruntime-android-arm64-v8a-static_lib-1.27.1.zip)
fi

# 3. cmake 构建 sherpa 静态库（正典配置在 tools/voice/，拷入树内执行）
cp "${ROOT}/tools/voice/build-aginx.sh" "${S}/"
bash "${S}/build-aginx.sh"

# 4. 终链两个 CLI（库 android21 / 链 android29 + shims + strip）
bash "${ROOT}/tools/voice/link-aginx.sh"

echo "voice build complete: ${V}/bin/ag-asr ${V}/bin/ag-tts"
