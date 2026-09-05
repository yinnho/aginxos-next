/* campix.h — cam-shot's pure pixel-chain library (M47②).
 *
 * Everything here is I/O-free and global-free so it builds and runs on the
 * host (campix_test.c, wired into scripts/check.sh) — the geometry and
 * LUT math is where the "normal phone camera" quality work lives, and it
 * must be testable without a device.
 *
 * Domain walk:
 *   RAW10 RDI buffer (5 B per 4 px, bits[9:2] per pixel)
 *     -> gray8 crop, black-level-subtracted + renormalized (LINEAR domain —
 *        WB measurement and AEC stats live here; target mean ~50/255)
 *     -> per-site gray-world WB gains
 *     -> debayer + rotate + scale in one pass (nearest-neighbor sampling of
 *        the bilinear-in-Bayer-domain reconstruction), WB multiply, then the
 *        display LUT (gamma 1/g lifts the linear raw onto the display curve)
 *
 * The old chain dropped bits[1:0] and fed the encoder raw: no black level
 * (optical black sits ~16 in the 8-bit domain on the rear imx363 — darks
 * washed gray-green), no gamma (a linear mean-19 frame shows as mud). These
 * LUTs are the whole fix for two of the three M47 defects.
 */
#ifndef CAMPIX_H
#define CAMPIX_H

#include <stdint.h>
#include <stddef.h>
#include <math.h>

/* static inline throughout: this is a header library shared by cam-shot.c
 * (device) and campix_test.c (host); any TU may use only part of it, and
 * plain static functions would warn unused there. */

/* ---- LUTs ---- */

/* Linear LUT: black level out, range renormalized onto 0..255.
 * v <= bl -> 0, else (v-bl)*255/(255-bl). clamps bl into [0,254]. */
static inline void cp_lut_linear(uint8_t lut[256], int bl)
{
    if (bl < 0) bl = 0;
    if (bl > 254) bl = 254;
    for (int v = 0; v < 256; v++)
        lut[v] = v <= bl ? 0 : (uint8_t)((v - bl) * 255 / (255 - bl));
}

/* Display LUT: normalized linear -> 255*(x/255)^(1/g).
 * g <= 1.0 is identity (gamma off — the old look). */
static inline void cp_lut_gamma(uint8_t lut[256], double g)
{
    if (g <= 1.0) {
        for (int v = 0; v < 256; v++) lut[v] = (uint8_t)v;
        return;
    }
    for (int v = 0; v < 256; v++) {
        double x = (double)v / 255.0;
        double y = 255.0 * pow(x, 1.0 / g);
        int iv = (int)(y + 0.5);
        lut[v] = iv < 0 ? 0 : (iv > 255 ? 255 : (uint8_t)iv);
    }
}

/* compose out[i] = b[a[i]] */
static inline void cp_lut_compose(const uint8_t a[256], const uint8_t b[256],
                                  uint8_t out[256])
{
    for (int i = 0; i < 256; i++) out[i] = b[a[i]];
}

/* ---- extraction ---- */

/* RAW10 (5 B / 4 px, pixel i of the group = byte i, bits[9:2]) -> gray8
 * crop (x0,y0,cw,ch) through a LUT. out is cw*ch. x0/y0 need no alignment;
 * groups are 4-px so a sub-group x0 reads its own byte. */
static inline void cp_raw10_gray(const uint8_t *raw, uint32_t stride,
                          uint32_t x0, uint32_t y0, uint32_t cw, uint32_t ch,
                          const uint8_t lut[256], uint8_t *out)
{
    for (uint32_t y = 0; y < ch; y++) {
        const uint8_t *r = raw + (size_t)(y0 + y) * stride;
        uint8_t *o = out + (size_t)y * cw;
        uint32_t x = 0;
        /* head: up to the next 4-px group boundary */
        for (; x < cw && ((x0 + x) & 3); x++) {
            const uint8_t *p = r + (size_t)((x0 + x) / 4) * 5;
            o[x] = lut[p[(x0 + x) & 3]];
        }
        for (; x + 4 <= cw; x += 4) {
            const uint8_t *p = r + (size_t)((x0 + x) / 4) * 5;
            o[x + 0] = lut[p[0]];
            o[x + 1] = lut[p[1]];
            o[x + 2] = lut[p[2]];
            o[x + 3] = lut[p[3]];
        }
        for (; x < cw; x++) {
            const uint8_t *p = r + (size_t)((x0 + x) / 4) * 5;
            o[x] = lut[p[(x0 + x) & 3]];
        }
    }
}

