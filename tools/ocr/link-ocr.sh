#!/usr/bin/env bash
# M45: 终链 ag-ocr —— 裸 ORT C API 静态库 + NDK，输出自包含 ELF。
# 正典在 tools/ocr；scripts/build-ocr.sh 负责铺前置（ORT zip、模型规整名）。
#
# 工艺同 tools/voice/link-aginx.sh（收据 docs/HARDWARE.md M42d）：
# - 终链 API 层用 android29：api21 的静态二进制机上 abort
#   "TLS segment underaligned"
# - NDK 无 liblog/libandroid/libdl 静态版 → android-shims.c 垫
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
V="${ROOT}/out/voice"
S="${V}/sherpa-onnx-src"

NDK="$HOME/Library/Android/sdk/ndk/27.0.12077973"
PRE="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin"
CC="$PRE/aarch64-linux-android29-clang++"
STRIP="$PRE/llvm-strip"

ORT_LIB="$(find "$S/ort" -name libonnxruntime.a -exec dirname {} \; | head -1)"
test -n "${ORT_LIB}" || { echo "ORT static lib missing — see scripts/build-voice.sh step 2" >&2; exit 1; }
ORT_INC="$(dirname "${ORT_LIB}")/include"

mkdir -p "${ROOT}/out/ocr/bin"

# 垫片：liblog→stderr、AAsset/dl 死码桩（NDK 只有 .so 没有静态版）
"$PRE/aarch64-linux-android29-clang" \
  -c -O2 -o "${ROOT}/out/ocr/android-shims.o" "$ROOT/tools/ocr/android-shims.c"

# ag-ocr.c 单 TU（含 stb_image 实现）；clang++ 编（同 voice 先例，C/C++ 公共子集）
"$CC" -static -O2 -I"$ROOT/tools/ocr" -I"$ORT_INC" \
  -o "${ROOT}/out/ocr/bin/ag-ocr" \
  "$ROOT/tools/ocr/ag-ocr.c" "${ROOT}/out/ocr/android-shims.o" \
  "${ORT_LIB}/libonnxruntime.a" -lm
"$STRIP" "${ROOT}/out/ocr/bin/ag-ocr"

file "${ROOT}/out/ocr/bin/ag-ocr"
ls -la "${ROOT}/out/ocr/bin"
echo LINK-OK
