/* jpegenc_tj — cam-shot's encoder face, backed by libjpeg-turbo (M47⑤d).
 *
 * The self-written double-precision DCT encoder (jpegenc.h) took 0.125 s
 * per 720x1561 q85 frame on device — the whole viewfinder fps ceiling.
 * Android itself encodes JPEG with libjpeg-turbo (AOSP external/
 * libjpeg-turbo); the same vendored tree the img crate decodes with is
 * built here WITH its NEON SIMD encoder, so this header keeps jpegenc.h's
 * exact call shape while the implementation underneath becomes the mature
 * one. Source list lives in scripts/build-rootfs.sh (mirrors
 * crates/img/build.rs, which mirrors upstream CMakeLists.txt).
 *
 * Same contract as jpegenc.h:
 *   jpeg_encode_gray8(px, w, h, stride, quality, out, cap) -> bytes | -1
 *   jpeg_encode_rgb24(px, w, h, stride, quality, out, cap) -> bytes | -1
 * Grayscale stays 1-component; RGB rides libjpeg's own NEON RGB->YCbCr
 * (replaces the hand-rolled per-pixel double math) with 4:2:0 subsampling,
 * matching the old encoder's output domain.
 */
#ifndef JPEGENC_TJ_H
#define JPEGENC_TJ_H

#include <stddef.h>
#include <stdint.h>
#include <sys/types.h> /* ssize_t */

ssize_t jpeg_encode_gray8(const uint8_t *px, int w, int h, int stride,
                          int quality, uint8_t *out, size_t cap);
ssize_t jpeg_encode_rgb24(const uint8_t *px, int w, int h, int stride,
                          int quality, uint8_t *out, size_t cap);

#endif /* JPEGENC_TJ_H */
