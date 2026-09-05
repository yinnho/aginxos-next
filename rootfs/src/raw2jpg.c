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
 *   --cfa    Bayer phase: rggb (default) | bggr | gbrg | grbg
 *
 * The encoder is jpegenc.h (same directory; validated host-side against
 * sips + libjpeg 2026-09-01). Once cam-shot grows a native --jpeg flag
 * this stays useful for converting already-captured dumps.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <time.h>
#include "jpegenc.h"

static int cfa = 0;   /* 0=rggb 1=bggr 2=gbrg 3=grbg */

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
            } else if (!(y & 1)) {    /* G site on an R row: R left/right */
                G = g[(size_t)y * w + x];
                R = (l + r) / 2;
                B = (u + d) / 2;
            } else {                  /* G site on a B row: B left/right */
                G = g[(size_t)y * w + x];
                B = (l + r) / 2;
                R = (u + d) / 2;
            }
            uint8_t *p = rgb + ((size_t)y * w + x) * 3;
            p[0] = (uint8_t)R; p[1] = (uint8_t)G; p[2] = (uint8_t)B;
        }
    return rgb;
}

int main(int argc, char **argv)
{
    if (argc < 5) {
        fprintf(stderr, "usage: %s <raw> <w> <h> <stride> [q] "
                "[--gray|--color] [--cfa rggb|bggr|gbrg|grbg] [--out p]\n",
                argv[0]);
        return 2;
    }
    const char *path = argv[1];
    int w = atoi(argv[2]), h = atoi(argv[3]), stride = atoi(argv[4]);
    int q = 85, color = 0;
    const char *out = NULL;
    for (int i = 5; i < argc; i++) {
        if (!strcmp(argv[i], "--color")) color = 1;
        else if (!strcmp(argv[i], "--gray")) color = 0;
        else if (!strcmp(argv[i], "--cfa") && i + 1 < argc) {
            const char *c = argv[++i];
            cfa = !strcmp(c, "bggr") ? 1 : !strcmp(c, "gbrg") ? 2 :
                  !strcmp(c, "grbg") ? 3 : 0;
        } else if (!strcmp(argv[i], "--out") && i + 1 < argc)
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
        uint8_t *rgb = g ? debayer(g, w, h) : NULL;
        free(g);
        if (!rgb) { fprintf(stderr, "debayer oom\n"); return 1; }
        n = jpeg_encode_rgb24(rgb, w, h, w * 3, q, outbuf,
                              (size_t)w * h * 3 + 65536);
        free(rgb);
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
