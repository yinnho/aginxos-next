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
 *     -> quad means with dark-floor gating + emissive soft-weight
 *     -> bayes CT search against Google's imx363 CT curve -> gains + CCM
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

/* Quad means for AWB + luminance mean in one walk. Site means over the
 * BGGR quads (B at even/even, R at odd/odd, G double on the cross);
 * *yavg =
 * 0.299 mr + 0.587 mg + 0.114 mb whole-frame — the number AEC drives at ~50.
 *
 * CFA ORIENTATION (device 2026-09-05, emissive color chart): the rear
 * imx363 RDI frame as delivered starts on a B site — effective BGGR. The
 * discriminator: under --no-ccm gray-world, measurement AND gains share
 * this table, so a wrong orientation swaps primaries SELF-CONSISTENTLY
 * (white invariant, red<->blue swapped) — no gray-card or green-cast test
 * can see it, which is exactly why M19c-2026-09-01's "RGGB verified"
 * (synthetic quadrants + green-cast WB fix) never caught it. This table
 * and cp_px MUST stay flipped together. Front/uw sensors: not probed.
 *
 * hist (optional, may be NULL): 256-bin histogram of EVERY gray byte, the
 * whole-frame luminance histogram the M47⑤k tone stretch consumes (RPi's
 * contrast algorithm eats the ISP yHist the same way) — free here because
 * this walk already touches every byte.
 *
 * Two stat-gating laws, both from the mature stacks (device-motivated
 * 2026-09-05, rendered-frame inversion of a warm-lamp + dark-room scene):
 *   - quads whose G-site mean is below CP_WB_MIN_G are the noise FLOOR, not
 *     the scene: after the single black-level subtraction the floor reads
 *     green ((r,g,b) resp ~(0.31,1,0.52) measured) and it carried no
 *     illuminant information — 68% of that frame, dragging gray-world until
 *     the warm source rendered green-cyan. The RPi AWB calls this min_g
 *     ("minimum G value of those pixels, to be regarded a 'useful'",
 *     libcamera awb_bayes.cpp ported-features note). Gated quads still count
 *     in yavg: AEC meters the scene as shot.
 *   - quads whose brightest site reaches 200 are the EMISSIVE pixels
 *     (screens, lamps) — the light, not a gray reference. Hard exclusion
 *     overshoots (screen white and desk reflectance are simply DIFFERENT
 *     chroma, neither is gray), so emissive quads carry half weight instead:
 *     robust-statistics down-weighting, means land between the emitter's
 *     point and the reflectors' (M47⑤h).
 * means[] is the soft-weighted (mr, mg, mb) in the absolute linear domain —
 * the bayes search consumes ratios; the legacy gray-world gains live in
 * cp_wb_gains_gray. All-dark frame -> means all 0 (the caller freezes). */
#define CP_WB_MIN_G 8

static inline void cp_wb_measure(const uint8_t *g, uint32_t w, uint32_t h,
                                 double means[3], double *yavg,
                                 uint32_t hist[256])
{
    double sr = 0, sg = 0, sb = 0, er = 0, eg = 0, eb = 0;
    uint64_t nr = 0, ne = 0, nf = 0;
    double fr = 0, fg = 0, fb = 0; /* floor quads: yavg only */
    for (uint32_t y = 0; y + 1 < h; y += 2)
        for (uint32_t x = 0; x + 1 < w; x += 2) {
            size_t i0 = (size_t)y * w + x;
            int b0 = g[i0], g1 = g[i0 + 1], g2 = g[i0 + w], r3 = g[i0 + w + 1];
            if (hist) {
                hist[b0]++;
                hist[g1]++;
                hist[g2]++;
                hist[r3]++;
            }
            int mx = b0 > g1 ? (b0 > r3 ? b0 : r3) : (g1 > r3 ? g1 : r3);
            if (mx < CP_WB_MIN_G) {
                fr += r3;
                fg += g1 + g2;
                fb += b0;
                nf++;
                continue; /* floor quad: no illuminant info */
            }
            if (mx < 200) {
                sr += r3;
                sg += g1 + g2;
                sb += b0;
                nr++;
            } else { /* emissive quads, plain sums — weight applied at use */
                er += r3;
                eg += g1 + g2;
                eb += b0;
                ne++;
            }
        }
    if (yavg) { /* whole-frame luminance, floor included */
        uint64_t n = nr + ne + nf;
        if (!n) {
            *yavg = 0;
        } else {
            double mr = (sr + er + fr) / (double)n;
            double mg = (sg + eg + fg) / (2.0 * (double)n);
            double mb = (sb + eb + fb) / (double)n;
            *yavg = 0.299 * mr + 0.587 * mg + 0.114 * mb;
        }
    }
    if (!nr && !ne) {
        means[0] = means[1] = means[2] = 0.0;
        return;
    }
    /* WB means: emissive quads count half (the 2x on er/eg/eb) */
    means[0] = (sr + 2 * er) / (double)(nr + 2 * ne);
    means[1] = (sg + 2 * eg) / (2.0 * (nr + 2 * ne));
    means[2] = (sb + 2 * eb) / (double)(nr + 2 * ne);
    if (means[0] < 1.0 && means[1] < 1.0 && means[2] < 1.0) {
        means[0] = means[1] = means[2] = 0.0; /* black-ish frame: freeze */
    }
}

/* The old gray-world gain law, kept for the no-CCM legacy path: normalize
 * all sites to the brightest mean (gains >= 1, never darken, cap 4x).
 * Black frame -> all 1.0. */
static inline void cp_wb_gains_gray(const double means[3], float wb[3])
{
    if (means[0] < 1.0 || means[1] < 1.0 || means[2] < 1.0) {
        wb[0] = wb[1] = wb[2] = 1.0f;
        return;
    }
    double top = means[0] > means[1] ? (means[0] > means[2] ? means[0] : means[2])
                                     : (means[1] > means[2] ? means[1] : means[2]);
    wb[0] = (float)(top / means[0]);
    wb[1] = (float)(top / means[1]);
    wb[2] = (float)(top / means[2]);
    for (int k = 0; k < 3; k++)
        if (wb[k] > 4.0f) wb[k] = 4.0f;
}

/* M47⑤j temporal CT smoothing — ported from libcamera libipa/awb.cpp,
 * AwbAlgorithmBase::process() (LGPL-2.1-or-later, Ideas On Board 2024 — the
 * AWB shared by rkisp1, the soft ISP and mali-c55):
 *   - "Minimum mean value below which AWB can't operate" -> FREEZE: too-dark
 *     stats carry no gray reference, the gains must not chase them.
 *   - "Smooth color gains adjustments": speed 0.2 EMA — libcamera smooths
 *     the gains AND the colour temperature; here gains are a pure function
 *     of CT (the vendor curve), so smoothing the CT alone keeps gains and
 *     the CCM pick consistent by construction.
 * Seed 5000 = libcamera kDefaultColourTemperature. Single-shot callers
 * behave exactly as before: the first valid measurement seeds the state. */
struct cp_ct_smooth { double ct; int primed; };

static inline void cp_ct_smooth_init(struct cp_ct_smooth *s)
{
    s->ct = 5000.0;
    s->primed = 0;
}

