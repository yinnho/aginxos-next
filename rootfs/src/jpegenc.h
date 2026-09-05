/* jpegenc.h — minimal baseline JPEG encoder, no libraries.
 *
 * Written for cam-shot (M19c): the phone's stills path produced gray8
 * PNGs at ~2.3 MB/frame (stored-DEFLATE = uncompressed). For the agent
 * use case the frame travels over the relay, so we need real entropy
 * coding; and color — RAW10 is Bayer, and a debayered color JPEG is
 * worth far more to a vision model than a sharper grayscale one.
 *
 * Scope kept deliberately boring:
 *   - baseline sequential DCT, Huffman (JFIF APP0)
 *   - gray8 input  -> 1 component, 1 quant table
 *   - RGB24 input  -> YCbCr 4:2:0 (encoder converts internally)
 *   - quality 1..100 via the IJG scaling rule
 *   - DCT: separable double-precision with a precomputed cosine
 *     matrix (1024 mults/block — ~0.5 s for a 2 MP color frame on the
 *     big core; switch to integer AAN if it ever matters)
 *   - Huffman tables are BUILT AT RUNTIME from a frequency model by a
 *     plain merge tree + canonical code assignment. No Annex-K tables
 *     copied from memory: construction is ~60 lines and is correct by
 *     construction (every legal symbol gets freq>=1, depth capped 16,
 *     codes canonical so any decoder rebuilds them identically).
 *
 * API:
 *   ssize_t jpeg_encode_gray8(const uint8_t *px, int w, int h, int stride,
 *                             int quality, uint8_t *out, size_t cap);
 *   ssize_t jpeg_encode_rgb24(const uint8_t *px, int w, int h, int stride,
 *                             int quality, uint8_t *out, size_t cap);
 * Return: bytes written, or -1 if out is too small (caller sizing:
 * raw size + 64 KiB has never been exceeded).
 */
#ifndef JPEGENC_H
#define JPEGENC_H

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <sys/types.h>

/* ---- standard quantization tables (Annex K, T.81) ---- */
static const uint8_t std_qt_luma[64] = {
    16, 11, 10, 16, 24, 40, 51, 61,
    12, 12, 14, 19, 26, 58, 60, 55,
    14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62,
    18, 22, 37, 56, 68, 109, 103, 77,
    24, 35, 55, 64, 81, 104, 113, 92,
    49, 64, 78, 87, 103, 121, 120, 101,
    72, 92, 95, 98, 112, 100, 103, 99,
};
static const uint8_t std_qt_chroma[64] = {
    17, 18, 24, 47, 99, 99, 99, 99,
    18, 21, 26, 66, 99, 99, 99, 99,
    24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
};

/* zigzag: natural (row-major) index of zigzag position k */
static const uint8_t zz_nat[64] = {
     0,  1,  8, 16,  9,  2,  3, 10,
    17, 24, 32, 25, 18, 11,  4,  5,
    12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13,  6,  7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36,
    29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63,
};

/* ---- encoder state ---- */
struct jenc {
    uint8_t *out;
    size_t cap, len;
    int quality;
    uint8_t qt[2][64];            /* [0]=luma [1]=chroma, scaled */
    double cm[8][8];              /* DCT basis: F = cm * f * cm^T */
    /* huffman: [comp 0=luma 1=chroma][0=DC 1=AC] */
    uint16_t hcode[2][2][256];
    uint8_t hlen[2][2][256];
    uint8_t hbits[2][2][17];      /* DHT bits histogram */
    uint8_t hval[2][2][256];      /* DHT values in canonical order */
    int nhval[2][2];
    /* bit writer */
    uint32_t acc;
    int nacc;
};

static void je_byte(struct jenc *e, uint8_t b)
{
    if (e->len < e->cap)
        e->out[e->len] = b;
    e->len++;
}

static void je_word(struct jenc *e, uint16_t w)
{
    je_byte(e, (uint8_t)(w >> 8));
    je_byte(e, (uint8_t)w);
}

