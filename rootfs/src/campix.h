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
#include <stdlib.h>
#include <string.h>
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
 * 0.587 mg + 0.114 mb — the number AEC drives at ~50.
 *
 * M47⑤h: quads whose brightest site reaches 200 are the EMISSIVE pixels
 * (screens, lamps) — the light, not a gray reference. A green-terminal
 * monitor as the room's only source used to drag the global means until
 * white cables rendered sage (device 2026-09-05: cable sensor (85,108,53)
 * vs the screen's own white — gains fit to everything left mid-tones 14%
 * green). But hard exclusion overshoots the other way (same device, next
 * hour: screen band G-R -49, magenta — screen white and desk reflectance
 * are simply DIFFERENT chroma, neither is gray), so emissive quads carry
 * half weight instead: robust-statistics down-weighting, gains land
 * between the emitter's point and the reflectors', no region more than
 * ~10 off neutral — what a phone renders in a monitor-lit room. yavg
 * stays whole-frame: AEC meters the scene as shot. */
static inline void cp_wb_measure(const uint8_t *g, uint32_t w, uint32_t h,
                                 float wb[3], double *yavg)
{
    double sr = 0, sg = 0, sb = 0, er = 0, eg = 0, eb = 0;
    uint64_t nr = 0, ne = 0;
    for (uint32_t y = 0; y + 1 < h; y += 2)
        for (uint32_t x = 0; x + 1 < w; x += 2) {
            size_t i0 = (size_t)y * w + x;
            int r0 = g[i0], g1 = g[i0 + 1], g2 = g[i0 + w], b3 = g[i0 + w + 1];
            int mx = r0 > g1 ? (r0 > b3 ? r0 : b3) : (g1 > b3 ? g1 : b3);
            if (mx < 200) {
                sr += r0;
                sg += g1 + g2;
                sb += b3;
                nr++;
            } else { /* emissive quads, plain sums — weight applied at use */
                er += r0;
                eg += g1 + g2;
                eb += b3;
                ne++;
            }
        }
    if (!nr && !ne) {
        wb[0] = wb[1] = wb[2] = 1.0f;
        if (yavg) *yavg = 0;
        return;
    }
    if (yavg) {
        double mr = (sr + er) / (double)(nr + ne);
        double mg = (sg + eg) / (2.0 * (nr + ne));
        double mb = (sb + eb) / (double)(nr + ne);
        *yavg = 0.299 * mr + 0.587 * mg + 0.114 * mb;
    }
    /* WB means: emissive quads count half (the 2x on er/eg/eb) */
    double mr = (sr + 2 * er) / (double)(nr + 2 * ne);
    double mg = (sg + 2 * eg) / (2.0 * (nr + 2 * ne));
    double mb = (sb + 2 * eb) / (double)(nr + 2 * ne);
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

/* ---- color correction (M47⑤g) ---- */

/* Google's own CCMs for THIS sensor+module, extracted from the device's
 * vendor image (redfin U1B2 factory, /lib64/camera/com.google.ghawb.tuning.
 * imx363.so — float32 scan, each row sums to exactly 1.0, CCT bracket in
 * the int header ahead of each matrix, 2026-09-05):
 *   D65  @ CCT [5800, 7100] daylight
 *   TL84 @ CCT [3600, 4600] warm fluorescent
 *   INC  @ CCT [2500, 3700] incandescent
 * Gray-world WB balances channel MEANS but cannot fix hue/saturation
 * distortion — that's what these matrices do (the missing piece behind
 * 「颜色还是偏绿」: without a CCM, green stays over-saturated relative to
 * R/B because the sensor's spectral overlaps are never undone). */
static const float CP_CCM_D65[9] = {
    1.5963f, -0.4867f, -0.1096f,
    -0.1936f, 1.4112f, -0.2176f,
    -0.0057f, -0.5851f, 1.5908f,
};
static const float CP_CCM_TL84[9] = {
    1.5485f, -0.3968f, -0.1517f,
    -0.1862f, 1.3719f, -0.2027f,
    0.0109f, -0.6279f, 1.6170f,
};
static const float CP_CCM_INC[9] = {
    1.7632f, -0.5245f, -0.2387f,
    -0.2382f, 1.3691f, -0.1309f,
    -0.0353f, -0.8022f, 1.8375f,
};

/* Pick a CCM from the gray-world WB gains — the poor man's CCT estimate.
 * A warm scene lights R brightest (r-gain 1) and leaves B starved (b-gain
 * large), so warmth = wb_b/wb_r: ~1.0 daylight, ~1.8 TL84/LED, ~3+
 * tungsten. Piecewise-linear through the three Google knots; anything
 * cooler than daylight clamps at D65 (the coolest matrix they ship). */
static inline void cp_ccm_for_wb(const float wb[3], float out[9])
{
    const float *m;
    float r = wb[0] > 0.01f ? wb[0] : 0.01f;
    float warm = wb[2] / r;
    const float *hi = NULL;
    float k = 0.0f;
    if (warm <= 1.0f) {
        m = CP_CCM_D65;
    } else if (warm < 1.8f) {
        m = CP_CCM_D65; hi = CP_CCM_TL84; k = (warm - 1.0f) / 0.8f;
    } else if (warm < 3.0f) {
        m = CP_CCM_TL84; hi = CP_CCM_INC; k = (warm - 1.8f) / 1.2f;
    } else {
        m = CP_CCM_INC;
    }
    if (!hi) {
        for (int i = 0; i < 9; i++) out[i] = m[i];
        return;
    }
    for (int i = 0; i < 9; i++) out[i] = m[i] * (1.0f - k) + hi[i] * k;
}

/* per-frame color transform: WB tables + CCM (Q14 int16) + gamma LUT.
 * The old chain fused WB*gamma into three DIAGONAL per-channel tables;
 * a CCM mixes channels, so the fused tables die and the pipeline becomes
 *   reconstruct -> wR/wG/wB -> CCM -> shadow desat -> highlight desat -> lut.
 * With ccm==NULL the tables clamp at 255, the matrix is identity and the
 * output is BIT-EXACT with the old fused path (the diff harness pins this).
 *
 * M47⑤h, two CCM-mode-only upgrades (both no-ops in the legacy path):
 *   - the WB tables clamp at 1020, not 255. A per-channel clamp BEFORE the
 *     CCM pins two channels at 255 while G keeps climbing — the ratio the
 *     matrix (and the highlight desat) sees is already destroyed. Letting
 *     gains run to 4x keeps the true ratios alive until the symmetric
 *     255/max desat does the one correct clamp.
 *   - shadow desat: the Google matrices carry -0.4..-0.6 G cross-terms; on
 *     a G-dominant shadow pixel (the noise floor IS green-dominant after
 *     gray-world) the R row goes negative and clamps at 0 asymmetrically —
 *     measured device 2026-09-05, darks G-R +13 -> +23 the moment a CCM
 *     turned on. Phones blend the CCM out in shadows; here the pixel's own
 *     pre-CCM luma picks the blend: full neutral below linear ~0, none at
 *     KNEE (20), neutral = the CCM output's own mean (rows sum to 1, so
 *     luma is preserved — blacks stay black, color noise dies). */
#define CP_SHADOW_KNEE 20
#define CP_WB_MAX_CCM 1020 /* 4x headroom — matches the WB gain cap */

/* M47⑤i divide-kill: both per-pixel integer divides below are precomputed
 * into tables — identical integer math, filled once per frame, so the
 * output is bit-exact with the dividing version (campix_test pins both).
 * s8[mx] = (255<<8)/mx for the highlight desat (mx spans 256..CP_DESAT_MAX;
 * the Google matrices' worst row-abs-sum ~2.53 on inputs <=1020 caps mx
 * ~2600 — entries above that exist but stay unread; anything past the table
 * falls back to the divide). f8[ypre] = (ypre<<8)/shadow for the shadow
 * taper. CP_DESAT_MAX must be a power of two so the bounds check is a mask
 * away; 4096 u16 = 8 KB per frame's xform. */
#define CP_DESAT_MAX 4096

struct cp_xform {
    uint16_t wr[256], wg[256], wb16[256]; /* v -> clamp(v*gain + 0.5) */
    int16_t m[9];                         /* CCM * 16384 */
    int shadow;                           /* shadow-desat knee, 0 = off */
    uint16_t f8[CP_SHADOW_KNEE];          /* (i<<8)/shadow taper */
    uint16_t s8[CP_DESAT_MAX];            /* (255<<8)/mx highlight desat */
    uint8_t lut[256];                     /* display gamma */
};

static inline void cp_xform_init(struct cp_xform *t, const float wb[3],
                                 const float ccm[9], const uint8_t lut[256])
{
    const float w[3] = { wb[0], wb[1], wb[2] };
    int hi = ccm ? CP_WB_MAX_CCM : 255;
    uint16_t *tab[3] = { t->wr, t->wg, t->wb16 };
    for (int c = 0; c < 3; c++) {
        if (!(w[c] >= 0.0f)) { /* NaN/negative guard */
            for (int v = 0; v < 256; v++) tab[c][v] = (uint16_t)v;
            continue;
        }
        for (int v = 0; v < 256; v++) {
            int x = (int)(v * w[c] + 0.5f);
            tab[c][v] = x > hi ? (uint16_t)hi : (uint16_t)(x < 0 ? 0 : x);
        }
    }
    if (ccm) {
        for (int i = 0; i < 9; i++) {
            int q = (int)(ccm[i] * 16384.0f + (ccm[i] >= 0 ? 0.5f : -0.5f));
            t->m[i] = q > 32767 ? 32767 : (q < -32768 ? -32768 : (int16_t)q);
        }
        t->shadow = CP_SHADOW_KNEE;
        for (int i = 0; i < t->shadow && i < CP_SHADOW_KNEE; i++)
            t->f8[i] = (uint16_t)((i << 8) / t->shadow);
    } else {
        for (int i = 0; i < 9; i++) t->m[i] = 0;
        t->m[0] = t->m[4] = t->m[8] = 16384;
        t->shadow = 0;
    }
    for (int mx = 256; mx < CP_DESAT_MAX; mx++)
        t->s8[mx] = (uint16_t)((255 << 8) / mx);
    for (int v = 0; v < 256; v++) t->lut[v] = lut[v];
}

/* WB -> CCM -> shadow desat -> highlight desat -> gamma for one
 * reconstructed linear pixel. The highlight desat is the symmetric clamp:
 * R/B sit farther from their saturation point than G after gray-world
 * gains, so a hard per-channel clamp turns bright neutral areas GREEN
 * (only G left standing) — instead scale the whole pixel by 255/max so
 * highlights keep their hue and wash toward white like every phone ISP
 * does. */
static inline void cp_apply_xform(const struct cp_xform *t, int R, int G, int B,
                                  uint8_t *out, uint16_t *o565)
{
    int r = t->wr[R], g = t->wg[G], b = t->wb16[B];
    const int16_t *m = t->m;
    int rr = (m[0] * r + m[1] * g + m[2] * b + 8192) >> 14;
    int gg = (m[3] * r + m[4] * g + m[5] * b + 8192) >> 14;
    int bb = (m[6] * r + m[7] * g + m[8] * b + 8192) >> 14;
    if (t->shadow) {
        /* pre-CCM luma of the WB'd values: Rec601 in 8.8 fixed point.
         * r,g,b are table outputs (>=0) and ypre < shadow <= KNEE, so the
         * f8 index is in range by construction. */
        int ypre = (r * 77 + g * 151 + b * 28) >> 8;
        if (ypre < t->shadow) {
            int f = t->f8[ypre];           /* 0..255, linear taper */
            int n = (rr + gg + bb) / 3;    /* the neutral target */
            rr = n + (((rr - n) * f + 128) >> 8);
            gg = n + (((gg - n) * f + 128) >> 8);
            bb = n + (((bb - n) * f + 128) >> 8);
        }
    }
    int mx = rr > gg ? (rr > bb ? rr : bb) : (gg > bb ? gg : bb);
    if (mx > 255) {
        int s = mx < CP_DESAT_MAX ? t->s8[mx] : (255 << 8) / mx;
        rr = (rr * s + 128) >> 8;
        gg = (gg * s + 128) >> 8;
        bb = (bb * s + 128) >> 8;
    }
    uint8_t R8 = t->lut[rr < 0 ? 0 : rr > 255 ? 255 : rr];
    uint8_t G8 = t->lut[gg < 0 ? 0 : gg > 255 ? 255 : gg];
    uint8_t B8 = t->lut[bb < 0 ? 0 : bb > 255 ? 255 : bb];
    if (out) {
        out[0] = R8;
        out[1] = G8;
        out[2] = B8;
    }
    if (o565)
        *o565 = (uint16_t)(((R8 & 0xF8) << 8) | ((G8 & 0xFC) << 3) | (B8 >> 3));
}

/* ---- debayer + rotate + scale (single pass) ---- */

/* One-pixel reconstruction: identical math to the M47② original (RGGB
 * bilinear-in-Bayer-domain; site classification on the UNCLAMPED rounded
 * coordinate; neighbor fetches clamp), restructured for speed (M47⑤e,
 * 2026-09-05 device probe: this pass was the whole fps budget — 60fps
 * capture arrived at 10.5fps published):
 *   - the color transform (WB tables + CCM + gamma) precomputed per frame
 *     in a cp_xform (M47⑤g — the diagonal-table fusion had to die for the
 *     CCM to exist)
 *   - the centroid map precomputed as per-column arrays + one row scalar,
 *     so the double coordinate math runs ow times per frame, not per pixel
 *   - optional RGB565 packed inline (the display frame) — kills the old
 *     separate conversion walk
 * xpar/ypar carry the parity of the unrounded source coordinate (border
 * pixels keep their pre-clamp Bayer phase, as the original did). out may
 * be NULL — display-only frames skip the RGB888 stores entirely (M47⑤e). */
static inline void cp_px(const uint8_t *g, uint32_t w,
                         int xc, int xm, int xp, int xpar,
                         int yc, int ym, int yp, int ypar,
                         const struct cp_xform *t, uint8_t *out, uint16_t *o565)
{
    int l = g[(size_t)yc * w + xm], r = g[(size_t)yc * w + xp];
    int u = g[(size_t)ym * w + xc], d = g[(size_t)yp * w + xc];
    int ul = g[(size_t)ym * w + xm], ur = g[(size_t)ym * w + xp];
    int dl = g[(size_t)yp * w + xm], dr = g[(size_t)yp * w + xp];
    int c = g[(size_t)yc * w + xc];
    int R, G, B;
    if (!(ypar & 1)) {          /* R row: even col = R site, odd = G */
        if (!(xpar & 1)) { R = c; G = (l + r + u + d) / 4; B = (ul + ur + dl + dr) / 4; }
        else            { G = c; R = (l + r) / 2;         B = (u + d) / 2; }
    } else {                    /* B row: even col = G, odd = B site */
        if (!(xpar & 1)) { G = c; B = (l + r) / 2;         R = (u + d) / 2; }
        else            { B = c; G = (l + r + u + d) / 4; R = (ul + ur + dl + dr) / 4; }
    }
    cp_apply_xform(t, R, G, B, out, o565);
}

/* One pass over the OUTPUT image: centroid-map each output pixel back into
 * the (rotated) source, inverse-rotate into the source grid, reconstruct
 * RGB there, then WB -> CCM -> desat -> gamma LUT (a per-frame cp_xform),
 * and (out565) the RGB565 display frame.
 *
 *   rot 0:   out (ow,oh) samples source (w,h) directly (pure scale;
 *            ow=w,oh=h is the identity pass)
 *   rot 90:  source rotated 90 deg CLOCKWISE — landscape sensor frame
 *            becomes portrait
 *   rot 270: same, counter-clockwise
 *
 * ccm selects the color matrix (NULL = identity — the legacy look; the
 * Google per-CCT matrices arrive via cp_ccm_for_wb).
 *
 * M47⑤i: split into cp_rot_init (geometry + column map + xform) /
 * cp_rot_rows (a [y0,y1) output-row range) / cp_rot_free so callers can fan
 * the row walk out over threads — rows are independent (each derives its own
 * row scalar from oy; the column map is read-only), so ANY partition is
 * bit-exact with the single-threaded walk. cp_debayer_rot below remains the
 * one-call form with the original signature (the diff harness pins it
 * against git HEAD).
 *
 * The caller owns out (ow*oh*3, or NULL for 565-only display frames) and,
 * when used, out565 (ow*oh u16) — both passed as BASE pointers; cp_rot_rows
 * offsets by the row range itself.
 * Sampling is nearest-neighbor (centroid map) — at downscale it decimates
 * rather than averages, fine at the scales we ship (>=0.5x). */
struct cp_rot {
    int32_t *cmap;              /* ow*4: cv|cm|cp|cq column map, malloc'd */
    struct cp_xform xf;         /* per-frame color transform */
    uint32_t w, h;              /* source dims */
    uint32_t ow, oh;            /* output dims */
    uint32_t rh;                /* row-dim of the pre-rotation frame */
    uint32_t rd;                /* clamp range of the row coordinate (= rh) */
    int rot;
};

/* returns 0 on bad dims or OOM (out/out565 must then not be touched) */
static inline int cp_rot_init(struct cp_rot *R, uint32_t w, uint32_t h,
                              const float wb[3], const float ccm[9],
                              const uint8_t lut[256],
                              int rot, uint32_t ow, uint32_t oh)
{
    memset(R, 0, sizeof *R);
    if (!ow || !oh || !w || !h)
        return 0;
    /* rotated-frame dims */
    uint32_t rw = (rot == 90 || rot == 270) ? h : w;
    R->w = w;
    R->h = h;
    R->ow = ow;
    R->oh = oh;
    R->rh = (rot == 90 || rot == 270) ? w : h;
    R->rd = R->rh;
    R->rot = rot;

    /* per-frame color transform (WB tables + CCM + gamma, M47⑤g) */
    cp_xform_init(&R->xf, wb, ccm, lut);

    /* column map: for each output column, the source coordinate that varies
     * along it (x for rot 0, y for 90/270), computed with the original
     * formulas (double math, ow times — cheap) and stored as raw parity +
     * clamped center/neighbor fetch offsets. 4 arrays of ow entries. */
    R->cmap = (int32_t *)malloc((size_t)ow * 4 * sizeof(int32_t));
    if (!R->cmap)
        return 0;
    int32_t *cv = R->cmap, *cm = R->cmap + ow, *cp = R->cmap + 2 * ow,
            *cq = R->cmap + 3 * ow;
    /* source dim of the column-varying coordinate */
    const uint32_t cd = (rot == 90 || rot == 270) ? h : w;
    for (uint32_t o = 0; o < ow; o++) {
        double t = ((double)o + 0.5) * (double)rw / (double)ow - 0.5;
        /* rot 0: sx = t (x varies); rot 90: sy = h-1-rx, rx = t; the ROUNDING
         * of either lands on the same integer lattice, and rot 270's
         * sy = rx rounds identically — only the traversal direction differs,
         * which the per-column table absorbs. */
        double s = (rot == 90) ? (double)cd - 1.0 - t : t;
        int64_t v = (int64_t)(s < 0 ? -1 : (s + 0.5));
        int cl = v < 0 ? 0 : (v > (int64_t)cd - 1 ? (int)cd - 1 : (int)v);
        int m = (int)(v - 1 < 0 ? 0 : (v - 1 > (int64_t)cd - 1 ? (int)cd - 1 : v - 1));
        int p = (int)(v + 1 < 0 ? 0 : (v + 1 > (int64_t)cd - 1 ? (int)cd - 1 : v + 1));
        cv[o] = cl;
        cm[o] = m;
        cp[o] = p;
        cq[o] = (int32_t)(v & 1);
    }
    return 1;
}

/* rows [y0,y1) of the output. Reads R + g (and the column map) only;
 * writes only rows [y0,y1) of out/out565. Thread-safe for disjoint ranges.
 *
 * The rotated branches walk SOURCE COLUMNS (x = row scalar): every one of
 * the 9 per-pixel loads strides w bytes — a cache miss each; the M47⑤i
 * device era measured 17.4 ms/frame for this pass even pinned to the big
 * cores at max (memory-LATENCY bound, not CPU bound — a register-only burn
 * probe scaled 2.9x on the same cores while this didn't). So the rotated
 * walk is CACHE-BLOCKED: output rows go in groups of CP_ROT_GROUP; a
 * group reads a narrow band of source columns, staged once with one
 * memcpy per source row (~1 cache line each), and the group's whole 9-load
 * pattern then lands in a ~20 KB strip that sits in L1. Per-pixel math,
 * visit values, and outputs are UNCHANGED — any partition of rows is
 * still bit-exact with the naive walk (cp_rot_rows_naive, kept as the
 * test reference). rot 0 is already row-sequential and walks directly. */
#define CP_ROT_GROUP 8

static inline void cp_rot_yscalar(const struct cp_rot *R, uint32_t oy,
                                  int *cl, int *m, int *p, int *rq)
{
    double t = ((double)oy + 0.5) * (double)R->rh / (double)R->oh - 0.5;
    double s = (R->rot == 270) ? (double)R->rd - 1.0 - t : t;
    int64_t v = (int64_t)(s < 0 ? -1 : (s + 0.5));
    *cl = v < 0 ? 0 : (v > (int64_t)R->rd - 1 ? (int)R->rd - 1 : (int)v);
    *m = (int)(v - 1 < 0 ? 0 : (v - 1 > (int64_t)R->rd - 1 ? (int)R->rd - 1 : v - 1));
    *p = (int)(v + 1 < 0 ? 0 : (v + 1 > (int64_t)R->rd - 1 ? (int)R->rd - 1 : v + 1));
    *rq = (int)(v & 1);
}

/* the naive (unblocked) row walk — the M47⑤i original. Kept as the
 * bit-exact reference for campix_test; the blocked path above must match
 * it byte for byte on every partition. */
static inline void cp_rot_rows_naive(const struct cp_rot *R, const uint8_t *g,
                                     uint32_t y0, uint32_t y1,
                                     uint8_t *out, uint16_t *out565)
{
    const int32_t *cv = R->cmap, *cm = R->cmap + R->ow,
                  *cp = R->cmap + 2 * R->ow, *cq = R->cmap + 3 * R->ow;
    const uint32_t w = R->w, ow = R->ow;
    for (uint32_t oy = y0; oy < y1; oy++) {
        int cl, m, p, rq;
        cp_rot_yscalar(R, oy, &cl, &m, &p, &rq);
        uint8_t *orow = out ? out + (size_t)oy * ow * 3 : NULL;
        uint16_t *p5row = out565 ? out565 + (size_t)oy * ow : NULL;
        if (R->rot == 0) {
            for (uint32_t ox = 0; ox < ow; ox++)
                cp_px(g, w, cv[ox], cm[ox], cp[ox], cq[ox],
                      cl, m, p, rq, &R->xf,
                      orow ? orow + ox * 3 : NULL, p5row ? &p5row[ox] : NULL);
        } else {
            for (uint32_t ox = 0; ox < ow; ox++)
                cp_px(g, w, cl, m, p, rq,
                      cv[ox], cm[ox], cp[ox], cq[ox], &R->xf,
                      orow ? orow + ox * 3 : NULL, p5row ? &p5row[ox] : NULL);
        }
    }
}

static inline void cp_rot_rows(const struct cp_rot *R, const uint8_t *g,
                               uint32_t y0, uint32_t y1,
                               uint8_t *out, uint16_t *out565)
{
    const int32_t *cv = R->cmap, *cm = R->cmap + R->ow,
                  *cp = R->cmap + 2 * R->ow, *cq = R->cmap + 3 * R->ow;
    const uint32_t ow = R->ow;
    if (R->rot == 0) {
        /* X = column map (x), Y = row scalar (y) — row-sequential already */
        for (uint32_t oy = y0; oy < y1; oy++) {
            int cl, m, p, rq;
            cp_rot_yscalar(R, oy, &cl, &m, &p, &rq);
            uint8_t *orow = out ? out + (size_t)oy * ow * 3 : NULL;
            uint16_t *p5row = out565 ? out565 + (size_t)oy * ow : NULL;
            for (uint32_t ox = 0; ox < ow; ox++)
                cp_px(g, R->w, cv[ox], cm[ox], cp[ox], cq[ox],
                      cl, m, p, rq, &R->xf,
                      orow ? orow + ox * 3 : NULL, p5row ? &p5row[ox] : NULL);
        }
        return;
    }
    /* rotated: blocked staging walk (see the block comment above).
     * Worst-case band one group can span (upscale-safe: ceil step per row,
     * +1 either side); capped at w — a span over that means the defensive
     * fallback below handles the group unblocked. */
    uint32_t step = (R->rh + R->oh - 1) / R->oh;
    uint32_t cap = CP_ROT_GROUP * step + 4;
    if (cap > R->w) cap = R->w;
    uint8_t *stage = (uint8_t *)malloc((size_t)R->h * cap);
    for (uint32_t gy = y0; gy < y1; gy += CP_ROT_GROUP) {
        uint32_t ge = gy + CP_ROT_GROUP;
        if (ge > y1) ge = y1;
        int cl0, m0, p0, rq0, cl1, m1, p1, rq1;
        cp_rot_yscalar(R, gy, &cl0, &m0, &p0, &rq0);
        cp_rot_yscalar(R, ge - 1, &cl1, &m1, &p1, &rq1);
        int lo = cl0 < cl1 ? cl0 : cl1, hi = cl0 > cl1 ? cl0 : cl1;
        int x0 = lo > 0 ? lo - 1 : 0;
        int x1 = hi < (int)R->w - 1 ? hi + 1 : (int)R->w - 1;
        uint32_t sw = (uint32_t)(x1 - x0 + 1);
        if (stage && sw <= cap) {
            /* stage the band: one memcpy per source row (~1 line each) */
            for (uint32_t r = 0; r < R->h; r++)
                memcpy(stage + (size_t)r * sw,
                       g + (size_t)r * R->w + x0, sw);
            for (uint32_t oy = gy; oy < ge; oy++) {
                int cl, m, p, rq;
                cp_rot_yscalar(R, oy, &cl, &m, &p, &rq);
                uint8_t *orow = out ? out + (size_t)oy * ow * 3 : NULL;
                uint16_t *p5row = out565 ? out565 + (size_t)oy * ow : NULL;
                for (uint32_t ox = 0; ox < ow; ox++)
                    cp_px(stage, sw, cl - x0, m - x0, p - x0, rq,
                          cv[ox], cm[ox], cp[ox], cq[ox], &R->xf,
                          orow ? orow + ox * 3 : NULL, p5row ? &p5row[ox] : NULL);
            }
        } else {
            /* OOM or an extreme span: walk the group directly — same bytes */
            for (uint32_t oy = gy; oy < ge; oy++) {
                int cl, m, p, rq;
                cp_rot_yscalar(R, oy, &cl, &m, &p, &rq);
                uint8_t *orow = out ? out + (size_t)oy * ow * 3 : NULL;
                uint16_t *p5row = out565 ? out565 + (size_t)oy * ow : NULL;
                for (uint32_t ox = 0; ox < ow; ox++)
                    cp_px(g, R->w, cl, m, p, rq,
                          cv[ox], cm[ox], cp[ox], cq[ox], &R->xf,
                          orow ? orow + ox * 3 : NULL, p5row ? &p5row[ox] : NULL);
            }
        }
    }
    free(stage);
}

static inline void cp_rot_free(struct cp_rot *R)
{
    free(R->cmap);
    R->cmap = NULL;
}

/* the original one-call form — single-threaded, byte-for-byte the M47⑤e
 * behavior (init + all rows + free) */
static inline void cp_debayer_rot(const uint8_t *g, uint32_t w, uint32_t h,
                           const float wb[3], const float ccm[9],
                           const uint8_t lut[256],
                           int rot, uint32_t ow, uint32_t oh, uint8_t *out,
                           uint16_t *out565)
{
    struct cp_rot R;
    if (!cp_rot_init(&R, w, h, wb, ccm, lut, rot, ow, oh))
        return;
    cp_rot_rows(&R, g, 0, oh, out, out565);
    cp_rot_free(&R);
}

/* crop geometry: the largest centered region of a (w,h) frame matching the
 * aspect aw:ah IN SENSOR (pre-rotation) DOMAIN — a caller producing a
 * rotated portrait output (rot 90/270) passes the SWAPPED aspect, and the
 * centered slice cut here becomes the portrait frame after rotation.
 * Real dims (rear #544): 2016x1136 sensor, viewfinder output 1080:1456 ->
 * pass 1456:1080 here -> 1530x1136 centered at x0=242 -> rot90 ->
 * 1136x1530 (0.7425, matches 1080/1456 to 3 decimals).
 * cw/ch AND x0/y0 are even-snapped: an odd x0 flips the Bayer phase of the
 * whole crop and the site classifier mislabels G as R/B (green tint,
 * device 2026-09-05). */
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
    if (*cw & 1) (*cw)--;
    if (*ch & 1) (*ch)--;
    *x0 = ((w - *cw) / 2) & ~1u;
    *y0 = ((h - *ch) / 2) & ~1u;
}

#endif /* CAMPIX_H */
