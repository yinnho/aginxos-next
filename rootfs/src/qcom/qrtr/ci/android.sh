#!/bin/sh
# SPDX-License-Identifier: BSD-3-Clause
#
# Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
#
# Cross-build qrtr for Android using the NDK.
#
# Expects the following environment variables:
#   ABI          - Android ABI (arm64-v8a, armeabi-v7a, x86_64)
#   API          - Android API level to target
#   ANDROID_NDK  - path to an unpacked Android NDK

set -ex

TOOLCHAIN="$ANDROID_NDK/toolchains/llvm/prebuilt/linux-x86_64"

case "$ABI" in
	arm64-v8a)
		CPU_FAMILY=aarch64
		CPU=aarch64
		TRIPLE=aarch64-linux-android
		ELF_MACHINE=AArch64
	;;
	armeabi-v7a)
		CPU_FAMILY=arm
		CPU=armv7a
		TRIPLE=armv7a-linux-androideabi
		ELF_MACHINE=ARM
	;;
	x86_64)
		CPU_FAMILY=x86_64
		CPU=x86_64
		TRIPLE=x86_64-linux-android
		ELF_MACHINE=X86-64
	;;
	*)
		echo "Unknown ABI '$ABI'" >&2
		exit 1
	;;
esac

# The per-ABI clang wrapper already selects the correct target and sysroot, so
# the cross file does not need to spell those out.
cat > android-cross.txt <<EOF
[binaries]
c = '$TOOLCHAIN/bin/${TRIPLE}${API}-clang'
ar = '$TOOLCHAIN/bin/llvm-ar'
strip = '$TOOLCHAIN/bin/llvm-strip'

[host_machine]
system = 'android'
cpu_family = '$CPU_FAMILY'
cpu = '$CPU'
endian = 'little'
EOF
cat android-cross.txt

# Android.bp builds qrtr with -Wno-error, so do not force -Werror here.
meson setup --errorlogs --cross-file android-cross.txt . build
ninja -C build

# Confirm the resulting binary really is for the requested ABI.
"$TOOLCHAIN/bin/llvm-readelf" -h build/src/qrtr-cfg | grep -q "Machine:.*$ELF_MACHINE"
echo "Android build for $ABI ($ELF_MACHINE) succeeded"