/* entropy bytes go through 0xFF stuffing */
static void je_put_bits(struct jenc *e, uint32_t code, int nbits)
{
    e->acc = (e->acc << nbits) | (code & ((1u << nbits) - 1u));
    e->nacc += nbits;
    while (e->nacc >= 8) {
        uint8_t b = (uint8_t)(e->acc >> (e->nacc - 8));
        je_byte(e, b);
        if (b == 0xFF)
            je_byte(e, 0x00);
        e->nacc -= 8;
        e->acc &= (1u << e->nacc) - 1u;
    }
}

static void je_flush_bits(struct jenc *e)
{
    if (e->nacc > 0) {
        int pad = 8 - e->nacc;                /* 1..7 */
        je_put_bits(e, (1u << pad) - 1u, pad); /* pad with 1-bits */
    }
}

/* ---- huffman construction ------------------------------------------
 * freq[sym] for every encodable symbol (>=1). Merge-tree lengths,
 * depth capped at 16 by clamping frequencies to a rising floor and
 * retrying, then canonical code assignment (sorted by length, then
 * symbol value — matching how the DHT segment lists them). */
static void je_build_huff(struct jenc *e, int comp, int ac,
                          const uint32_t *freq)
{
    /* One phantom symbol (index 256) joins the merge tree with freq 1
     * but is excluded from hbits/hval. Without it the code is COMPLETE
     * (Kraft = 1): the all-1s code gets assigned, and libjpeg rejects
     * the stream ("Bogus Huffman table"). Annex-K tables are incomplete
     * the same way. */
    int nsym = 0, sym[256 + 1];
    for (int s = 0; s < 256; s++)
        if (freq[s])
            sym[nsym++] = s;
    sym[nsym++] = 256;
    uint8_t len[257] = {0};
    uint32_t f[256];
    memcpy(f, freq, sizeof(f));

    for (int floor = 0;; floor = floor ? floor * 2 : 1) {
        struct jnode { uint32_t w; int a, b; };
        struct jnode nodes[520];
        int nn = 0;
        uint32_t wf[257];
        uint64_t total = 0;
        for (int i = 0; i < nsym; i++) {
            wf[i] = (sym[i] < 256 ? f[sym[i]] : 1) + floor;
            total += wf[i];
        }
        uint64_t scale = total > 100000 ? total / 100000 : 1;
        for (int i = 0; i < nsym; i++)
            wf[i] = (uint32_t)(wf[i] / scale) + 1;
        for (int i = 0; i < nsym; i++) {
            nodes[nn].w = wf[i]; nodes[nn].a = i; nodes[nn].b = -1;
            nn++;
        }
        /* classic two-queue merge on sorted weights */
        for (int i = 1; i < nn; i++) {          /* insertion sort (n<=256) */
            struct jnode t = nodes[i];
            int j = i - 1;
            while (j >= 0 && nodes[j].w > t.w) { nodes[j+1] = nodes[j]; j--; }
            nodes[j+1] = t;
        }
        int head = 0;
        while (nn - head > 1) {
            uint32_t w = nodes[head].w + nodes[head+1].w;
            int a = head, b = head + 1;
            head += 2;
            /* insert merged node sorted after existing equal weights */
            int pos = nn;
            while (pos > head && nodes[pos-1].w > w) { nodes[pos] = nodes[pos-1]; pos--; }
            nodes[pos].w = w; nodes[pos].a = a; nodes[pos].b = b;
            nn++;
        }
        /* depth = tree height; walk from root */
        int root = head;
        int depth[512];
        int stack[512], sp = 0;
        depth[root] = 0; stack[sp++] = root;
        int maxd = 0;
        while (sp) {
            int n = stack[--sp];
            if (nodes[n].b < 0) {
                len[sym[nodes[n].a]] = (uint8_t)depth[n];
                if (depth[n] > maxd) maxd = depth[n];
                continue;
            }
            depth[nodes[n].a] = depth[n] + 1;
            depth[nodes[n].b] = depth[n] + 1;
            stack[sp++] = nodes[n].a;
            stack[sp++] = nodes[n].b;
        }
        if (maxd <= 16)
            break;
        /* too deep: raise the floor (flattens the tree) and retry */
        if (floor > (1u << 20)) {
            /* pathological — force near-flat lengths */
            for (int i = 0; i < nsym; i++)
                len[sym[i]] = 16;
            break;
        }
    }

    /* canonical assignment */
    memset(e->hbits[comp][ac], 0, 17);
    for (int i = 0; i < nsym; i++)
        if (sym[i] < 256)
            e->hbits[comp][ac][len[sym[i]]]++;
    /* order values by (length, symbol) */
    int idx = 0;
    for (int l = 1; l <= 16; l++)
        for (int s = 0; s < 256; s++)
            if (freq[s] && len[s] == l)
                e->hval[comp][ac][idx++] = (uint8_t)s;
    e->nhval[comp][ac] = idx;

    uint32_t code = 0;               /* left-aligned in 16 bits */
    int k = 0;
    for (int l = 1; l <= 16; l++) {
        for (int i = 0; i < e->hbits[comp][ac][l]; i++) {
            int s = e->hval[comp][ac][k++];
            e->hcode[comp][ac][s] = (uint16_t)(code >> (16 - l));
            e->hlen[comp][ac][s] = (uint8_t)l;
            code += 1u << (16 - l);
        }
    }
}

