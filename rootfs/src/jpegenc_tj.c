/* jpegenc_tj.c — jpegenc.h's API surface, implemented on vendored
 * libjpeg-turbo 2.1.5.1 (M47⑤d). cam-shot's two call sites change nothing;
 * underneath, the self-written double-precision DCT encoder (0.125 s/frame
 * on device, the viewfinder fps ceiling) is replaced by libjpeg-turbo's
 * NEON path — the same library Android itself encodes JPEG with (AOSP
 * external/libjpeg-turbo), from the same vendored tree the img crate uses.
 *
 * Encode straight into the caller's buffer: a private destination manager
 * (no jpeg_mem_dest malloc round-trip), setjmp error handler so any libjpeg
 * complaint — including cap overflow — surfaces as -1, and per-row strides
 * via the caller's stride argument. Grayscale rides 1-component; RGB rides
 * libjpeg's own NEON RGB->YCbCr + 2x2 downsampling (jpeg_set_defaults), the
 * same output domain the old encoder produced by hand.
 */
#include <setjmp.h>
#include <stdio.h>

#include "jpeglib.h"
#include "jerror.h" // application-visible error codes (jpeglib.h skips it for extern apps)

#include "jpegenc_tj.h"

// ---- caller-buffer destination (jdatadst.c's memory dest, minus malloc) ----

typedef struct {
    struct jpeg_destination_mgr pub;
    uint8_t *out;
    size_t cap;
} buf_dest;

// jpeg_start_compress calls this — so it (not the caller site) owns the
// pointer reset, same shape as jdatadst.c's memory destination.
static void buf_dest_init(j_compress_ptr cinfo) {
    buf_dest *d = (buf_dest *)cinfo->dest;
    d->pub.next_output_byte = d->out;
    d->pub.free_in_buffer = d->cap;
}

static boolean buf_dest_empty(j_compress_ptr cinfo) {
    // Output cap exhausted. There is no way to hand libjpeg more room, so
    // fail the compress through the standard error path (longjmp -> -1).
    ERREXIT(cinfo, JERR_BUFFER_SIZE);
    return FALSE; // unreachable
}

static void buf_dest_term(j_compress_ptr cinfo) {
    (void)cinfo; // size is read from free_in_buffer after finish_compress
}

// ---- error handler: libjpeg must not exit() the process ----

typedef struct {
    struct jpeg_error_mgr pub;
    jmp_buf jb;
} err_mgr;

static void err_exit(j_common_ptr cinfo) {
    err_mgr *e = (err_mgr *)cinfo->err;
    longjmp(e->jb, 1);
}

// ---- the one encoder, parameterized by component count ----

static ssize_t encode(const uint8_t *px, int w, int h, int stride,
                      int quality, int components, J_COLOR_SPACE in_space,
                      uint8_t *out, size_t cap) {
    struct jpeg_compress_struct cinfo;
    err_mgr jerr;
    buf_dest dest;

    if (quality < 1) quality = 1;
    if (quality > 100) quality = 100;

    cinfo.err = jpeg_std_error(&jerr.pub);
    jerr.pub.error_exit = err_exit;
    if (setjmp(jerr.jb)) {
        jpeg_destroy_compress(&cinfo);
        return -1;
    }
    jpeg_create_compress(&cinfo);

    dest.out = out;
    dest.cap = cap;
    dest.pub.init_destination = buf_dest_init;
    dest.pub.empty_output_buffer = buf_dest_empty;
    dest.pub.term_destination = buf_dest_term;
    dest.pub.next_output_byte = out;
    dest.pub.free_in_buffer = cap;
    cinfo.dest = &dest.pub;

    cinfo.image_width = (JDIMENSION)w;
    cinfo.image_height = (JDIMENSION)h;
    cinfo.input_components = components;
    cinfo.in_color_space = in_space;
    jpeg_set_defaults(&cinfo); // 2x2 subsampling == the old encoder's 4:2:0
    jpeg_set_quality(&cinfo, quality, TRUE);
    jpeg_start_compress(&cinfo, TRUE);
    while (cinfo.next_scanline < cinfo.image_height) {
        JSAMPROW row = (JSAMPROW)(px + (size_t)cinfo.next_scanline * (size_t)stride);
        jpeg_write_scanlines(&cinfo, &row, 1);
    }
    jpeg_finish_compress(&cinfo);
    ssize_t n = (ssize_t)(cap - dest.pub.free_in_buffer);
    jpeg_destroy_compress(&cinfo);
    return n;
}

ssize_t jpeg_encode_gray8(const uint8_t *px, int w, int h, int stride,
                          int quality, uint8_t *out, size_t cap) {
    return encode(px, w, h, stride, quality, 1, JCS_GRAYSCALE, out, cap);
}

ssize_t jpeg_encode_rgb24(const uint8_t *px, int w, int h, int stride,
                          int quality, uint8_t *out, size_t cap) {
    return encode(px, w, h, stride, quality, 3, JCS_RGB, out, cap);
}
