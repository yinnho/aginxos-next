/* raw2jpg — RAW10 dump -> JPEG, standalone companion to cam-shot (M19c).
 *
 * cam-shot (as of the 2026-09-01 bake) writes its captured frame to a
 * raw file (RAW10 packed, stride-padded) via inspect_buf(). This tool
 * converts that dump in place:
 *
 *   raw2jpg <raw> <w> <h> <stride> [q=85] [--gray|--color] [--cfa rggb]
 *          [--out <path>]
 *
 *   --gray   (default) bits[9:2] of each pixel -> gray8 JPEG
 *   --color  bilinear debayer -> RGB -> YCbCr 4:2:0 JPEG
 *   --cfa    Bayer phase: bggr (default, rear-proven) | rggb | gbrg | grbg
 *
 * The encoder is jpegenc.h (same directory; validated host-side against
 * sips + libjpeg 2026-09-01). Once cam-shot grows a native --jpeg flag
 * this stays useful for converting already-captured dumps.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <math.h>
#include <time.h>
#include "jpegenc.h"

static int cfa = 1;   /* 0=rggb 1=bggr 2=gbrg 3=grbg — bggr default: rear
                       * imx363 RDI starts on a B site (campix.h CFA
                       * ORIENTATION receipt, device 2026-09-05) */

/* RAW10 packed: 5 bytes -> 4 pixels; byte i of the group = pixel i's
 * bits[9:2] (the high 8 of 10). bits[1:0] live in the 5th byte. */
static uint8_t *raw10_gray(const uint8_t *raw, int w, int h, int stride)
{
    uint8_t *g = malloc((size_t)w * h);
    if (!g) return NULL;
    for (int y = 0; y < h; y++) {
        const uint8_t *r = raw + (size_t)y * stride;
        for (int x = 0; x < w; x += 4) {
            const uint8_t *p = r + (size_t)(x / 4) * 5;
            int n = w - x < 4 ? w - x : 4;
            for (int i = 0; i < n; i++)
                g[(size_t)y * w + x + i] = p[i];
        }
    }
    return g;
}

/* Optical black: v<=bl → 0, else stretch onto 0..255. Same LUT as campix. */
static void gray_black_level(uint8_t *g, size_t n, int bl)
{
    if (bl <= 0) return;
    if (bl > 254) bl = 254;
    int den = 255 - bl;
    for (size_t i = 0; i < n; i++) {
        int v = g[i];
        g[i] = v <= bl ? 0 : (uint8_t)((v - bl) * 255 / den);
    }
}

static uint8_t at(const uint8_t *g, int w, int h, int x, int y)
{
    if (x < 0) x = 0;
    if (y < 0) y = 0;
    if (x >= w) x = w - 1;
    if (y >= h) y = h - 1;
    return g[(size_t)y * w + x];
}

/* bilinear debayer: returns full-res RGB24 */
static uint8_t *debayer(const uint8_t *g, int w, int h)
{
    uint8_t *rgb = malloc((size_t)w * h * 3);
    if (!rgb) return NULL;
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            /* site color at (x,y) for the chosen CFA phase */
            int site;
            int even = !(y & 1), xeven = !(x & 1);
            switch (cfa) {
            case 1:  site = even ? (xeven ? 2 : 1) : (xeven ? 1 : 0); break;
            case 2:  site = even ? (xeven ? 1 : 2) : (xeven ? 0 : 1); break;
            case 3:  site = even ? (xeven ? 1 : 0) : (xeven ? 2 : 1); break;
            default: site = even ? (xeven ? 0 : 1) : (xeven ? 1 : 2); break;
            }
            /* 0=R 1=G 2=B */
            int R, G, B;
            int l = at(g, w, h, x - 1, y), r = at(g, w, h, x + 1, y);
            int u = at(g, w, h, x, y - 1), d = at(g, w, h, x, y + 1);
            int ul = at(g, w, h, x - 1, y - 1), ur = at(g, w, h, x + 1, y - 1);
            int dl = at(g, w, h, x - 1, y + 1), dr = at(g, w, h, x + 1, y + 1);
            if (site == 0) {          /* R site */
                R = g[(size_t)y * w + x];
                G = (l + r + u + d) / 4;
                B = (ul + ur + dl + dr) / 4;
            } else if (site == 2) {   /* B site */
                B = g[(size_t)y * w + x];
                G = (l + r + u + d) / 4;
                R = (ul + ur + dl + dr) / 4;
            } else {
                /* G site: horizontal chroma is the row primary.
                 * RGGB/GRBG even rows are R-primary; BGGR/GBRG even rows
                 * are B-primary (campix.h cp_px_lin, 2026-09-05). Using
                 * y-parity alone swapped R/B on every G site for --cfa bggr. */
                G = g[(size_t)y * w + x];
                int rrow = (cfa == 1 || cfa == 2) ? !even : even;
                if (rrow) {
                    R = (l + r) / 2;
                    B = (u + d) / 2;
                } else {
                    B = (l + r) / 2;
                    R = (u + d) / 2;
                }
            }
            uint8_t *p = rgb + ((size_t)y * w + x) * 3;
            p[0] = (uint8_t)R; p[1] = (uint8_t)G; p[2] = (uint8_t)B;
        }
    return rgb;
}