/* ---- DCT ---- */
static void je_dct(struct jenc *e, const double *in /*64, natural*/,
                   int32_t *out /*64, natural*/)
{
    double t[64], F[64];
    for (int u = 0; u < 8; u++)
        for (int j = 0; j < 8; j++) {
            double s = 0;
            for (int i = 0; i < 8; i++)
                s += e->cm[u][i] * in[i * 8 + j];
            t[u * 8 + j] = s;
        }
    for (int u = 0; u < 8; u++)
        for (int v = 0; v < 8; v++) {
            double s = 0;
            for (int j = 0; j < 8; j++)
                s += t[u * 8 + j] * e->cm[v][j];
            F[u * 8 + v] = s;
        }
    for (int i = 0; i < 64; i++)
        out[i] = (int32_t)lround(F[i]);
}

/* ---- one block: fdct + quant + huffman ---- */
static void je_encode_block(struct jenc *e, const uint8_t *px, int stride,
                            int x0, int y0, int w, int h, int comp /*0/1*/,
                            int *dcpred)
{
    double in[64];
    int32_t q[64];
    for (int y = 0; y < 8; y++) {
        int sy = y0 + y < h ? y0 + y : h - 1;
        if (sy < 0) sy = 0;
        for (int x = 0; x < 8; x++) {
            int sx = x0 + x < w ? x0 + x : w - 1;
            if (sx < 0) sx = 0;
            in[y * 8 + x] = (double)px[sy * stride + sx] - 128.0;
        }
    }
    je_dct(e, in, q);
    for (int i = 0; i < 64; i++)
        q[i] = (int32_t)lround((double)q[i] / e->qt[comp][i]);

    int dc = q[0], diff = dc - *dcpred;
    *dcpred = dc;
    int ad = diff < 0 ? -diff : diff;
    int sz = 0;
    while ((1 << sz) <= ad) sz++;               /* category */
    je_put_bits(e, e->hcode[comp][0][sz], e->hlen[comp][0][sz]);
    if (sz) {
        int v = diff < 0 ? diff - 1 : diff;     /* one's-complement cat */
        je_put_bits(e, (uint32_t)v, sz);
    }
    int run = 0;
    for (int k = 1; k < 64; k++) {
        int c = q[zz_nat[k]];
        if (c == 0) { run++; continue; }
        while (run > 15) {
            je_put_bits(e, e->hcode[comp][1][0xF0], e->hlen[comp][1][0xF0]);
            run -= 16;
        }
        ad = c < 0 ? -c : c;
        sz = 0;
        while ((1 << sz) <= ad) sz++;
        je_put_bits(e, e->hcode[comp][1][(run << 4) | sz],
                    e->hlen[comp][1][(run << 4) | sz]);
        je_put_bits(e, (uint32_t)(c < 0 ? c - 1 : c), sz);
        run = 0;
    }
    if (run)
        je_put_bits(e, e->hcode[comp][1][0x00], e->hlen[comp][1][0x00]);
}

/* ---- markers ---- */
static void je_marker(struct jenc *e, uint8_t m)
{
    je_byte(e, 0xFF);
    je_byte(e, m);
}