static inline void cp_ct_smooth_step(struct cp_ct_smooth *s,
                                     double meas_ct, double yavg)
{
    if (!(yavg >= 2.0))
        return; /* stats invalid (covered lens / black frame): freeze */
    if (!(meas_ct > 0.0))
        return; /* search found no usable stats: freeze */
    if (!s->primed) {
        s->ct = meas_ct;
        s->primed = 1;
        return;
    }
    const double speed = 0.2;
    s->ct = meas_ct * speed + s->ct * (1.0 - speed);
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

/* M47⑤j: Google's own imx363 CT calibration — the sensor's G-normalized
 * R/B response to gray under standard illuminants, extracted 2026-09-05
 * from the device's factory image (.factory/tuning-extract/
 * ghawb_para_lut_imx363.bin: 36-byte records (cct, r_resp, b_resp, ...),
 * table at offset 24; the tuning blobs stay local per DECISIONS §7 — only
 * these constants travel). This is the ct_curve a bayes AWB searches:
 * gains at a CT are (1/r, 1, 1/b). */
#define CP_AWB_N 9
static const double CP_AWB_CT[CP_AWB_N] = {
    15000, 10000, 7500, 6500, 5000, 3800, 2800, 2300, 1800,
};
static const double CP_AWB_R[CP_AWB_N] = {
    0.36288, 0.36713, 0.37563, 0.39831, 0.44080,
    0.54731, 0.73272, 0.91381, 1.27599,
};
static const double CP_AWB_B[CP_AWB_N] = {
    3.36998, 1.63423, 0.73643, 0.68913, 0.56942,
    0.46457, 0.37128, 0.30129, 0.13751,
};

/* piecewise-linear response interpolation, clamped at the knot ends */
static inline double cp_awb_curve(const double c[CP_AWB_N], double ct)
{
    if (ct <= CP_AWB_CT[CP_AWB_N - 1]) return c[CP_AWB_N - 1];
    if (ct >= CP_AWB_CT[0]) return c[0];
    for (int i = 0; i < CP_AWB_N - 1; i++) {
        double t0 = CP_AWB_CT[i], t1 = CP_AWB_CT[i + 1];
        if (ct <= t0 && ct >= t1) {
            double k = (ct - t0) / (t1 - t0);
            return c[i] * (1.0 - k) + c[i + 1] * k;
        }
    }
    return c[0];
}

/* AwbStats::computeColourError semantics: squared non-greyness of
 * gains(means) — G-gain is 1, so only R and B carry error. The per-frame
 * normalization cancels in the argmin, so the absolute scale is free. */
static inline double cp_awb_err(const double m[3], double ct)
{
    double r = m[0] / cp_awb_curve(CP_AWB_R, ct) - m[1];
    double b = m[2] / cp_awb_curve(CP_AWB_B, ct) - m[1];
    return r * r + b * b;
}

/* quadratic-extremum refinement, ported from libcamera
 * AwbBayes::interpolateQuadratic: given the three samples around the best
 * point, return the vertex CT (clamped into [a.x, c.x]). */
static inline double cp_awb_quad(double xa, double ya, double xb, double yb,
                                 double xc, double yc)
{
    const double eps = 1e-3;
    double cax = xc - xa, cay = yc - ya, bax = xb - xa, bay = yb - ya;
    double den = 2.0 * (bay * cax - cay * bax);
    if (fabs(den) > eps) {
        double num = bay * cax * cax - cay * bax * bax;
        double r = num / den + xa;
        return xa > r ? xa : (xc < r ? xc : r);
    }
    return ya < yc - eps ? xa : (yc < ya - eps ? xc : xb);
}

/* M47⑤j bayesian AWB search — ported from libcamera awb_bayes.cpp
 * (LGPL-2.1-or-later, Raspberry Pi Ltd 2019 / Ideas On Board 2024),
 * coarseSearch only: "The search works very well without prior
 * likelihoods", and with lux unknown the prior is the constant 1 (its
 * log drops out of the argmin) — so no priors, no transverse fine search.
 * Walk the CT range multiplicatively (t += t/10*kSearchStep, kSearchStep
 * 0.2), evaluate the colour error of the curve gains at each t, keep the
 * best sample, refine quadratically around it.
 *
 * Why this replaces free gray-world gains (device 2026-09-05, warm-lamp +
 * dark-room frame): gray-world equalizes whatever the means happen to be —
 * a green noise floor or a green screen drags the gains off any real
 * illuminant. The curve-CONSTRAINED search can only pick gains a real
 * light would produce for THIS sensor: on-locus scenes recover exactly,
 * off-locus ones land at the nearest plausible CT with bounded gains.
 * means all-zero / mg<=0 -> returns 0 (the caller freezes on it). */
static inline double cp_awb_search(const double m[3])
{
    if (!(m[1] > 0.0))
        return 0.0;
    /* the multiplicative walk makes ~107 samples over [1800, 15000] */
    double ts[160], es[160];
    int n = 0;
    double t = CP_AWB_CT[CP_AWB_N - 1];
    const double hi = CP_AWB_CT[0];
    while (n < 160) {
        ts[n] = t;
        es[n] = cp_awb_err(m, t);
        n++;
        if (t >= hi)
            break;
        t = t + t / 10.0 * 0.2; /* kSearchStep */
        if (t > hi)
            t = hi;
    }
    int best = 0;
    for (int i = 1; i < n; i++)
        if (es[i] < es[best])
            best = i;
    /* refine around the best sample's neighbors (libcamera refines the
     * clamped bestPoint the same way) */
    if (best > 0 && best + 1 < n)
        return cp_awb_quad(ts[best - 1], es[best - 1],
                           ts[best], es[best],
                           ts[best + 1], es[best + 1]);
    return ts[best];
}

/* gains for a CT off the vendor curve: (1/r, 1, 1/b), clamped into the
 * xform table domain [0.25, 4] — the curve itself spans [0.30, 2.76], so
 * the clamps are a guard, not a law. */
static inline void cp_awb_gains(double ct, float wb[3])
{
    double r = 1.0 / cp_awb_curve(CP_AWB_R, ct);
    double b = 1.0 / cp_awb_curve(CP_AWB_B, ct);
    if (!(r >= 0.25)) r = 0.25;
    if (!(b >= 0.25)) b = 0.25;
    if (r > 4.0) r = 4.0;
    if (b > 4.0) b = 4.0;
    wb[0] = (float)r;
    wb[1] = 1.0f;
    wb[2] = (float)b;
}

/* Pick the CCM by colour temperature — libcamera keys ccmAlgo_ on
 * frameContext.awb.colourTemperature; the vendor's own tuning keys each
 * matrix on its CCT bracket (INC [2500,3700], TL84 [3600,4600],
 * D65 [5800,7100]). Knot = each bracket's midpoint; piecewise-linear
 * between knots, clamped outside — the same interpolation structure the
 * gains curve uses. (Supersedes the M47⑤g warmth=wb_b/wb_r heuristic.) */
static inline void cp_ccm_for_ct(double ct, float out[9])
{
    const float *m;
    const float *hi = NULL;
    float k = 0.0f;
    if (ct <= 3100.0) {
        m = CP_CCM_INC;
    } else if (ct < 4100.0) {
        m = CP_CCM_INC; hi = CP_CCM_TL84; k = (float)((ct - 3100.0) / 1000.0);
    } else if (ct < 6450.0) {
        m = CP_CCM_TL84; hi = CP_CCM_D65; k = (float)((ct - 4100.0) / 2350.0);
    } else {
        m = CP_CCM_D65;
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
/* M47⑤j clipped-highlight neutral, generalized from the M47⑥ triad rule:
 * a pixel with TWO OR MORE sites pinned at the sensor clip carries no
 * usable chroma — the pinned pair's true ratio is destroyed (a clipped
 * R+G white with a live B channel is the pink-center device artifact of
 * 2026-09-05: unequal WB gains dye the pinned pair and the hue-preserving
 * desat keeps the dye). Saturation-flag ISPs never let pinned values drive
 * color; CCM mode renders such pixels achromatic — the level survives, the
 * tint dies. Legacy (ccm==NULL) is untouched (bit-exact harness). */
#define CP_NEUTRAL_CLIP 250
/* M47⑤j fringe ramp — the continuous form of the law above. Around a blown
 * source the transition band is green-DEPLETED: lens-coating veiling glare
 * reflects green most and the green sites compress first near saturation,
 * so the halo's raw ratio rides 10-20% off the illuminant locus (device
 * 2026-09-05 inverted: pre-WB R/G 0.75-0.83 vs the lamp's 0.70) and the CCM
 * — built to amplify chroma ~1.4x — blooms it into a magenta ring (annuli
 * r120-320 rendered R/B +27..+33 over G; true post-WB (226,211,223) down to
 * (188,175,187): bright, near-neutral, max/min 1.07-1.18). Law: in CCM mode
 * a post-WB pixel whose max reaches CP_FRINGE_LO AND whose max/min ratio is
 * <= 1.5 (mx*2 <= mn*3 — the fringe signature; genuinely saturated colors
 * sit far past it and are never touched, e.g. a red LED at 3x) blends toward
 * achromatic, ramping to full neutral at CP_FRINGE_HI. Luma is preserved
 * (blend target is the pixel's own mean). */
#define CP_FRINGE_LO 180
#define CP_FRINGE_HI 235

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
    int sat;                              /* M47⑤k saturation, Q8 (256 = off) */
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
    t->sat = 256; /* saturation off unless the caller raises it (M47⑤k) */
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
    int rr, gg, bb;
    int nclip = (R >= CP_NEUTRAL_CLIP) + (G >= CP_NEUTRAL_CLIP) +
                (B >= CP_NEUTRAL_CLIP);
    int neutral = t->shadow && nclip >= 2;
    if (t->shadow) {
        /* M47⑤j fringe ramp (CP_FRINGE_*): blend bright near-neutral pixels
         * toward achromatic BEFORE the matrix — the continuous form of the
         * clipped-neutral law; saturated colors fail the ratio gate.
         * M47⑤s: the ratio test became two-domain. Testing only the post-WB
         * values was circular for self-luminous content — our own CT gains
         * dye an input-neutral halo non-neutral exactly when the CT is off,
         * so it failed its own rescue and the CCM + saturation bloomed it
         * into a saturated ring (magenta-red annulus around any bright
         * screen in a dim room; host repro through this exact chain,
         * 2026-09-06). Both populations are achromatic scene content:
         *   - reflective gray under the scene illuminant: input tinted by
         *     the light, neutral AFTER the gains (the ⑤j device halo,
         *     raw 158/211/86 -> post-WB 226/211/222)
         *   - self-luminous/specular: input already neutral, dyed BY the
         *     gains (an emissive screen halo)
         * Neutral in EITHER domain -> collapse. Genuinely chromatic input
         * (a lamp, an LED) fails both ratios and keeps its color. The
         * brightness trigger stays on the rendered (post-WB) value, where
         * the fringe artifact lives. */
        int mxi = R > G ? (R > B ? R : B) : (G > B ? G : B);
        int mni = R < G ? (R < B ? R : B) : (G < B ? G : B);
        int mxw = r > g ? (r > b ? r : b) : (g > b ? g : b);
        int mnw = r < g ? (r < b ? r : b) : (g < b ? g : b);
        if (mxw >= CP_FRINGE_LO &&
            (mxi * 2 <= mni * 3 || mxw * 2 <= mnw * 3)) {
            int s = ((mxw - CP_FRINGE_LO) * 256) / (CP_FRINGE_HI - CP_FRINGE_LO);
            if (s >= 256) {
                neutral = 1;
            } else if (s > 0) {
                int n = (r + g + b) / 3;
                int keep = 256 - s;
                r = n + (((r - n) * keep + 128) >> 8);
                g = n + (((g - n) * keep + 128) >> 8);
                b = n + (((b - n) * keep + 128) >> 8);
            }
        }
    }
    if (neutral) {
        /* fully neutral (clipped pair, or fringe at full ramp): collapse
         * to achromatic (M47⑤j, see CP_NEUTRAL_CLIP). Equal channels sail
         * through shadow/highlight desat unchanged and gamma renders them
         * white; the unequal WB gains never get to dye the clip. */
        rr = gg = bb = (r + g + b) / 3;
    } else {
        const int16_t *m = t->m;
        rr = (m[0] * r + m[1] * g + m[2] * b + 8192) >> 14;
        gg = (m[3] * r + m[4] * g + m[5] * b + 8192) >> 14;
        bb = (m[6] * r + m[7] * g + m[8] * b + 8192) >> 14;
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
    if (t->sat != 256) {
        /* M47⑤k saturation, in the display domain (the phone-preview "punch"):
         * scale each channel's distance from its own Rec601 luma. Neutral in,
         * neutral out — the M47⑤j color verdicts are untouched; 256 = off is
         * bit-exact with the pre-⑤k chain. */
        int y = (R8 * 77 + G8 * 151 + B8 * 28 + 127) >> 8;
        int rs = y + (((R8 - y) * t->sat + 128) >> 8);
        int gs = y + (((G8 - y) * t->sat + 128) >> 8);
        int bs = y + (((B8 - y) * t->sat + 128) >> 8);
        R8 = (uint8_t)(rs < 0 ? 0 : rs > 255 ? 255 : rs);
        G8 = (uint8_t)(gs < 0 ? 0 : gs > 255 ? 255 : gs);
        B8 = (uint8_t)(bs < 0 ? 0 : bs > 255 ? 255 : bs);
    }
    if (out) {
        out[0] = R8;
        out[1] = G8;
        out[2] = B8;
    }
    if (o565)
        *o565 = (uint16_t)(((R8 & 0xF8) << 8) | ((G8 & 0xFC) << 3) | (B8 >> 3));
}

/* ---- debayer + rotate + scale (single pass) ---- */

/* One-pixel reconstruction: identical math to the M47② original (bilinear
 * in Bayer domain, BGGR sites since 2026-09-05 — see the CFA ORIENTATION
 * receipt at cp_wb_measure; site classification on the UNCLAMPED rounded
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
/* ⑤v: the interpolation half of cp_px alone — LINEAR R,G,B, no color
 * transform. The fold (cp_fold) averages these per output bin and applies
 * cp_apply_xform ONCE per output pixel; cp_px remains the fused per-pixel
 * form (interp + xform). Same math, one source. */
static inline void cp_px_lin(const uint8_t *g, uint32_t w,
                             int xc, int xm, int xp, int xpar,
                             int yc, int ym, int yp, int ypar,
                             int *oR, int *oG, int *oB)
{
    int l = g[(size_t)yc * w + xm], r = g[(size_t)yc * w + xp];
    int u = g[(size_t)ym * w + xc], d = g[(size_t)yp * w + xc];
    int ul = g[(size_t)ym * w + xm], ur = g[(size_t)ym * w + xp];
    int dl = g[(size_t)yp * w + xm], dr = g[(size_t)yp * w + xp];
    int c = g[(size_t)yc * w + xc];
    /* BGGR site classification (see the CFA ORIENTATION receipt above —
     * flipped together with cp_wb_measure 2026-09-05) */
    if (!(ypar & 1)) {          /* B row: even col = B site, odd = G */
        if (!(xpar & 1)) { *oB = c; *oG = (l + r + u + d) / 4; *oR = (ul + ur + dl + dr) / 4; }
        else            { *oG = c; *oB = (l + r) / 2;         *oR = (u + d) / 2; }
    } else {                    /* R row: even col = G, odd = R site */
        if (!(xpar & 1)) { *oG = c; *oR = (l + r) / 2;         *oB = (u + d) / 2; }
        else            { *oR = c; *oG = (l + r + u + d) / 4; *oB = (ul + ur + dl + dr) / 4; }
    }
}

static inline void cp_px(const uint8_t *g, uint32_t w,
                         int xc, int xm, int xp, int xpar,
                         int yc, int ym, int yp, int ypar,
                         const struct cp_xform *t, uint8_t *out, uint16_t *o565)
{
    int R, G, B;
    cp_px_lin(g, w, xc, xm, xp, xpar, yc, ym, yp, ypar, &R, &G, &B);
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

/* ---- M47⑤k preview look: tone / denoise / sharpen (display domain) ----
 *
 * The remaining gap to a normal phone's preview after the M47⑤j color work
 * is not color — it is noise (no temporal/spatial NR), a flat tone (plain
 * gamma 2.2 where phones run an adaptive stretch + saturation + sharpening).
 * All three laws below are ports, each credited in place; they run on the
 * packed display frame the debayer walk produces, so the whole ⑤j color
 * chain (WB -> CCM -> fringe/clip -> LUT) is upstream and untouched. */

/* histogram quantile: the smallest value whose cumulative count reaches
 * q*total (the RPi Histogram::quantile semantics, integer bins) */
static inline int cp_hist_quantile(const uint32_t hist[256], uint64_t total,
                                   double q)
{
    uint64_t want = (uint64_t)(q * (double)total + 0.5);
    uint64_t acc = 0;
    for (int v = 0; v < 256; v++) {
        acc += hist[v];
        if (acc >= want && acc > 0)
            return v;
    }
    return 255;
}

/* M47⑤k tone — ported from libcamera RPi IPA rpi/contrast.cpp (BSD-2-Clause,
 * Copyright (C) 2019 Raspberry Pi Ltd): computeStretchCurve() quantile-
 * stretches the histogram — pull the 1st percentile down to lo_level (by at
 * most lo_max: "if the start of the histogram is rather empty, try to pull
 * it down a bit"), PIN the median ("to limit the apparent amount of global
 * brightness shift"), push the 95th percentile up to hi_level (by at most
 * hi_max), piecewise-linear through the knots — then applyManualContrast()
 * on top ((v - mid) * contrast + mid + brightness). RPi works a 16-bit
 * domain and composes onto the sensor gamma curve; here the histogram is the
 * 8-bit LINEAR gray plane (cp_wb_measure's hist), quantile VALUES map
 * through the gamma LUT (monotone, so quantile order survives), and the
 * output is the composed display LUT out = manual(stretch(gam[v])) — a
 * drop-in replacement for the plain gamma LUT at cp_rot_init time. Neutral
 * stays neutral (one curve for all channels). Defaults are RPi's
 * (0.01, 0.015, 500, 0.95, 0.95, 2000) scaled by 255/65536. */
struct cp_tone {
    uint8_t lut[256];   /* composed manual ∘ stretch ∘ gam */
};

static inline void cp_tone_step(struct cp_tone *t, const uint32_t hist[256],
                                uint64_t total, const uint8_t gam[256],
                                double contrast, double brightness)
{
    if (!hist || !total) { /* no stats: gamma only, no stretch */
        for (int i = 0; i < 256; i++) t->lut[i] = gam[i];
        return;
    }
    /* quantiles in the linear domain, mapped through gam into display
     * values — gamma is monotone, so quantile order is preserved */
    int dlo = gam[cp_hist_quantile(hist, total, 0.01)];
    int dmid = gam[cp_hist_quantile(hist, total, 0.5)];
    int dhi = gam[cp_hist_quantile(hist, total, 0.95)];
    int level_lo = (int)(0.015 * 255.0 + 0.5);        /* 4 */
    int level_hi = (int)(0.95 * 255.0 + 0.5);         /* 242 */
    int mv_lo = (int)(500.0 / 65536.0 * 255.0 + 0.5); /* 2 */
    int mv_hi = (int)(2000.0 / 65536.0 * 255.0 + 0.5);/* 8 */
    /* RPi laws verbatim:
     *   histLo = max(levelLo, min(65535, min(histLo, levelLo + loMax)))
     *   histHi = min(levelHi, max(0, max(histHi, levelHi - hiMax))) */
    int klo = dlo < level_lo + mv_lo ? dlo : level_lo + mv_lo;
    if (klo > 255) klo = 255;
    klo = level_lo > klo ? level_lo : klo;
    int khi = dhi > level_hi - mv_hi ? dhi : level_hi - mv_hi;
    if (khi < 0) khi = 0;
    khi = level_hi < khi ? level_hi : khi;
    /* defensive monotonicity (RPi's Pwl assumes lo<=mid<=hi; a degenerate
     * histogram could fold a knot past the median and bend the LUT) */
    if (klo > dmid) klo = dmid;
    if (khi < dmid) khi = dmid;
    const int ks[5] = { 0, klo, dmid, khi, 255 };
    const int vs[5] = { 0, level_lo, dmid, level_hi, 255 };
    for (int i = 0; i < 256; i++) {
        int d = gam[i];
        int seg = 1;
        while (seg < 4 && d > ks[seg]) seg++;
        int out;
        if (d <= ks[0])
            out = vs[0];
        else if (d >= ks[4])
            out = vs[4];
        else if (ks[seg] == ks[seg - 1])
            out = vs[seg];
        else
            out = vs[seg - 1] + ((d - ks[seg - 1]) * (vs[seg] - vs[seg - 1]) +
                                 (ks[seg] - ks[seg - 1]) / 2) /
                                (ks[seg] - ks[seg - 1]);
        /* applyManualContrast: (y - mid) * contrast + mid + brightness */
        out = (int)((out - 128) * contrast + 128 + brightness + 0.5);
        t->lut[i] = (uint8_t)(out < 0 ? 0 : out > 255 ? 255 : out);
    }
}

/* M47⑤k denoise — hqdn3d, ported from FFmpeg libavfilter/vf_hqdn3d.c
 * (GPL-2.0-or-later, Copyright (c) 2003 Daniel Moreno, 2010 Baptiste
 * Coudurier, 2012 Loren Merritt — itself ported from MPlayer), specialized
 * to 8-bit depth with the planes interleaved (pixel stride 3) so it runs in
 * place on the packed display frame.
 *
 * Law (ffmpeg math verbatim): per plane, a 1-D spatial lowpass runs along
 * each row (pixel_ant within the row, line_ant across rows), and its output
 * feeds a temporal lowpass against the previous FILTERED frame (frame_ant).
 * Both are lowpass(): cur + coef[(prev-cur)>>4], where precalc_coefs builds
 * the correction table as similarity^gamma * 256 * f — near-zero at large
 * diffs (real motion / edges keep their sharpness), substantial at small
 * diffs (noise collapses). Motion-adaptive by construction, no explicit
 * motion search. Strengths are the ffmpeg option semantics (dist25):
 * luma_spatial / chroma_spatial / luma_tmp / chroma_tmp, defaults 4/3/6/4.5.
 * Our planes are R,G,B rather than Y+C: every plane runs the LUMA tables
 * (the chroma tables exist to baby YUV's noise-amplified subsampled chroma;
 * RGB planes are all "luma-like"). */
#define CP_NR_LUT_BITS 4                 /* ffmpeg LUT_BITS at depth 8 */
#define CP_NR_TAB (512 << CP_NR_LUT_BITS)

struct cp_nr {
    uint32_t w, h;
    int16_t *coefs[4];                   /* [0]=on/off flag, [4096+i]=table */
    uint16_t *line[3];                   /* per-plane row lowpass state */
    uint16_t *fprev[3];                  /* previous filtered frame, lazily */
};

/* ffmpeg precalc_coefs(dist25, depth=8) verbatim */
static inline void cp_nr_precalc(double dist25, int16_t *ct)
{
    int i;
    double gamma, simil, C;
    double d25 = dist25 < 252.0 ? dist25 : 252.0;
    gamma = log(0.25) / log(1.0 - d25 / 255.0 - 0.00001);
    for (i = -(256 << CP_NR_LUT_BITS); i < (256 << CP_NR_LUT_BITS); i++) {
        double f = (i * (1 << (9 - CP_NR_LUT_BITS)) +
                    (1 << (8 - CP_NR_LUT_BITS)) - 1) / 512.0;
        simil = 1.0 - fabs(f) / 255.0;
        if (simil < 0.0) simil = 0.0;
        C = pow(simil, gamma) * 256.0 * f;
        ct[(256 << CP_NR_LUT_BITS) + i] = (int16_t)lrint(C);
    }
    ct[0] = dist25 != 0.0;   /* the denoise_depth on/off flag */
}

/* ffmpeg lowpass(): prev/cur are the 16-bit internal values (v<<8 + 127) */
static inline int cp_nr_lowpass(int prev, int cur, const int16_t *coef)
{
    int d = (prev - cur) >> (8 - CP_NR_LUT_BITS);
    return cur + coef[d];
}

/* one plane's temporal lowpass over rows [y0,y1) — ffmpeg denoise_temporal()
 * verbatim. Rows are independent (each reads only its own rgb row and its
 * own frame_ant row), so ANY row partition is bit-exact — the parallel
 * form the viewfinder ships (spatial cannot be banded: line_ant chains
 * rows, so the spatial path stays whole-plane single-threaded). */
static inline void cp_nr_temporal_rows(struct cp_nr *n, int c, uint8_t *rgb,
                                       uint32_t y0, uint32_t y1)
{
    const uint32_t w = n->w;
    const size_t stride = (size_t)w * 3;
    const int16_t *temporal = n->coefs[2] + (256 << CP_NR_LUT_BITS); /* luma_tmp —
                                                  every RGB plane runs the luma tables */
    uint16_t *fprev = n->fprev[c];
    if (!fprev)
        return; /* not primed: nothing to filter against */
    if (y1 > n->h)
        y1 = n->h;
    for (uint32_t y = y0; y < y1; y++) {
        uint8_t *row = rgb + (size_t)y * stride;
        uint16_t *fa = fprev + (size_t)y * w;
        for (uint32_t x = 0; x < w; x++) {
            uint32_t cur = ((uint32_t)row[(size_t)x * 3 + c] << 8) + 127;
            uint32_t tmp = (uint32_t)cp_nr_lowpass(fa[x], (int)cur, temporal);
            fa[x] = (uint16_t)tmp;
            row[(size_t)x * 3 + c] = (uint8_t)(tmp >> 8);
        }
    }
}

static inline int cp_nr_init(struct cp_nr *n, uint32_t w, uint32_t h,
                             double ls, double cs, double lt, double ct)
{
    memset(n, 0, sizeof *n);
    if (!w || !h || w > 8192 || h > 8192)
        return 0;
    n->w = w;
    n->h = h;
    for (int i = 0; i < 4; i++) {
        n->coefs[i] = (int16_t *)malloc(sizeof(int16_t) * CP_NR_TAB);
        if (!n->coefs[i])
            return 0;
        double d[4] = { ls, cs, lt, ct };
        cp_nr_precalc(d[i] > 0.0 ? d[i] : 0.0, n->coefs[i]);
    }
    for (int c = 0; c < 3; c++) {
        n->line[c] = (uint16_t *)calloc(w, sizeof(uint16_t));
        if (!n->line[c])
            return 0;
    }
    return 1;
}

/* spatial+temporal whole-plane pass — ffmpeg denoise_spatial() feeding
 * denoise_temporal() (rows verbatim; LOAD = v<<8 + 127, STORE = >>8), in
 * place on the interleaved frame. Whole-plane ONLY: line_ant chains rows,
 * so this cannot be row-banded — which is why the shipped default turns
 * spatial off and runs the temporal path banded instead. */
static inline void cp_nr_plane(struct cp_nr *n, int c, uint8_t *rgb)
{
    const uint32_t w = n->w, h = n->h;
    const size_t stride = (size_t)w * 3;
    uint16_t *line_ant = n->line[c];
    uint16_t *frame_ant = n->fprev[c];
    const int16_t *spatial = n->coefs[0] + (256 << CP_NR_LUT_BITS);  /* luma_spatial */
    const int16_t *temporal = n->coefs[2] + (256 << CP_NR_LUT_BITS); /* luma_tmp —
                                                  every RGB plane runs the luma tables */
    uint32_t tmp, pixel_ant;
    if (!frame_ant)
        return; /* not primed (OOM): the plane stays unfiltered */
    if (!n->coefs[0][0]) {
        cp_nr_temporal_rows(n, c, rgb, 0, h); /* spatial off */
        return;
    }
    /* first row: no top neighbor — only the left one (ffmpeg verbatim) */
    pixel_ant = ((uint32_t)rgb[c] << 8) + 127;
    for (uint32_t x = 0; x < w; x++) {
        uint32_t cur = ((uint32_t)rgb[(size_t)x * 3 + c] << 8) + 127;
        line_ant[x] = tmp = pixel_ant =
            (uint32_t)cp_nr_lowpass((int)pixel_ant, (int)cur, spatial);
        frame_ant[x] = tmp =
            (uint32_t)cp_nr_lowpass(frame_ant[x], (int)tmp, temporal);
        rgb[(size_t)x * 3 + c] = (uint8_t)(tmp >> 8);
    }
    for (uint32_t y = 1; y < h; y++) {
        uint8_t *row = rgb + (size_t)y * stride;
        uint16_t *fa = frame_ant + (size_t)y * w;
        pixel_ant = ((uint32_t)row[c] << 8) + 127;
        for (uint32_t x = 0; x + 1 < w; x++) {
            line_ant[x] = tmp = (uint32_t)cp_nr_lowpass(
                line_ant[x], (int)pixel_ant, spatial);
            pixel_ant = (uint32_t)cp_nr_lowpass(
                (int)pixel_ant,
                (int)(((uint32_t)row[(size_t)(x + 1) * 3 + c] << 8) + 127),
                spatial);
            fa[x] = tmp = (uint32_t)cp_nr_lowpass(fa[x], (int)tmp, temporal);
            row[(size_t)x * 3 + c] = (uint8_t)(tmp >> 8);
        }
        line_ant[w - 1] = tmp = (uint32_t)cp_nr_lowpass(
            line_ant[w - 1], (int)pixel_ant, spatial);
        fa[w - 1] = tmp =
            (uint32_t)cp_nr_lowpass(fa[w - 1], (int)tmp, temporal);
        row[(size_t)(w - 1) * 3 + c] = (uint8_t)(tmp >> 8);
    }
}

/* allocate + seed any missing frame_ant plane from the current frame —
 * ffmpeg denoise_depth's lazy first-frame path, split out so a banded
 * caller primes once single-threaded before fanning rows out */
static inline void cp_nr_prime(struct cp_nr *n, uint8_t *rgb)
{
    for (int c = 0; c < 3; c++) {
        if (n->fprev[c])
            continue;
        n->fprev[c] =
            (uint16_t *)malloc(sizeof(uint16_t) * (size_t)n->w * n->h);
        if (!n->fprev[c])
            continue; /* OOM: that plane stays unfiltered */
        for (uint32_t y = 0; y < n->h; y++)
            for (uint32_t x = 0; x < n->w; x++)
                n->fprev[c][(size_t)y * n->w + x] =
                    (uint16_t)(((uint32_t)
                        rgb[((size_t)y * n->w + x) * 3 + c] << 8) + 127);
    }
}

/* banded temporal-only pass over rows [y0,y1): the parallel form the
 * viewfinder runs (default strengths leave spatial off). Guards against
 * spatial being on — banded spatial is impossible (line_ant chains rows). */
static inline void cp_nr_rows(struct cp_nr *n, uint8_t *rgb,
                              uint32_t y0, uint32_t y1)
{
    if (n->coefs[0][0])
        return; /* spatial on: the caller must use the whole-plane form */
    for (int c = 0; c < 3; c++)
        cp_nr_temporal_rows(n, c, rgb, y0, y1);
}

/* denoise the packed display frame in place (single-threaded form):
 * primes frame_ant from the current frame on first call, then filters
 * every plane (spatial+temporal, or temporal-only when ls == 0) */
static inline void cp_nr_frame(struct cp_nr *n, uint8_t *rgb)
{
    cp_nr_prime(n, rgb);
    for (int c = 0; c < 3; c++)
        cp_nr_plane(n, c, rgb);
}

static inline void cp_nr_free(struct cp_nr *n)
{
    for (int i = 0; i < 4; i++) {
        free(n->coefs[i]);
        n->coefs[i] = NULL;
    }
    for (int c = 0; c < 3; c++) {
        free(n->line[c]);
        n->line[c] = NULL;
        free(n->fprev[c]);
        n->fprev[c] = NULL;
    }
}

/* M47⑤k sharpen — RPi rpi/sharpen.cpp parameter semantics (threshold /
 * strength / limit; BSD-2-Clause, Raspberry Pi Ltd) over the textbook 3x3
 * unsharp kernel [0,-1,0; -1,5,-1; 0,-1,0]: the correction applied to a
 * pixel is 4*cur - (l+r+u+d) (the kernel output minus the pixel itself —
 * 0 on flat regions by construction). |corrections| at or below
 * `threshold` are dropped (noise guard), the rest scale by `strength` (Q8)
 * and clamp to +-`limit` (halo guard). Out-of-place (src -> dst): every
 * output byte reads only SOURCE neighbors, so a row band [y0,y1) can run
 * on any thread — an in-place band would read rows a neighboring band had
 * already written. Row edges clamp to self (replicate), like RPi's edge
 * handling. src == dst degenerates: only valid single-threaded whole-frame. */
static inline void cp_sharpen(const uint8_t *src, uint8_t *dst, uint32_t w,
                              uint32_t h, uint32_t y0, uint32_t y1,
                              int strength_q8, int threshold, int limit)
{
    if (!strength_q8 || w < 3 || h < 3 || y1 > h)
        return;
    const size_t stride = (size_t)w * 3;
    if (y0 > h) y0 = h;
    for (uint32_t y = y0; y < y1; y++) {
        const uint8_t *cur = src + (size_t)y * stride;
        const uint8_t *up = y ? cur - stride : cur;
        const uint8_t *dn = y + 1 < h ? cur + stride : cur;
        uint8_t *out = dst + (size_t)y * stride;
        for (uint32_t x = 0; x < w; x++)
            for (int c = 0; c < 3; c++) {
                size_t i = (size_t)x * 3 + c;
                int l = x ? cur[i - 3] : cur[i];
                int r = x + 1 < w ? cur[i + 3] : cur[i];
                int u = up[i];
                int d = dn[i];
                int delta = 4 * cur[i] - (l + r + u + d);
                if (delta > threshold || delta < -threshold) {
                    int adj = (delta * strength_q8 + 128) >> 8;
                    if (adj > limit) adj = limit;
                    if (adj < -limit) adj = -limit;
                    int v = cur[i] + adj;
                    out[i] = (uint8_t)(v < 0 ? 0 : v > 255 ? 255 : v);
                } else {
                    out[i] = cur[i];
                }
            }
    }
}

/* ⑤l noise factor: the per-frame noise floor in display units. Shot-noise
 * law — halving the photons and doubling the gain keeps the mean but scales
 * sigma_adu by sqrt(gain) (sigma = gain*sqrt(N), N ∝ 1/gain) — so the AEC
 * ladder (aec_rung: gain x16 + dgain x2 at the darkest rung) maps to
 * nf = sqrt(gain * dgain): 1.0 at the bright rungs, ~5.7 at the bottom.
 * Same role as RPi's cameraMode.noiseFactor (rpi/sharpen.cpp switchMode,
 * "can't be less than one, right?"); it feeds BOTH scalings below. */
static inline double cp_noise_factor(double gain, double dgain)
{
    double nf = sqrt(gain * dgain); /* TOTAL gain, then clamp — the two
                                     * axes can't be clamped separately:
                                     * 4x * 0.25x is still total 1x */
    return nf < 1.0 ? 1.0 : nf;
}

/* ⑤l sharpen parameter scaling — RPi rpi/sharpen.cpp prepare() at
 * userStrength 1 (so sqrt(userStrength)==1): threshold *= modeFactor,
 * strength /= modeFactor, limit /= modeFactor. Threshold scales UP so grain
 * diffs fall under the noise guard; strength and limit scale DOWN ("Binned
 * modes seem to need the sharpening toned down") so a noisy frame is neither
 * over-sharpened nor haloed. */
static inline void cp_sharp_adapt(double nf, int *strength_q8, int *threshold,
                                  int *limit)
{
    if (nf < 1.0) nf = 1.0;
    *threshold = (int)(*threshold * nf + 0.5);
    *strength_q8 = (int)(*strength_q8 / nf + 0.5);
    *limit = (int)(*limit / nf + 0.5);
}

/* ⑤m denoise knee scaling — sdn semantics (rpi/sdn.cpp programs the ISP
 * denoise with noise constant/slope; denoise strength must track the noise
 * floor, noiseSlope defaulting 3.0 "in case no metadata"). The hqdn3d
 * dist25 knee IS the noise-sigma dial: scale it by the same nf and every
 * pixel diff keeps its noise-to-knee RATIO — noise still collapses at
 * gain 16, real edges (>> knee) keep their sharpness. ⑤m moves the knee
 * to the SPATIAL tables: the handheld viewfinder scene moves every frame
 * (hand tremor at 0.7 MP = many px/frame), so temporal diffs ride above
 * the knee and the temporal path no-ops on noise yet ghosts slow motion
 * (2026-09-06 daylight receipt: 重影/一片一片) — spatial is motion-
 * agnostic by construction, which is also why phone preview ISPs run
 * spatial-only denoise. Re-precalcs the two spatial tables in place (pow
 * over 16K entries — a gain-change event, not a per-frame budget item)
 * and zeroes the line state: line_ant chains rows, so it cannot survive
 * a table swap (fresh zeros = the calloc'd first-frame state). */
static inline void cp_nr_adapt(struct cp_nr *n, double ls, double cs)
{
    cp_nr_precalc(ls > 0.0 ? ls : 0.0, n->coefs[0]);
    cp_nr_precalc(cs > 0.0 ? cs : 0.0, n->coefs[1]);
    for (int c = 0; c < 3; c++)
        memset(n->line[c], 0, sizeof(uint16_t) * n->w);
}

/* RGB888 -> RGB565 (the same pack cp_px does inline — used when the display
 * frame goes through a post-pass and must pack AFTER it) */
static inline void cp_pack565(const uint8_t *rgb, size_t npx, uint16_t *out)
{
    for (size_t i = 0; i < npx; i++) {
        const uint8_t *p = rgb + i * 3;
        out[i] = (uint16_t)(((p[0] & 0xF8) << 8) | ((p[1] & 0xFC) << 3) |
                            (p[2] >> 3));
    }
}

/* ---- M47⑤o area-average downscale (full-res 565 -> preview 565+888) ---- */

/* The point-sampled debayer walk above keeps full per-pixel sensor noise in
 * the preview (point sampling never averages — the √area noise cut of a real
 * downscaler never happens, and the 1.5x upscale in term then magnifies the
 * noise 1:1). Every real pipeline (libcamera software ISP, Megapixels, any
 * phone ISP preview path) demosaics at native resolution and downscales with
 * an AREA filter. This is that filter: the classic integer box — each output
 * pixel averages every source pixel in its rectangle — the INTER_AREA
 * structure. Source samples are expanded to 8-bit BEFORE averaging (5-bit
 * means would quantize the mean itself). Ratios we ship are ~1.3, so each
 * box is 1-4 samples. */

struct cp_box {
    uint32_t *xs;               /* ow*2: [x0,x1) source span per column */
    uint32_t fw, fh;            /* full-res source dims */
    uint32_t ow, oh;            /* preview dims (must not exceed source) */
};

/* returns 0 on bad dims or OOM */
static inline int cp_box_init(struct cp_box *B, uint32_t fw, uint32_t fh,
                              uint32_t ow, uint32_t oh)
{
    memset(B, 0, sizeof *B);
    if (!fw || !fh || !ow || !oh || ow > fw || oh > fh)
        return 0;
    B->fw = fw;
    B->fh = fh;
    B->ow = ow;
    B->oh = oh;
    B->xs = (uint32_t *)malloc((size_t)ow * 2 * sizeof(uint32_t));
    if (!B->xs)
        return 0;
    for (uint32_t o = 0; o < ow; o++) {
        B->xs[o] = (uint32_t)((uint64_t)o * fw / ow);
        B->xs[ow + o] = (uint32_t)(((uint64_t)(o + 1) * fw + ow - 1) / ow);
    }
    return 1;
}

static inline void cp_box_free(struct cp_box *B)
{
    free(B->xs);
    B->xs = NULL;
}

/* rows [y0,y1) of the OUTPUT. Reads src5 read-only, writes only its own
 * output rows — disjoint ranges are bit-exact with the whole walk (same
 * contract as cp_rot_rows). out565/out888 independently optional. */
static inline void cp_box_rows(const struct cp_box *B, const uint16_t *src5,
                               uint32_t y0, uint32_t y1,
                               uint16_t *out565, uint8_t *out888)
{
    const uint32_t fw = B->fw, ow = B->ow;
    for (uint32_t oy = y0; oy < y1; oy++) {
        uint32_t sy0 = (uint32_t)((uint64_t)oy * B->fh / B->oh);
        uint32_t sy1 =
            (uint32_t)(((uint64_t)(oy + 1) * B->fh + B->oh - 1) / B->oh);
        uint16_t *r5 = out565 ? out565 + (size_t)oy * ow : NULL;
        uint8_t *r8 = out888 ? out888 + (size_t)oy * ow * 3 : NULL;
        for (uint32_t ox = 0; ox < ow; ox++) {
            uint32_t x0 = B->xs[ox], x1 = B->xs[ow + ox];
            uint32_t sr = 0, sg = 0, sb = 0;
            for (uint32_t sy = sy0; sy < sy1; sy++) {
                const uint16_t *p = src5 + (size_t)sy * fw;
                for (uint32_t sx = x0; sx < x1; sx++) {
                    uint16_t v = p[sx];
                    uint32_t rv = (v >> 11) & 0x1F;
                    uint32_t gv = (v >> 5) & 0x3F;
                    uint32_t bv = v & 0x1F;
                    sr += (rv << 3) | (rv >> 2);
                    sg += (gv << 2) | (gv >> 4);
                    sb += (bv << 3) | (bv >> 2);
                }
            }
            uint32_t cnt = (x1 - x0) * (sy1 - sy0);
            uint32_t r = (sr + cnt / 2) / cnt;
            uint32_t g = (sg + cnt / 2) / cnt;
            uint32_t b = (sb + cnt / 2) / cnt;
            if (r5)
                r5[ox] = (uint16_t)(((r & 0xF8) << 8) | ((g & 0xFC) << 3) |
                                    (b >> 3));
            if (r8) {
                r8[ox * 3] = (uint8_t)r;
                r8[ox * 3 + 1] = (uint8_t)g;
                r8[ox * 3 + 2] = (uint8_t)b;
            }
        }
    }
}

/* ---- ⑤v linear-domain fold: debayer + rotate + area-average, one pass ----
 *
 * The ⑤o chain this replaces ran TWO full passes over the frame:
 *   cp_rot (interp + FULL xform + 565 pack) at fw x fh  -> plane5f (3.75 MB
 *   DDR round-trip on the shipped geometry) -> cp_box (565 unpack + mean +
 * repack) -> preview. The ⑤u device receipt: debayer+box ~56 ms/frame,
 * fps 26 -> 10.9 — the price of the grain fix.
 *
 * The fold does the same sampling with the COLOR TRANSFORM MOVED AFTER the
 * average: reconstruct LINEAR RGB per source site (cp_px_lin — identical
 * interpolation, xform deferred), accumulate exact integer sums per output
 * bin, integer mean (the cp_box rounding law), then ONE cp_apply_xform per
 * OUTPUT pixel. What dies: the 7.5 MB plane write+read, and 1.9M full-res
 * xform+pack applications (only ow*oh remain).
 *
 * Domain order (average BEFORE gamma, not after) is the same order every
 * real ISP and sensor-side binning uses — averaging display-domain samples
 * after a sqrt-shaped LUT is the aberrant order, and it also double-
 * quantizes (565 pack/unpack sat inside every mean). By design the fold is
 * NOT bit-exact with ⑤o: less quantization noise, one less 5-bit rounding.
 * The bin geometry IS cp_box's exactly (floor start / ceil end, shared
 * boundary pixels double-count into both bins — same span law, same
 * (s + cnt/2)/cnt mean), so the √area noise cut is identical.
 *
 * Threading: same contract as cp_rot_rows/cp_box_rows — rows [y0,y1) of the
 * output, disjoint ranges bit-exact with the whole walk (each output row's
 * bins are private; no cross-row state). One malloc per rows-call (the
 * rotated branch's staging strip, direct-walk fallback on OOM — the
 * cp_rot_rows law); rot 0 walks source-sequential with no allocation.
 * Overflow: sums are u32; per-channel sample <= 255 and cnt = xcnt*yspan
 * stays tiny at every ratio the parser can reach (<= ~121 at a 10x
 * downscale) — 255*121*... is six orders below 2^32. */
struct cp_fold {
    struct cp_rot R;        /* scale-1 rotation stage at fw x fh; owns xf */
    uint32_t fw, fh;        /* rotated full-res dims (== R.ow, R.oh) */
    uint32_t ow, oh;        /* preview dims */
    uint32_t *xs, *xse;     /* ow: bin span per output column (cp_box law) */
    uint32_t *xc;           /* ow: per-bin column count (xse - xs) */
    uint32_t *ys, *yse;     /* oh: bin span per output row */
};

/* returns 0 on bad dims or OOM (caller must not touch the outputs) */
static inline int cp_fold_init(struct cp_fold *F, uint32_t w, uint32_t h,
                               const float wb[3], const float ccm[9],
                               const uint8_t lut[256],
                               int rot, uint32_t ow, uint32_t oh)
{
    memset(F, 0, sizeof *F);
    uint32_t fw = (rot == 90 || rot == 270) ? h : w;
    uint32_t fh = (rot == 90 || rot == 270) ? w : h;
    if (!w || !h || !ow || !oh || ow > fw || oh > fh ||
        ow > 8192 || oh > 8192)
        return 0;
    /* scale-1 rotation stage: at fw x fh the centroid maps are exact
     * integer maps, so every fold sample IS the two-stage path's full-res
     * pixel (pre-xform) — the sampling is pinned, only the average moves. */
    if (!cp_rot_init(&F->R, w, h, wb, ccm, lut, rot, fw, fh))
        return 0;
    F->fw = fw;
    F->fh = fh;
    F->ow = ow;
    F->oh = oh;
    /* xs/xse/xc/ys/yse live in ONE block (mallocng churn law) */
    uint32_t *tab =
        (uint32_t *)malloc((size_t)(3 * ow + 2 * oh) * sizeof(uint32_t));
    if (!tab) {
        cp_rot_free(&F->R);
        return 0;
    }
    F->xs = tab;
    F->xse = tab + ow;
    F->xc = tab + 2 * ow;
    F->ys = tab + 3 * ow;
    F->yse = tab + 3 * ow + oh;
    for (uint32_t o = 0; o < ow; o++) {
        F->xs[o] = (uint32_t)((uint64_t)o * fw / ow);
        F->xse[o] = (uint32_t)(((uint64_t)(o + 1) * fw + ow - 1) / ow);
        F->xc[o] = F->xse[o] - F->xs[o];
    }
    for (uint32_t o = 0; o < oh; o++) {
        F->ys[o] = (uint32_t)((uint64_t)o * fh / oh);
        F->yse[o] = (uint32_t)(((uint64_t)(o + 1) * fh + oh - 1) / oh);
    }
    return 1;
}

static inline void cp_fold_free(struct cp_fold *F)
{
    cp_rot_free(&F->R);
    free(F->xs); /* one block: xs|xse|xc|ys|yse */
    F->xs = F->xse = F->xc = F->ys = F->yse = NULL;
}

/* rows [y0,y1) of the output; reads F + g only, writes only its own rows.
 * The per-bin accumulator is a plain per-output-column scalar (spans are
 * 1-4 samples at shipped ratios) — no frame-sized sums plane, no locks. */
static inline void cp_fold_rows(const struct cp_fold *F, const uint8_t *g,
                                uint32_t y0, uint32_t y1,
                                uint8_t *out, uint16_t *out565)
{
    if (!out && !out565)
        return;
    if (y0 > F->oh) y0 = F->oh;
    if (y1 > F->oh) y1 = F->oh;
    const uint32_t ow = F->ow;
    /* R's column map is sized fw (the scale-1 stage's output width) */
    const int32_t *cv = F->R.cmap, *cm = F->R.cmap + F->fw,
                  *cp = F->R.cmap + 2 * F->fw, *cq = F->R.cmap + 3 * F->fw;

    if (F->R.rot == 0) {
        /* row-sequential in the source: per output row the whole bin sweep
         * touches ys1-ys0 source rows (~2, ~2 KB) — L1-resident, no staging */
        for (uint32_t oy = y0; oy < y1; oy++) {
            uint32_t ys0 = F->ys[oy], ys1 = F->yse[oy];
            uint8_t *orow = out ? out + (size_t)oy * ow * 3 : NULL;
            uint16_t *p5row = out565 ? out565 + (size_t)oy * ow : NULL;
            for (uint32_t ox = 0; ox < ow; ox++) {
                uint32_t xs0 = F->xs[ox], xs1 = F->xse[ox];
                uint32_t sr = 0, sg = 0, sb = 0;
                for (uint32_t ry = ys0; ry < ys1; ry++) {
                    int a, b, c, q;
                    cp_rot_yscalar(&F->R, ry, &a, &b, &c, &q);
                    for (uint32_t rx = xs0; rx < xs1; rx++) {
                        int Rv, Gv, Bv;
                        cp_px_lin(g, F->R.w, cv[rx], cm[rx], cp[rx], cq[rx],
                                  a, b, c, q, &Rv, &Gv, &Bv);
                        sr += (uint32_t)Rv;
                        sg += (uint32_t)Gv;
                        sb += (uint32_t)Bv;
                    }
                }
                uint32_t cnt = (xs1 - xs0) * (ys1 - ys0);
                cp_apply_xform(&F->R.xf,
                               (int)((sr + cnt / 2) / cnt),
                               (int)((sg + cnt / 2) / cnt),
                               (int)((sb + cnt / 2) / cnt),
                               orow ? orow + (size_t)ox * 3 : NULL,
                               p5row ? &p5row[ox] : NULL);
            }
        }
        return;
    }

    /* rotated: the 9-load pattern strides w bytes in the source (the ⑤i
     * memory-latency receipt) — stage a narrow source-column strip per
     * group of output rows, exactly the cp_rot_rows blocking law. Within a
     * group, the row scalar's cl over [ys[gy], yse[ge-1]) spans a few
     * source columns; the column map sweeps every source row, so the strip
     * is R->h rows x (span+2) bytes (~8 KB — L1). */
    uint32_t step = (F->fh + F->oh - 1) / F->oh;
    uint32_t cap = CP_ROT_GROUP * step + 4;
    if (cap > F->R.w) cap = F->R.w;
    uint8_t *stage = (uint8_t *)malloc((size_t)F->R.h * cap);
    for (uint32_t gy = y0; gy < y1; gy += CP_ROT_GROUP) {
        uint32_t ge = gy + CP_ROT_GROUP;
        if (ge > y1) ge = y1;
        int a0, b0, c0, q0, a1, b1, c1, q1;
        cp_rot_yscalar(&F->R, F->ys[gy], &a0, &b0, &c0, &q0);
        cp_rot_yscalar(&F->R, F->yse[ge - 1] - 1, &a1, &b1, &c1, &q1);
        int lo = a0 < a1 ? a0 : a1, hi = a0 > a1 ? a0 : a1;
        int x0 = lo > 0 ? lo - 1 : 0;
        int x1 = hi < (int)F->R.w - 1 ? hi + 1 : (int)F->R.w - 1;
        uint32_t sw = (uint32_t)(x1 - x0 + 1);
        const uint8_t *base = g;
        uint32_t stride = F->R.w;
        int off = 0;
        if (stage && sw <= cap) {
            for (uint32_t r = 0; r < F->R.h; r++)
                memcpy(stage + (size_t)r * sw,
                       g + (size_t)r * F->R.w + x0, sw);
            base = stage;
            stride = sw;
            off = x0;
        }
        for (uint32_t oy = gy; oy < ge; oy++) {
            uint32_t ys0 = F->ys[oy], ys1 = F->yse[oy];
            uint8_t *orow = out ? out + (size_t)oy * ow * 3 : NULL;
            uint16_t *p5row = out565 ? out565 + (size_t)oy * ow : NULL;
            for (uint32_t ox = 0; ox < ow; ox++) {
                uint32_t xs0 = F->xs[ox], xs1 = F->xse[ox];
                uint32_t sr = 0, sg = 0, sb = 0;
                for (uint32_t ry = ys0; ry < ys1; ry++) {
                    int a, b, c, q;
                    cp_rot_yscalar(&F->R, ry, &a, &b, &c, &q);
                    for (uint32_t rx = xs0; rx < xs1; rx++) {
                        int Rv, Gv, Bv;
                        cp_px_lin(base, stride, a - off, b - off, c - off, q,
                                  cv[rx], cm[rx], cp[rx], cq[rx],
                                  &Rv, &Gv, &Bv);
                        sr += (uint32_t)Rv;
                        sg += (uint32_t)Gv;
                        sb += (uint32_t)Bv;
                    }
                }
                uint32_t cnt = (xs1 - xs0) * (ys1 - ys0);
                cp_apply_xform(&F->R.xf,
                               (int)((sr + cnt / 2) / cnt),
                               (int)((sg + cnt / 2) / cnt),
                               (int)((sb + cnt / 2) / cnt),
                               orow ? orow + (size_t)ox * 3 : NULL,
                               p5row ? &p5row[ox] : NULL);
            }
        }
    }
    free(stage);
}

#endif /* CAMPIX_H */