static void rgb_wb_gamma(uint8_t *rgb, int w, int h, int do_wb, double gamma)
{
    size_t n = (size_t)w * (size_t)h;
    double kr = 1.0, kb = 1.0;
    if (do_wb && n) {
        unsigned long long sr = 0, sg = 0, sb = 0;
        for (size_t i = 0; i < n; i++) {
            sr += rgb[i * 3];
            sg += rgb[i * 3 + 1];
            sb += rgb[i * 3 + 2];
        }
        double mr = (double)sr / (double)n;
        double mg = (double)sg / (double)n;
        double mb = (double)sb / (double)n;
        if (mr > 1.0) kr = mg / mr;
        if (mb > 1.0) kb = mg / mb;
        if (kr > 4.0) kr = 4.0;
        if (kb > 4.0) kb = 4.0;
    }
    uint8_t lut[256];
    if (gamma > 1.0) {
        for (int v = 0; v < 256; v++) {
            double y = 255.0 * pow((double)v / 255.0, 1.0 / gamma);
            int iv = (int)(y + 0.5);
            lut[v] = (uint8_t)(iv < 0 ? 0 : iv > 255 ? 255 : iv);
        }
    } else {
        for (int v = 0; v < 256; v++) lut[v] = (uint8_t)v;
    }
    for (size_t i = 0; i < n; i++) {
        int r = (int)(rgb[i * 3] * kr);
        int b = (int)(rgb[i * 3 + 2] * kb);
        if (r > 255) r = 255;
        if (b > 255) b = 255;
        rgb[i * 3] = lut[r];
        rgb[i * 3 + 1] = lut[rgb[i * 3 + 1]];
        rgb[i * 3 + 2] = lut[b];
    }
}

/* deg is clockwise, same sense as DT `rotation`. */
static uint8_t *rgb_rotate_cw(const uint8_t *in, int w, int h, int deg,
                             int *ow, int *oh)
{
    uint8_t *out;
    size_t n = (size_t)w * (size_t)h;
    if (deg == 0) {
        out = malloc(n * 3);
        if (!out) return NULL;
        memcpy(out, in, n * 3);
        *ow = w;
        *oh = h;
        return out;
    }
    if (deg == 180) {
        out = malloc(n * 3);
        if (!out) return NULL;
        for (int y = 0; y < h; y++)
            for (int x = 0; x < w; x++) {
                const uint8_t *s = in + ((size_t)y * w + x) * 3;
                uint8_t *d = out + ((size_t)(h - 1 - y) * w + (w - 1 - x)) * 3;
                d[0] = s[0]; d[1] = s[1]; d[2] = s[2];
            }
        *ow = w;
        *oh = h;
        return out;
    }
    out = malloc(n * 3);
    if (!out) return NULL;
    *ow = h;
    *oh = w;
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            const uint8_t *s = in + ((size_t)y * w + x) * 3;
            int nx, ny;
            if (deg == 90) {
                nx = h - 1 - y;
                ny = x;
            } else { /* 270 */
                nx = y;
                ny = w - 1 - x;
            }
            uint8_t *d = out + ((size_t)ny * (*ow) + nx) * 3;
            d[0] = s[0]; d[1] = s[1]; d[2] = s[2];
        }
    return out;
}