static void je_dqt(struct jenc *e)
{
    je_marker(e, 0xDB);
    je_word(e, 2 + 65 * 2);
    for (int t = 0; t < 2; t++) {
        je_byte(e, (uint8_t)t);                 /* 8-bit precision */
        for (int k = 0; k < 64; k++)
            je_byte(e, e->qt[t][zz_nat[k]]);
    }
}

static void je_dht_one(struct jenc *e, int comp, int ac)
{
    je_marker(e, 0xC4);
    je_word(e, (uint16_t)(2 + 1 + 16 + e->nhval[comp][ac]));
    je_byte(e, (uint8_t)((ac ? 0x10 : 0x00) | comp)); /* Tc<<4 | Th */
    for (int l = 1; l <= 16; l++)
        je_byte(e, e->hbits[comp][ac][l]);
    for (int i = 0; i < e->nhval[comp][ac]; i++)
        je_byte(e, e->hval[comp][ac][i]);
}

static ssize_t je_run(struct jenc *e, const uint8_t *planes[3],
                      const int strides[3], int w, int h, int color)
{
    /* quant tables */
    for (int t = 0; t < 2; t++) {
        int q = e->quality;
        for (int k = 0; k < 64; k++) {
            int v = t == 0 ? std_qt_luma[k] : std_qt_chroma[k];
            v = q < 50 ? v * 50 / (q ? q : 1) : v * (200 - 2 * q) / 100;
            if (v < 1) v = 1;
            if (v > 255) v = 255;
            e->qt[t][k] = (uint8_t)v;
        }
    }
    /* DCT basis */
    for (int u = 0; u < 8; u++) {
        double cu = u == 0 ? 0.7071067811865476 : 1.0;
        for (int x = 0; x < 8; x++)
            e->cm[u][x] = 0.5 * cu *
                cos((2 * x + 1) * u * 3.141592653589793 / 16.0);
    }
    /* huffman models: smooth decays, every legal symbol >= 1 */
    uint32_t fdc[2][256] = {{0}, {0}}, fac[2][256] = {{0}, {0}};
    for (int c = 0; c < 2; c++) {
        for (int s = 0; s <= 11; s++)
            fdc[c][s] = (uint32_t)(40 >> (s > 4 ? s - 4 : 0)) + 1;
        fac[c][0x00] = c ? 500 : 260;           /* EOB */
        fac[c][0xF0] = 6;                       /* ZRL */
        for (int run = 0; run < 16; run++)
            for (int sz = 1; sz <= 10; sz++) {
                uint32_t base = run == 0 ? (c ? 34 : 60) : (c ? 8 : 16);
                fac[c][(run << 4) | sz] = base >> sz / 2;
                if (!fac[c][(run << 4) | sz])
                    fac[c][(run << 4) | sz] = 1;
            }
        je_build_huff(e, c, 0, fdc[c]);
        je_build_huff(e, c, 1, fac[c]);
    }

    je_marker(e, 0xD8);                          /* SOI */
    je_marker(e, 0xE0);                          /* APP0 JFIF */
    je_word(e, 16);
    je_byte(e, 'J'); je_byte(e, 'F'); je_byte(e, 'I'); je_byte(e, 'F');
    je_byte(e, 0);
    je_word(e, 0x0101);
    je_byte(e, 0); je_word(e, 1); je_word(e, 1);
    je_byte(e, 0); je_byte(e, 0);
    je_dqt(e);

    int nc = color ? 3 : 1;
    je_marker(e, 0xC0);                          /* SOF0 */
    je_word(e, (uint16_t)(8 + 3 * nc));
    je_byte(e, 8);
    je_word(e, (uint16_t)h);
    je_word(e, (uint16_t)w);
    je_byte(e, (uint8_t)nc);
    if (color) {
        je_byte(e, 1); je_byte(e, 0x22); je_byte(e, 0);
        je_byte(e, 2); je_byte(e, 0x11); je_byte(e, 1);
        je_byte(e, 3); je_byte(e, 0x11); je_byte(e, 1);
    } else {
        je_byte(e, 1); je_byte(e, 0x11); je_byte(e, 0);
    }

    je_dht_one(e, 0, 0); je_dht_one(e, 0, 1);
    if (color) { je_dht_one(e, 1, 0); je_dht_one(e, 1, 1); }

    je_marker(e, 0xDA);                          /* SOS */
    je_word(e, (uint16_t)(6 + 2 * nc));
    je_byte(e, (uint8_t)nc);
    if (color) {
        je_byte(e, 1); je_byte(e, 0x00);
        je_byte(e, 2); je_byte(e, 0x11);
        je_byte(e, 3); je_byte(e, 0x11);
    } else {
        je_byte(e, 1); je_byte(e, 0x00);
    }
    je_byte(e, 0); je_byte(e, 63); je_byte(e, 0); /* Ss, Se, Ah/Al */

    e->acc = 0; e->nacc = 0;
    int dcpred[3] = {0, 0, 0};
    if (color) {
        int cw = (w + 1) / 2, ch = (h + 1) / 2;
        for (int my = 0; my < h; my += 16)
            for (int mx = 0; mx < w; mx += 16) {
                for (int by = 0; by < 16; by += 8)
                    for (int bx = 0; bx < 16; bx += 8)
                        je_encode_block(e, planes[0], strides[0],
                                        mx + bx, my + by, w, h, 0, &dcpred[0]);
                je_encode_block(e, planes[1], strides[1], mx / 2, my / 2,
                                cw, ch, 1, &dcpred[1]);
                je_encode_block(e, planes[2], strides[2], mx / 2, my / 2,
                                cw, ch, 1, &dcpred[2]);
            }
    } else {
        for (int my = 0; my < h; my += 8)
            for (int mx = 0; mx < w; mx += 8)
                je_encode_block(e, planes[0], strides[0], mx, my, w, h,
                                0, &dcpred[0]);
    }
    je_flush_bits(e);
    je_marker(e, 0xD9);                          /* EOI */
    return e->len > e->cap ? -1 : (ssize_t)e->len;
}

