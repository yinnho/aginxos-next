/* agimg_shim.c — the whole libjpeg dance in C so Rust never needs struct
 * layouts: jpeg_decompress_struct is opaque to the FFI boundary. Scale
 * selection walks scale_num/8 downward (DCT scaling inside the decoder —
 * the cheap way to fit a 12MP shot into a phone screen). Output pixel
 * format JCS_EXT_BGRX = bytes B,G,R,X = little-endian u32 0x00RRGGBB,
 * exactly aterm's DRM XRGB8888 framebuffers. Errors longjmp back here
 * instead of calling exit() (libjpeg default).
 */
#include <stdio.h>
#include <stdlib.h>
#include <setjmp.h>
#include "jpeglib.h"

struct agerr {
    struct jpeg_error_mgr pub;
    jmp_buf jb;
};

static void ag_err_exit(j_common_ptr ci)
{
    struct agerr *e = (struct agerr *)ci->err;
    longjmp(e->jb, 1);
}

unsigned int *agimg_decode(const unsigned char *data, unsigned long len,
                           unsigned max_w, unsigned max_h,
                           unsigned *out_w, unsigned *out_h)
{
    struct jpeg_decompress_struct ci;
    struct agerr err;
    unsigned w, h;
    unsigned int *pix;

    ci.err = jpeg_std_error(&err.pub);
    err.pub.error_exit = ag_err_exit;
    if (setjmp(err.jb)) {
        jpeg_destroy_decompress(&ci);
        return NULL;
    }

    jpeg_create_decompress(&ci);
    jpeg_mem_src(&ci, (unsigned char *)data, len);
    if (jpeg_read_header(&ci, TRUE) != JPEG_HEADER_OK) {
        jpeg_destroy_decompress(&ci);
        return NULL;
    }
    for (int num = 8; num > 1; num--) {
        ci.scale_num = num;
        ci.scale_denom = 8;
        jpeg_calc_output_dimensions(&ci);
        if (ci.output_width <= max_w && ci.output_height <= max_h)
            goto scaled;
    }
    ci.scale_num = 1;
    ci.scale_denom = 8;
    jpeg_calc_output_dimensions(&ci);
scaled:
    ci.out_color_space = JCS_EXT_BGRX;
    jpeg_start_decompress(&ci);
    w = ci.output_width;
    h = ci.output_height;
    pix = malloc((size_t)w * h * 4);
    if (!pix) {
        jpeg_destroy_decompress(&ci);
        return NULL;
    }
    while (ci.output_scanline < h) {
        JSAMPROW row = (JSAMPROW)(pix + (size_t)ci.output_scanline * w);
        jpeg_read_scanlines(&ci, &row, 1);
    }
    jpeg_finish_decompress(&ci);
    jpeg_destroy_decompress(&ci);
    *out_w = w;
    *out_h = h;
    return pix;
}