int main(int argc, char **argv)
{
    if (argc < 5) {
        fprintf(stderr, "usage: %s <raw> <w> <h> <stride> [q] "
                "[--gray|--color] [--cfa rggb|bggr|gbrg|grbg] "
                "[--rotate 90|180|270] [--wb] [--gamma g] [--bl n] [--out p]\n",
                argv[0]);
        return 2;
    }
    const char *path = argv[1];
    int w = atoi(argv[2]), h = atoi(argv[3]), stride = atoi(argv[4]);
    int q = 85, color = 0, rotate = 0, do_wb = 0, bl = 0;
    double gamma = 0;
    const char *out = NULL;
    for (int i = 5; i < argc; i++) {
        if (!strcmp(argv[i], "--color")) color = 1;
        else if (!strcmp(argv[i], "--gray")) color = 0;
        else if (!strcmp(argv[i], "--wb")) do_wb = 1;
        else if (!strcmp(argv[i], "--cfa") && i + 1 < argc) {
            const char *c = argv[++i];
            cfa = !strcmp(c, "bggr") ? 1 : !strcmp(c, "gbrg") ? 2 :
                  !strcmp(c, "grbg") ? 3 : 0;
        } else if (!strcmp(argv[i], "--rotate") && i + 1 < argc) {
            rotate = atoi(argv[++i]);
            if (rotate != 0 && rotate != 90 && rotate != 180 && rotate != 270)
                rotate = 0;
        } else if (!strcmp(argv[i], "--gamma") && i + 1 < argc)
            gamma = atof(argv[++i]);
        else if (!strcmp(argv[i], "--bl") && i + 1 < argc)
            bl = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--out") && i + 1 < argc)
            out = argv[++i];
        else if (argv[i][0] != '-')
            q = atoi(argv[i]);
    }
    if (q < 1) q = 1;
    if (q > 100) q = 100;

    FILE *f = fopen(path, "rb");
    if (!f) { fprintf(stderr, "open %s: %s\n", path, strerror(errno)); return 2; }
    fseek(f, 0, SEEK_END);
    long fsz = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (fsz < (long)stride * h) {
        fprintf(stderr, "%s: %ld B < stride*h %d\n", path, fsz, stride * h);
        fclose(f);
        return 2;
    }
    uint8_t *raw = malloc((size_t)stride * h);
    if (fread(raw, 1, (size_t)stride * h, f) != (size_t)stride * h) {
        fprintf(stderr, "short read\n"); return 2;
    }
    fclose(f);

    struct timespec a, b;
    clock_gettime(CLOCK_MONOTONIC, &a);

    uint8_t *outbuf = malloc((size_t)w * h * 3 + 65536);
    ssize_t n;
    if (color) {
        uint8_t *g = raw10_gray(raw, w, h, stride);
        if (g) gray_black_level(g, (size_t)w * (size_t)h, bl);
        uint8_t *rgb = g ? debayer(g, w, h) : NULL;
        free(g);
        if (!rgb) { fprintf(stderr, "debayer oom\n"); return 1; }
        rgb_wb_gamma(rgb, w, h, do_wb, gamma);
        int ow = w, oh = h;
        uint8_t *rot = rgb_rotate_cw(rgb, w, h, rotate, &ow, &oh);
        free(rgb);
        if (!rot) { fprintf(stderr, "rotate oom\n"); return 1; }
        n = jpeg_encode_rgb24(rot, ow, oh, ow * 3, q, outbuf,
                              (size_t)ow * oh * 3 + 65536);
        w = ow;
        h = oh;
        free(rot);
    } else {
        uint8_t *g = raw10_gray(raw, w, h, stride);
        if (!g) { fprintf(stderr, "oom\n"); return 1; }
        n = jpeg_encode_gray8(g, w, h, w, q, outbuf,
                              (size_t)w * h + 65536);
        free(g);
    }
    if (n < 0) { fprintf(stderr, "encode overflow\n"); return 1; }

    char def[512];
    if (!out) {
        snprintf(def, sizeof def, "%s", path);
        char *dot = strrchr(def, '.');
        if (dot && !strcmp(dot, ".raw")) strcpy(dot, ".jpg");
        else strcat(def, ".jpg");
        out = def;
    }
    FILE *o = fopen(out, "wb");
    if (!o) { fprintf(stderr, "open %s: %s\n", out, strerror(errno)); return 2; }
    fwrite(outbuf, 1, (size_t)n, o);
    fclose(o);

    clock_gettime(CLOCK_MONOTONIC, &b);
    double t = (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9;
    printf("%s: %dx%d %s q%d -> %zd B (%.2f bpp) in %.3f s\n",
           out, w, h, color ? "color" : "gray", q, n,
           (double)n * 8 / ((double)w * h), t);
    return 0;
}
