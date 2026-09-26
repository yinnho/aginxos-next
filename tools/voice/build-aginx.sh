#!/usr/bin/env bash
# M42d: bionic-static sherpa-onnx for redfin rootfs — cmake 配置正典。
# 必须在 sherpa-onnx 源树根执行（scripts/build-voice.sh 会把本文件拷进去）。
# 预编译 onnxruntime android arm64 静态库 + sherpa-onnx v1.13.7 源码（C API, 无 JNI/二进制）。
set -exuo pipefail
cd "$(dirname "$0")"

ANDROID_NDK="$HOME/Library/Android/sdk/ndk/27.0.12077973"
ORT_ROOT="$(find ort -maxdepth 2 -type d -name 'onnxruntime-android*' | head -1)"
# onnxruntime.cmake 只认环境变量（-D 会被无视→走 download 分支→Android 报错
# "Only support Linux, macOS, and Windows"）
export SHERPA_ONNXRUNTIME_LIB_DIR="$PWD/$ORT_ROOT/lib"
export SHERPA_ONNXRUNTIME_INCLUDE_DIR="$PWD/$ORT_ROOT/include"

dir=build-aginx
cmake -DCMAKE_TOOLCHAIN_FILE="$ANDROID_NDK/build/cmake/android.toolchain.cmake" \
  -DSHERPA_ONNX_ENABLE_TTS=ON \
  -DSHERPA_ONNX_ENABLE_SPEAKER_DIARIZATION=OFF \
  -DSHERPA_ONNX_ENABLE_BINARY=OFF \
  -DBUILD_PIPER_PHONMIZE_EXE=OFF \
  -DBUILD_PIPER_PHONMIZE_TESTS=OFF \
  -DBUILD_ESPEAK_NG_EXE=OFF \
  -DBUILD_ESPEAK_NG_TESTS=OFF \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF \
  -DSHERPA_ONNX_ENABLE_PYTHON=OFF \
  -DSHERPA_ONNX_ENABLE_TESTS=OFF \
  -DSHERPA_ONNX_ENABLE_CHECK=OFF \
  -DSHERPA_ONNX_ENABLE_PORTAUDIO=OFF \
  -DSHERPA_ONNX_ENABLE_JNI=OFF \
  -DSHERPA_ONNX_LINK_LIBSTDCPP_STATICALLY=OFF \
  -DSHERPA_ONNX_ENABLE_C_API=ON \
  -DCMAKE_INSTALL_PREFIX="$PWD/install-aginx" \
  -DANDROID_ABI=arm64-v8a \
  -DANDROID_PLATFORM=android-21 \
  -B "$dir" -S .

cmake --build "$dir" -- -j8
cmake --install "$dir"
echo AGINX-BUILD-OK