/* plain full-frame variant (LUT optional: NULL = copy bits[9:2] as-is) */
static inline void cp_raw10_gray_full(const uint8_t *raw, uint32_t w, uint32_t h,
                               uint32_t stride, const uint8_t lut[256],
                               uint8_t *out)
{
    static const uint8_t id[256] = {
#define R16(a) a,a,a,a,a,a,a,a,a,a,a,a,a,a,a,a
        R16(0),R16(1),R16(2),R16(3),R16(4),R16(5),R16(6),R16(7),
        R16(8),R16(9),R16(10),R16(11),R16(12),R16(13),R16(14),R16(15)
#undef R16
    };
    const uint8_t *l = lut ? lut : id;
    for (uint32_t y = 0; y < h; y++) {
        const uint8_t *r = raw + (size_t)y * stride;
        uint8_t *o = out + (size_t)y * w;
        for (uint32_t x = 0; x + 4 <= w; x += 4) {
            const uint8_t *p = r + (size_t)(x / 4) * 5;
            o[x + 0] = l[p[0]];
            o[x + 1] = l[p[1]];
            o[x + 2] = l[p[2]];
            o[x + 3] = l[p[3]];
        }
    }
}

/* ---- stats (LINEAR domain: feed the linear-LUT'd gray) ---- */

/* gray-world WB + luminance mean in one walk. Site means over the RGGB
 * quads (R at even/even, B at odd/odd, G double on the cross), gains
 * normalizing all three to the brightest site (>=1, never darkens; capped
 * 4x for tinted scenes, black frame -> all 1.0). *yavg = 0.299 mr +
 * 0.587 mg + 0.114 mb — the number AEC drives at ~50. */
static inline void cp_wb_measure(const uint8_t *g, uint32_t w, uint32_t h,
                                 float wb[3], double *yavg)
{
    double sr = 0, sg = 0, sb = 0;
    uint64_t nr = 0;
    for (uint32_t y = 0; y + 1 < h; y += 2)
        for (uint32_t x = 0; x + 1 < w; x += 2) {
            size_t i0 = (size_t)y * w + x;
            sr += g[i0];
            sg += g[i0 + 1] + g[i0 + w];
            sb += g[i0 + w + 1];
            nr++;
        }
    if (!nr) {
        wb[0] = wb[1] = wb[2] = 1.0f;
        if (yavg) *yavg = 0;
        return;
    }
    double mr = sr / nr, mg = sg / (2.0 * nr), mb = sb / nr;
    if (yavg) *yavg = 0.299 * mr + 0.587 * mg + 0.114 * mb;
    if (mr < 1.0 || mg < 1.0 || mb < 1.0) {
        wb[0] = wb[1] = wb[2] = 1.0f;
        return;
    }
    double top = mr > mg ? (mr > mb ? mr : mb) : (mg > mb ? mg : mb);
    wb[0] = (float)(top / mr);
    wb[1] = (float)(top / mg);
    wb[2] = (float)(top / mb);
    for (int k = 0; k < 3; k++)
        if (wb[k] > 4.0f) wb[k] = 4.0f;
}

/* plain luminance mean (gray path) */
static inline double cp_yavg(const uint8_t *g, size_t n)
{
    if (!n) return 0;
    double s = 0;
    for (size_t i = 0; i < n; i++) s += g[i];
    return s / (double)n;
}

/* ---- debayer + rotate + scale (single pass) ---- */

static inline uint8_t cp_at(const uint8_t *g, uint32_t w, uint32_t h,
                            int64_t x, int64_t y)
{
    if (x < 0) x = 0;
    if (y < 0) y = 0;
    if (x >= (int64_t)w) x = w - 1;
    if (y >= (int64_t)h) y = h - 1;
    return g[(size_t)y * w + (size_t)x];
}

/* One pass over the OUTPUT image: centroid-map each output pixel back into
 * the (rotated) source, inverse-rotate into the source grid, reconstruct
 * RGB there with the same RGGB bilinear-in-Bayer-domain scheme the old
 * cs_debayer used, multiply WB, apply the display LUT. No intermediate
 * full-res RGB plane exists.
 *
 *   rot 0:   out (ow,oh) samples source (w,h) directly (ow/oh may differ
 *            from w/h — pure scale; ow=w,oh=h is the identity pass)
 *   rot 90:  source rotated 90 deg CLOCKWISE — out (ow,oh) with
 *            ow:oh == h:w samples the rotated frame; a landscape sensor
 *            frame becomes portrait
 *   rot 270: same, counter-clockwise
 *
 * The caller owns out (ow*oh*3). Sampling is nearest-neighbor (centroid
 * map): at downscale it decimates rather than averages — fine at the
 * scales we ship (>=0.5x) and one quarter the code of an area filter. */
