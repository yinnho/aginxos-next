#!/usr/bin/env bash
# Build cam-shot (+raw2jpg) for the device (M47). Standalone so a camera-line
# change deploy-tests with one script instead of a full bake (build-rootfs.sh
# calls this too — single source for the build command).
#
# cam-shot's JPEG encoder is vendored libjpeg-turbo 2.1.5.1 (M47⑤d) — the
# same tree the img crate builds from (crates/img/vendor; Android itself
# encodes JPEG with this library, AOSP external/libjpeg-turbo). Source lists
# mirror crates/img/build.rs, which mirrors upstream CMakeLists.txt: core
# sources +, for aarch64, the NEON_INTRINSICS set (pure C intrinsics, no .S
# assembly — exactly why zig cc can cross it like any other C). The
# hand-written encoder it replaced (jpegenc.h, still used by raw2jpg) took
# 0.125 s per 720x1561 q85 frame on device.
#
# NOTE: keep these lists in lockstep with crates/img/build.rs (edit both).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RECIPE="${ROOT}/rootfs"
VEND="${ROOT}/crates/img/vendor"
OUT="${OUT:-${ROOT}/out/cam}"
ZIG="${ZIG:-zig}"

test -f "${VEND}/jpeglib.h" || { echo "missing ${VEND}/jpeglib.h" >&2; exit 1; }

JPEG_CORE=(
  jcapimin.c jcapistd.c jccoefct.c jccolor.c jcdctmgr.c jchuff.c jcicc.c
  jcinit.c jcmainct.c jcmarker.c jcmaster.c jcomapi.c jcparam.c jcphuff.c
  jcprepct.c jcsample.c jctrans.c jdapimin.c jdapistd.c jdatadst.c jdatasrc.c
  jdcoefct.c jdcolor.c jddctmgr.c jdhuff.c jdicc.c jdinput.c jdmainct.c
  jdmarker.c jdmaster.c jdmerge.c jdphuff.c jdpostct.c jdsample.c jdtrans.c
  jerror.c jfdctflt.c jfdctfst.c jfdctint.c jidctflt.c jidctfst.c jidctint.c
  jidctred.c jquant1.c jquant2.c jutils.c jmemmgr.c jmemnobs.c jaricom.c
  jcarith.c jdarith.c
)
JPEG_NEON=(
  jcgray-neon.c jcphuff-neon.c jcsample-neon.c jdmerge-neon.c jdsample-neon.c
  jfdctfst-neon.c jidctred-neon.c jquanti-neon.c jccolor-neon.c jidctint-neon.c
  jidctfst-neon.c jdcolor-neon.c jfdctint-neon.c
)

mkdir -p "${OUT}"

# libjpeg-turbo objects, aarch64 musl static with the NEON intrinsics set.
JPEG_INC=(-I"${VEND}" -I"${VEND}/simd" -I"${VEND}/simd/arm" -DNEON_INTRINSICS)
OBJS=()
for f in "${JPEG_CORE[@]}"; do
  "${ZIG}" cc -target aarch64-linux-musl -O2 -c "${JPEG_INC[@]}" \
    -o "${OUT}/${f%.c}.o" "${VEND}/${f}"
  OBJS+=("${OUT}/${f%.c}.o")
done
for f in "${JPEG_NEON[@]}" jsimd.c jchuff-neon.c; do
  case "${f}" in
    jsimd.c) src="${VEND}/simd/arm/aarch64/jsimd.c" ;;
    jchuff-neon.c) src="${VEND}/simd/arm/aarch64/jchuff-neon.c" ;;
    *) src="${VEND}/simd/arm/${f}" ;;
  esac
  "${ZIG}" cc -target aarch64-linux-musl -O2 -c "${JPEG_INC[@]}" \
    -o "${OUT}/${f%.c}.o" "${src}"
  OBJS+=("${OUT}/${f%.c}.o")
done
# jccolext-neon.c is deliberately absent: it is #included by jccolor-neon.c
# (CMake compiles only these two aarch64 files).

"${ZIG}" cc -target aarch64-linux-musl -static -O2 "${JPEG_INC[@]}" \
  -o "${OUT}/aginx-cam-shot" \
  "${RECIPE}/src/cam-shot.c" "${RECIPE}/src/jpegenc_tj.c" "${OBJS[@]}"
echo "built ${OUT}/aginx-cam-shot"

# raw2jpg (M19c companion) keeps the self-written encoder — offline dump
# converter, no fps budget.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${OUT}/raw2jpg" "${RECIPE}/src/raw2jpg.c"
echo "built ${OUT}/raw2jpg"