ssize_t jpeg_encode_gray8(const uint8_t *px, int w, int h, int stride,
                          int quality, uint8_t *out, size_t cap)
{
    struct jenc *e = calloc(1, sizeof(*e));
    if (!e) return -1;
    e->out = out; e->cap = cap; e->len = 0; e->quality = quality;
    const uint8_t *planes[3] = { px, NULL, NULL };
    const int strides[3] = { stride, 0, 0 };
    ssize_t r = je_run(e, planes, strides, w, h, 0);
    free(e);
    return r;
}

ssize_t jpeg_encode_yuv420(const uint8_t *y, int ys,
                           const uint8_t *cb, int cs,
                           const uint8_t *cr, int cs2,
                           int w, int h, int quality,
                           uint8_t *out, size_t cap)
{
    struct jenc *e = calloc(1, sizeof(*e));
    if (!e) return -1;
    e->out = out; e->cap = cap; e->len = 0; e->quality = quality;
    const uint8_t *planes[3] = { y, cb, cr };
    const int strides[3] = { ys, cs, cs2 };
    ssize_t r = je_run(e, planes, strides, w, h, 1);
    free(e);
    return r;
}

ssize_t jpeg_encode_rgb24(const uint8_t *px, int w, int h, int stride,
                          int quality, uint8_t *out, size_t cap)
{
    int cw = (w + 1) / 2, ch = (h + 1) / 2;
    uint8_t *yp = malloc((size_t)w * h), *cbp = malloc((size_t)cw * ch),
            *crp = malloc((size_t)cw * ch);
    if (!yp || !cbp || !crp) { free(yp); free(cbp); free(crp); return -1; }
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            const uint8_t *p = &px[y * stride + 3 * x];
            double r = p[0], g = p[1], b = p[2];
            yp[y * w + x] = (uint8_t)(0.299 * r + 0.587 * g + 0.114 * b);
            if ((y & 1) == 0 && (x & 1) == 0) {
                cbp[(y / 2) * cw + x / 2] =
                    (uint8_t)(-0.168736 * r - 0.331264 * g + 0.5 * b + 128.0);
                crp[(y / 2) * cw + x / 2] =
                    (uint8_t)(0.5 * r - 0.418688 * g - 0.081312 * b + 128.0);
            }
        }
    ssize_t r = jpeg_encode_yuv420(yp, w, cbp, cw, crp, cw, w, h,
                                   quality, out, cap);
    free(yp); free(cbp); free(crp);
    return r;
}

#endif /* JPEGENC_H */