static inline void cp_debayer_rot(const uint8_t *g, uint32_t w, uint32_t h,
                           const float wb[3], const uint8_t lut[256],
                           int rot, uint32_t ow, uint32_t oh, uint8_t *out)
{
    /* rotated-frame dims */
    uint32_t rw = (rot == 90 || rot == 270) ? h : w;
    uint32_t rh = (rot == 90 || rot == 270) ? w : h;
    for (uint32_t oy = 0; oy < oh; oy++) {
        double ry = ((double)oy + 0.5) * (double)rh / (double)oh - 0.5;
        for (uint32_t ox = 0; ox < ow; ox++) {
            double rx = ((double)ox + 0.5) * (double)rw / (double)ow - 0.5;
            /* inverse-rotate rotated (rx,ry) -> source (sx,sy) */
            double sx, sy;
            if (rot == 90) {        /* fwd: (x,y)->(h-1-y,x); inv below */
                sx = ry;
                sy = (double)h - 1.0 - rx;
            } else if (rot == 270) { /* fwd: (x,y)->(y,w-1-x) */
                sx = (double)w - 1.0 - ry;
                sy = rx;
            } else {
                sx = rx;
                sy = ry;
            }
            int64_t x = (int64_t)(sx < 0 ? -1 : (sx + 0.5));
            int64_t y = (int64_t)(sy < 0 ? -1 : (sy + 0.5));
            /* same RGGB reconstruction as the old cs_debayer (verified
             * phase on device 2026-09-01) */
            int site = !(y & 1) ? (!(x & 1) ? 0 : 1) : (!(x & 1) ? 1 : 2);
            int R, G, B;
            int l = cp_at(g, w, h, x - 1, y), r = cp_at(g, w, h, x + 1, y);
            int u = cp_at(g, w, h, x, y - 1), d = cp_at(g, w, h, x, y + 1);
            int ul = cp_at(g, w, h, x - 1, y - 1), ur = cp_at(g, w, h, x + 1, y - 1);
            int dl = cp_at(g, w, h, x - 1, y + 1), dr = cp_at(g, w, h, x + 1, y + 1);
            if (site == 0) {
                R = cp_at(g, w, h, x, y);
                G = (l + r + u + d) / 4;
                B = (ul + ur + dl + dr) / 4;
            } else if (site == 2) {
                B = cp_at(g, w, h, x, y);
                G = (l + r + u + d) / 4;
                R = (ul + ur + dl + dr) / 4;
            } else if (!(y & 1)) {   /* G on R row */
                G = cp_at(g, w, h, x, y);
                R = (l + r) / 2;
                B = (u + d) / 2;
            } else {                 /* G on B row */
                G = cp_at(g, w, h, x, y);
                B = (l + r) / 2;
                R = (u + d) / 2;
            }
            int Rc = (int)(R * wb[0] + 0.5f);
            int Gc = (int)(G * wb[1] + 0.5f);
            int Bc = (int)(B * wb[2] + 0.5f);
            uint8_t *p = out + ((size_t)oy * ow + ox) * 3;
            p[0] = lut[Rc > 255 ? 255 : Rc];
            p[1] = lut[Gc > 255 ? 255 : Gc];
            p[2] = lut[Bc > 255 ? 255 : Bc];
        }
    }
}

/* crop geometry: the largest centered region of a (w,h) frame matching the
 * aspect aw:ah IN SENSOR (pre-rotation) DOMAIN — a caller producing a
 * rotated portrait output (rot 90/270) passes the SWAPPED aspect, and the
 * centered slice cut here becomes the portrait frame after rotation.
 * Real dims (rear #544): 2016x1136 sensor, viewfinder output 1080:1456 ->
 * pass 1456:1080 here -> 1531x1136 centered -> rot90 -> 1136x1531
 * (0.7420, matches 1080/1456 to 3 decimals). */
static inline void cp_crop_for_aspect(uint32_t w, uint32_t h, uint32_t aw, uint32_t ah,
                               uint32_t *x0, uint32_t *y0,
                               uint32_t *cw, uint32_t *ch)
{
    double want = (double)aw / (double)ah;
    double have = (double)w / (double)h;
    if (want >= have) {           /* crop rows */
        *cw = w;
        *ch = (uint32_t)((double)w / want + 0.5);
        if (*ch > h) *ch = h;
        if (*ch < 2) *ch = h < 2 ? h : 2;
    } else {                      /* crop cols */
        *ch = h;
        *cw = (uint32_t)((double)h * want + 0.5);
        if (*cw > w) *cw = w;
        if (*cw < 2) *cw = w < 2 ? w : 2;
    }
    *x0 = (w - *cw) / 2;
    *y0 = (h - *ch) / 2;
}

#endif /* CAMPIX_H */
