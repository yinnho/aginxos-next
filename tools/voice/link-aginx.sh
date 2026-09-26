#!/usr/bin/env bash
# M42d: 终链 ag-asr / ag-tts —— sherpa-onnx bionic 静态库 + NDK，输出自包含 ELF。
# 在 tools/voice 存放正典；构建在 out/voice/sherpa-onnx-src 工作树内执行
# （scripts/build-voice.sh 负责铺树）。产物落 out/voice/bin/。
#
# 工艺要点（详见 README.md，收据在 docs/HARDWARE.md M42d）：
# - 终链 API 层用 android29：api21 的静态二进制机上 abort
#   "TLS segment underaligned: alignment 8, needs ≥64"
# - NDK 无 liblog/libandroid/libdl 静态版 → android-shims.c 垫
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
V="${ROOT}/out/voice"
S="${V}/sherpa-onnx-src"

NDK="$HOME/Library/Android/sdk/ndk/27.0.12077973"
PRE="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin"
CC="$PRE/aarch64-linux-android29-clang++"
STRIP="$PRE/llvm-strip"

L="$S/install-aginx/lib"
ORT_LIB="$(find "$S/ort" -name libonnxruntime.a -exec dirname {} \; | head -1)"
test -n "${ORT_LIB}" || { echo "ORT static lib missing — see scripts/build-voice.sh" >&2; exit 1; }

LIBS="$L/libsherpa-onnx-c-api.a $L/libsherpa-onnx-core.a \
$L/libkaldi-decoder-core.a $L/libkaldi-native-fbank-core.a $L/libkissfft-float.a \
$L/libsherpa-onnx-kaldifst-core.a $L/libsherpa-onnx-fst.a $L/libsherpa-onnx-fstfar.a \
$L/libssentencepiece_core.a $L/libespeak-ng.a $L/libpiper_phonemize.a $L/libucd.a \
$ORT_LIB/libonnxruntime.a"

# 垫片：liblog→stderr、AAsset/dl 死码桩（NDK 只有 .so 没有静态版）
"$PRE/aarch64-linux-android29-clang" \
  -c -O2 -o "$S/android-shims.o" "$ROOT/tools/voice/android-shims.c"

mkdir -p "$V/bin"
for tool in asr tts; do
  $CC -static -O2 -I"$S/sherpa-onnx/c-api" -o "$V/bin/ag-$tool" \
    "$ROOT/tools/voice/ag-$tool.c" "$S/android-shims.o" $LIBS -lm
  # debug_info 让裸链产物 ~780MB；strip 后 26MB
  "$STRIP" "$V/bin/ag-$tool"
done
file "$V/bin/ag-asr" "$V/bin/ag-tts"
ls -la "$V/bin"
echo LINK-OK
