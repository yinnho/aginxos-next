/* campix_test.c — host tests for campix.h (M47②).
 *
 * Runs on the host (zig cc -O1, wired into scripts/check.sh): the whole
 * point of campix.h is that the geometry/LUT math is verifiable without a
 * device. Every test builds a synthetic RAW10/gray image whose expected
 * output is hand-computed, not derived by running the code under test.
 *
 * The REAL-dims case (2016x1136 sensor, 1456:1080 sensor-domain crop ->
 * rot90 -> 1080:1456-ish portrait) is asserted here so the viewfinder
 * geometry can't silently drift from the term face's fixed frame.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include "campix.h"

static int fails = 0;

#define CHECK(cond, msg) do { \
    if (!(cond)) { printf("FAIL: %s\n", msg); fails++; } \
} while (0)

#define CHECK_EQ_I(a, b, msg) do { \
    long _a = (long)(a), _b = (long)(b); \
    if (_a != _b) { printf("FAIL: %s (%ld != %ld)\n", msg, _a, _b); fails++; } \
} while (0)

/* --- LUTs --- */
static void test_luts(void)
{
    uint8_t lin[256], gam[256], comp[256];
    cp_lut_linear(lin, 16);
    CHECK_EQ_I(lin[0], 0, "linear lut[0]");
    CHECK_EQ_I(lin[16], 0, "linear lut[bl]");
    CHECK_EQ_I(lin[17], 1, "linear lut[bl+1]");
    CHECK_EQ_I(lin[255], 255, "linear lut[255]");
    CHECK_EQ_I(lin[80], 68, "linear lut[80] bl=16"); /* 64*255/239 */

    cp_lut_gamma(gam, 1.0);
    CHECK(gam[137] == 137, "gamma 1.0 is identity");
    cp_lut_gamma(gam, 0.0);
    CHECK(gam[42] == 42, "gamma 0 (off) is identity");

    cp_lut_gamma(gam, 2.2);
    CHECK_EQ_I(gam[0], 0, "gamma lut[0]");
    CHECK_EQ_I(gam[255], 255, "gamma lut[255]");
    int mono = 1;
    for (int v = 1; v < 256; v++) if (gam[v] < gam[v-1]) mono = 0;
    CHECK(mono, "gamma monotonic");
    /* 255*(128/255)^(1/2.2) = 186.4 — allow +-1 across libm */
    CHECK(abs((int)gam[128] - 186) <= 1, "gamma lut[128] ~186");
    /* mid-dark lift is the whole point: linear 32 -> display 99 */
    CHECK(abs((int)gam[32] - 99) <= 1, "gamma lut[32] ~99");

    cp_lut_compose(lin, gam, comp);
    CHECK_EQ_I(comp[16], 0, "compose at black");
    CHECK_EQ_I(comp[255], 255, "compose at white");
    CHECK_EQ_I(comp[80], gam[lin[80]], "compose follows both");
}

/* --- RAW10 extraction (5 B / 4 px, byte i of the group = px i bits[9:2]) --- */
static void test_extract(void)
{
    /* 8x1: two groups, 5th byte (low bits) ignored by the gray path */
    static const uint8_t row[10] = {10, 20, 30, 40, 0xFF, 50, 60, 70, 80, 0xFF};
    uint8_t out[8];
    uint8_t id[256];
    for (int v = 0; v < 256; v++) id[v] = (uint8_t)v;

    cp_raw10_gray_full(row, 8, 1, 10, id, out);
    CHECK(0 == memcmp(out, (uint8_t[8]){10, 20, 30, 40, 50, 60, 70, 80}, 8),
          "raw10_full identity");

    uint8_t lin[256];
    cp_lut_linear(lin, 16);
    /* (v-16)*255/239: 20->4, 30->14, 40->25, 50->36, 60->46, 70->57, 80->68 */
    cp_raw10_gray_full(row, 8, 1, 10, lin, out);
    CHECK(0 == memcmp(out, (uint8_t[8]){0, 4, 14, 25, 36, 46, 57, 68}, 8),
          "raw10_full through linear lut");

    /* crop at group-aligned x0 */
    cp_raw10_gray(row, 10, 4, 0, 4, 1, id, out);
    CHECK(0 == memcmp(out, (uint8_t[4]){50, 60, 70, 80}, 4), "crop x0 group-aligned");
    /* crop at odd x0 (sub-group byte reads) */
    cp_raw10_gray(row, 10, 1, 0, 4, 1, id, out);
    CHECK(0 == memcmp(out, (uint8_t[4]){20, 30, 40, 50}, 4), "crop x0 sub-group");
    /* y0 offset: same row twice, pick row 1 */
    uint8_t two[20];
    memcpy(two, row, 10); memcpy(two + 10, row, 10);
    uint8_t o2[8];
    cp_raw10_gray(two, 10, 0, 1, 8, 1, id, o2);
    CHECK(0 == memcmp(o2, (uint8_t[8]){10, 20, 30, 40, 50, 60, 70, 80}, 8),
          "crop y0=1 picks row 1");
}

/* --- quad means + yavg (M47⑤j: means out, floor gating, emissive 0.5) --- */
static void test_wb(void)
{
    double m[3];
    double y;
    uint8_t g[8 * 4];
    /* neutral uniform */
    memset(g, 128, sizeof g);
    cp_wb_measure(g, 8, 4, m, &y, NULL);
    CHECK(fabs(m[0] - 128.0) < 0.01 && fabs(m[1] - 128.0) < 0.01 &&
          fabs(m[2] - 128.0) < 0.01, "neutral means 128");
    CHECK(fabs(y - 128.0) < 0.01, "neutral yavg 128");
    /* black frame: no stats (freeze signal), yavg 0 */
    memset(g, 0, sizeof g);
    cp_wb_measure(g, 8, 4, m, &y, NULL);
    CHECK(m[0] == 0.0 && m[1] == 0.0 && m[2] == 0.0, "black means 0");
    CHECK(y == 0.0, "black yavg 0");

    /* gray-gain law on the means (legacy path): red-tinted quads
     * (R sites 200 at odd/odd — BGGR, G/B 100 -> means (200,100,100))
     * -> gains (1,2,2) */
    float wb[3];
    memset(g, 100, sizeof g);
    for (uint32_t yy = 1; yy < 4; yy += 2)
        for (uint32_t xx = 1; xx < 8; xx += 2)
            g[yy * 8 + xx] = 200;
    cp_wb_measure(g, 8, 4, m, &y, NULL);
    CHECK(fabs(m[0] - 200.0) < 0.01 && fabs(m[1] - 100.0) < 0.01 &&
          fabs(m[2] - 100.0) < 0.01, "tinted means (200,100,100)");
    /* 0.299*200 + 0.587*100 + 0.114*100 = 129.9 */
    CHECK(fabs(y - 129.9) < 0.05, "tinted yavg 129.9");
    cp_wb_gains_gray(m, wb);
    CHECK(fabsf(wb[0] - 1.0f) < 1e-4, "gray gains r 1");
    CHECK(fabsf(wb[1] - 2.0f) < 1e-4, "gray gains g 2");
    CHECK(fabsf(wb[2] - 2.0f) < 1e-4, "gray gains b 2");
    /* extreme tint caps at 4x */
    cp_wb_gains_gray((const double[3]){250.0, 10.0, 10.0}, wb);
    CHECK(fabsf(wb[0] - 1.0f) < 1e-4, "extreme gray r 1");
    CHECK(fabsf(wb[1] - 4.0f) < 1e-4, "extreme gray g capped 4");
    CHECK(fabsf(wb[2] - 4.0f) < 1e-4, "extreme gray b capped 4");
    cp_wb_gains_gray((const double[3]){0.0, 0.0, 0.0}, wb);
    CHECK(wb[0] == 1.0f && wb[1] == 1.0f && wb[2] == 1.0f, "black gray gains 1");

    /* M47⑤h emissive soft-weight: top quad-row a clipped "screen" (all
     * 250), bottom the lit pattern B 80 / G 108 / R 53 — the white-cable
     * sensor ratios from the device (2026-09-05; the 2026-09-05 chart test
     * flipped the site labels, so the site that MEASURED "R 80" is
     * physically B — labels corrected with the flip). Emissive quads carry
     * 0.5 weight, so the means land BETWEEN the lit point and the all-in
     * gray point: mr (from the 53 sites) 184.3 / mg 202.7 / mb (80 sites)
     * 193.3. yavg stays whole-frame (mr 151.5 / mg 179 / mb 165 -> 169.2). */
    for (uint32_t yy = 0; yy < 4; yy++)
        for (uint32_t xx = 0; xx < 8; xx++) {
            int site = !(yy & 1) ? (!(xx & 1) ? 0 : 1) : (!(xx & 1) ? 1 : 2);
            g[yy * 8 + xx] = (uint8_t)(yy < 2 ? 250
                : site == 0 ? 80 : site == 2 ? 53 : 108);
        }
    cp_wb_measure(g, 8, 4, m, &y, NULL);
    CHECK(fabs(m[0] - 184.3) < 0.1, "emissive soft-weight mr 184.3");
    CHECK(fabs(m[1] - 202.7) < 0.1, "emissive soft-weight mg 202.7");
    CHECK(fabs(m[2] - 193.3) < 0.1, "emissive soft-weight mb 193.3");
    CHECK(fabs(y - 169.2) < 0.1, "yavg stays whole-frame");
    /* all-emissive frame (white wall): neutral white -> means equal */
    memset(g, 250, sizeof g);
    cp_wb_measure(g, 8, 4, m, &y, NULL);
    CHECK(fabs(m[0] - 250.0) < 0.01 && fabs(m[1] - 250.0) < 0.01 &&
          fabs(m[2] - 250.0) < 0.01, "all-emissive stays neutral");
    CHECK(fabs(y - 250.0) < 0.01, "all-emissive yavg 250");

    /* M47⑤j floor gating: a green noise floor (all sites 3) carries no
     * illuminant info (RPi min_g) — excluded from the means but counted
     * in yavg. Top quad-row floor, bottom uniform 100. */
    for (uint32_t yy = 0; yy < 2; yy++)
        memset(g + yy * 8, 3, 8);
    for (uint32_t yy = 2; yy < 4; yy++)
        memset(g + yy * 8, 100, 8);
    cp_wb_measure(g, 8, 4, m, &y, NULL);
    CHECK(fabs(m[0] - 100.0) < 0.01 && fabs(m[1] - 100.0) < 0.01 &&
          fabs(m[2] - 100.0) < 0.01, "floor gated out of means");
    /* yavg over all 8 quads: means (51.5, 51.5, 51.5) -> 51.5 */
    CHECK(fabs(y - 51.5) < 0.1, "yavg keeps the floor");
    /* all-floor frame: means 0 (freeze), yavg alive */
    memset(g, 3, sizeof g);
    cp_wb_measure(g, 8, 4, m, &y, NULL);
    CHECK(m[0] == 0.0 && m[1] == 0.0 && m[2] == 0.0, "all-floor means 0");
    CHECK(fabs(y - 3.0) < 0.01, "all-floor yavg 3");

    CHECK(fabs(cp_yavg((uint8_t[4]){10, 10, 10, 10}, 4) - 10.0) < 0.01,
          "cp_yavg flat");
    CHECK(cp_yavg(g, 0) == 0.0, "cp_yavg empty");
}

/* --- debayer / rotate / scale --- */

/* 8x8 with per-site constants: B(even,even)=160, G=128, R(odd,odd)=100
 * (BGGR — see the CFA ORIENTATION receipt in campix.h). Every interior
 * reconstruction is exactly (100,128,160) = (R,G,B) regardless of site —
 * the site scheme cancels out. */
static void site_const(uint8_t *g, uint32_t w, uint32_t h)
{
    for (uint32_t y = 0; y < h; y++)
        for (uint32_t x = 0; x < w; x++) {
            int site = !(y & 1) ? (!(x & 1) ? 0 : 1) : (!(x & 1) ? 1 : 2);
            g[y * w + x] = (uint8_t)(site == 0 ? 160 : site == 2 ? 100 : 128);
        }
}

static void test_debayer(void)
{
    uint8_t id[256];
    for (int v = 0; v < 256; v++) id[v] = (uint8_t)v;
    const float w1[3] = {1.0f, 1.0f, 1.0f};

    uint8_t g[8 * 8];
    site_const(g, 8, 8);

    /* rot 0 identity scale: interior pixels reconstruct exactly; the inline
     * RGB565 plane must pack from the SAME reconstructed bytes */
    uint8_t out[8 * 8 * 3];
    uint16_t five[8 * 8];
    cp_debayer_rot(g, 8, 8, w1, NULL, id, 0, 8, 8, out, five);
    int ok = 1;
    for (uint32_t y = 1; y < 7; y++)
        for (uint32_t x = 1; x < 7; x++) {
            const uint8_t *p = out + (y * 8 + x) * 3;
            if (p[0] != 100 || p[1] != 128 || p[2] != 160) ok = 0;
            uint16_t want = (uint16_t)(((p[0] & 0xF8) << 8) | ((p[1] & 0xFC) << 3) | (p[2] >> 3));
            if (five[y * 8 + x] != want) ok = 0;
        }
    CHECK(ok, "debayer rot0 interior (100,128,160) + 565 pack");
    /* display-only pass (out==NULL): the 565 plane must match the
     * dual-plane run byte for byte */
    uint16_t five2[8 * 8];
    cp_debayer_rot(g, 8, 8, w1, NULL, id, 0, 8, 8, NULL, five2);
    ok = 1;
    for (uint32_t y = 1; y < 7; y++)
        for (uint32_t x = 1; x < 7; x++)
            if (five2[y * 8 + x] != five[y * 8 + x]) ok = 0;
    CHECK(ok, "display-only 565 matches dual-plane");

    /* half scale, rot 0: output px -> src (2ox+1, 2oy+1), all R sites
     * (BGGR: odd/odd) */
    uint8_t half[4 * 4 * 3];
    cp_debayer_rot(g, 8, 8, w1, NULL, id, 0, 4, 4, half, NULL);
    ok = 1;
    for (uint32_t y = 0; y < 3; y++)
        for (uint32_t x = 0; x < 3; x++) {
            const uint8_t *p = half + (y * 4 + x) * 3;
            if (p[0] != 100 || p[1] != 128 || p[2] != 160) ok = 0;
        }
    CHECK(ok, "debayer rot0 half-scale interior");

    /* rotation mapping, 8x8 gradient g[x][y] = x*16+y. Even/even output
     * positions land on G sites under rotation, whose R comes from a
     * neighbor PAIR — the gradient is separable (linear in x and y), so
     * whichever pair the BGGR layout picks averages to the same exact
     * value:
     *   rot90 CW:  out(ox,oy) samples src(oy, 7-ox); oy even, 7-ox odd ->
     *              G site, R = (l+r)/2 = oy*16+(7-ox)  [RGGB said (u+d)/2 —
     *              same value by separability; only the pair swapped]
     *   rot270 CCW: out(ox,oy) samples src(7-oy, ox); 7-oy odd, ox even ->
     *              G site, R = (u+d)/2 = (7-oy)*16+ox   [RGGB said (l+r)/2]
     * Positions with ox,oy in {2,4,6} keep both pairs' members in bounds. */
    uint8_t q[64];
    for (uint32_t y = 0; y < 8; y++)
        for (uint32_t x = 0; x < 8; x++)
            q[y * 8 + x] = (uint8_t)(x * 16 + y);
    uint8_t rot[8 * 8 * 3];
    cp_debayer_rot(q, 8, 8, w1, NULL, id, 90, 8, 8, rot, NULL);
    ok = 1;
    for (uint32_t oy = 2; oy <= 6; oy += 2)
        for (uint32_t ox = 2; ox <= 6; ox += 2) {
            const uint8_t *p = rot + (oy * 8 + ox) * 3;
            if (p[0] != (uint8_t)(oy * 16 + (7 - ox))) ok = 0;
        }
    CHECK(ok, "rot90 CW maps out(ox,oy) to src(oy, 7-ox)");
    cp_debayer_rot(q, 8, 8, w1, NULL, id, 270, 8, 8, rot, NULL);
    ok = 1;
    for (uint32_t oy = 2; oy <= 6; oy += 2)
        for (uint32_t ox = 2; ox <= 6; ox += 2) {
            const uint8_t *p = rot + (oy * 8 + ox) * 3;
            if (p[0] != (uint8_t)((7 - oy) * 16 + ox)) ok = 0;
        }
    CHECK(ok, "rot270 CCW maps out(ox,oy) to src(7-oy, ox)");

    /* rotated dims: portrait out of a landscape slice */
    uint8_t pr[4 * 8 * 3]; /* rot of 8x4 -> 4x8 */
    uint8_t wide[8 * 4];
    memset(wide, 0, sizeof wide);
    cp_debayer_rot(wide, 8, 4, w1, NULL, id, 90, 4, 8, pr, NULL);
    CHECK(pr[0] == 0 && pr[(8 * 4 - 1) * 3] == 0, "rot dims no crash");

    /* WB gains multiply before the lut, saturating at 255 */
    uint8_t sat[8 * 8 * 3];
    const float w2[3] = {1.0f, 2.0f, 4.0f};
    cp_debayer_rot(g, 8, 8, w2, NULL, id, 0, 8, 8, sat, NULL);
    /* interior: G=128*2=256 -> 255; B=160*4=640 -> 255 */
    const uint8_t *p = sat + ((3 * 8 + 3) * 3);
    CHECK_EQ_I(p[0], 100, "wb+sat R");
    CHECK_EQ_I(p[1], 255, "wb+sat G clamps");
    CHECK_EQ_I(p[2], 255, "wb+sat B clamps");

    /* gamma LUT rides the tail: 100 -> ~167 at 2.2 (0.392^0.4545) */
    uint8_t gam[256];
    cp_lut_gamma(gam, 2.2);
    cp_debayer_rot(g, 8, 8, w1, NULL, gam, 0, 8, 8, out, NULL);
    p = out + ((3 * 8 + 3) * 3);
    CHECK(abs((int)p[0] - 167) <= 1, "gamma on R channel");
}

/* --- crop geometry (REAL viewfinder dims) --- */
static void test_crop(void)
{
    uint32_t x0, y0, cw, ch;
    /* 2016x1136 sensor, viewfinder 1080:1456 portrait OUT -> sensor-domain
     * aspect 1456:1080 -> 1530x1136 centered at x0=242, then rot90 ->
     * 1136x1530 (0.7425 vs 1080/1456 = 0.7418). cw/x0 MUST be even: an odd
     * x0 flips Bayer phase -> green tint (device 2026-09-05). */
    cp_crop_for_aspect(2016, 1136, 1456, 1080, &x0, &y0, &cw, &ch);
    CHECK_EQ_I(cw, 1530, "vf crop width");
    CHECK_EQ_I(ch, 1136, "vf crop height");
    CHECK_EQ_I(x0, 242, "vf crop centered x");
    CHECK_EQ_I(y0, 0, "vf crop y0");
    CHECK(cw % 2 == 0 && x0 % 2 == 0, "vf crop Bayer-parity even");
    double portrait = (double)ch / cw; /* after rot90 the ratio flips */
    CHECK(fabs(portrait - 1080.0 / 1456.0) < 0.001, "vf aspect after rot");

    cp_crop_for_aspect(2016, 1136, 2016, 1136, &x0, &y0, &cw, &ch);
    CHECK_EQ_I(cw, 2016, "identity crop width");
    CHECK_EQ_I(ch, 1136, "identity crop height");
    CHECK_EQ_I(x0 + y0, 0, "identity crop full frame");

    /* crop rows: wide target on a taller frame (100x60 -> 2:1 = 100x50) */
    cp_crop_for_aspect(100, 50, 2, 1, &x0, &y0, &cw, &ch);
    CHECK_EQ_I(cw, 100, "row-crop width");
    CHECK_EQ_I(ch, 50, "row-crop height full");
    CHECK_EQ_I(y0, 0, "row-crop y0");
    cp_crop_for_aspect(100, 60, 2, 1, &x0, &y0, &cw, &ch);
    CHECK_EQ_I(ch, 50, "row-crop 60->50");
    CHECK_EQ_I(y0, 4, "row-crop centered (even-snapped)");
    CHECK(cw % 2 == 0 && ch % 2 == 0 && x0 % 2 == 0 && y0 % 2 == 0,
          "row-crop Bayer-parity even");
}

/* --- CCM (M47⑤g): Google imx363 matrices, xform, desat, warmth pick --- */
static void test_ccm(void)
{
    /* rows sum to 1 (luminance-preserving) — EXCEPT Google's TL84 green
     * row at 0.983: warm-fluorescent spikes green, and their tuning
     * deliberately under-gains it 1.7% (the raw bytes in the .so confirm
     * every other row sums to exactly 1.000000). Tolerance covers it. */
    for (int k = 0; k < 3; k++) {
        const float *m = k == 0 ? CP_CCM_D65 : k == 1 ? CP_CCM_TL84 : CP_CCM_INC;
        for (int r = 0; r < 3; r++) {
            float s = m[r*3] + m[r*3+1] + m[r*3+2];
            if (k != 1 || r != 1)
                CHECK(fabsf(s - 1.0f) < 1e-4, "ccm rows sum to 1");
            else
                CHECK(fabsf(s - 0.983f) < 1e-4, "TL84 green row 0.983");
        }
    }

    /* identity transform is bit-exact pass-through */
    uint8_t id[256];
    for (int v = 0; v < 256; v++) id[v] = (uint8_t)v;
    const float w1[3] = {1.0f, 1.0f, 1.0f};
    struct cp_xform t;
    cp_xform_init(&t, w1, NULL, id);
    uint8_t o3[3]; uint16_t p5;
    cp_apply_xform(&t, 100, 128, 160, o3, &p5);
    CHECK(o3[0] == 100 && o3[1] == 128 && o3[2] == 160, "identity xform passthrough");
    CHECK(p5 == (uint16_t)(((100 & 0xF8) << 8) | ((128 & 0xFC) << 3) | (160 >> 3)),
          "identity 565 pack");

    /* D65 hand-computed: (100,128,160) -> (80,126,179) — the matrix UN-greens
     * (B rises above its input, R falls: green's over-representation is undone) */
    cp_xform_init(&t, w1, CP_CCM_D65, id);
    cp_apply_xform(&t, 100, 128, 160, o3, NULL);
    CHECK(abs(o3[0] - 80) <= 2 && abs(o3[1] - 126) <= 2 && abs(o3[2] - 179) <= 2,
          "D65 ccm hand values");

    /* uniform gray through D65 stays gray (row sums 1) */
    cp_apply_xform(&t, 128, 128, 128, o3, NULL);
    CHECK(abs(o3[0] - 128) <= 1 && abs(o3[1] - 128) <= 1 && abs(o3[2] - 128) <= 1,
          "gray stays gray under ccm");

    /* highlight desat: wb b=1.5 on (100,128,160) drives CCM-B to ~306 —
     * the pixel scales to (59,91,255): B pinned, hue kept. A bare per-channel
     * clamp would leave (71,109,255) — visibly greener. */
    const float wb15[3] = {1.0f, 1.0f, 1.5f};
    cp_xform_init(&t, wb15, CP_CCM_D65, id);
    cp_apply_xform(&t, 100, 128, 160, o3, NULL);
    CHECK(o3[2] == 255, "desat pins the max channel");
    CHECK(abs(o3[0] - 59) <= 2 && abs(o3[1] - 91) <= 2, "desat hand values");
    CHECK(o3[1] < 100, "desat beats the green-tint clamp");

    /* M47⑤h shadow desat: a near-black green pixel (2,10,8) under D65 —
     * the raw matrix's -0.49*G red term clamps R below 0 (G-dominant floor
     * noise), leaving a green shadow. The knee (20) blends toward the CCM
     * output's own mean: channels collapse to ~equal (~(2,7,5) hand-worked),
     * level preserved. */
    cp_xform_init(&t, w1, CP_CCM_D65, id);
    cp_apply_xform(&t, 2, 10, 8, o3, NULL);
    CHECK(o3[1] - o3[0] <= 6, "shadow desat kills the green floor");
    CHECK(o3[2] - o3[0] <= 6, "shadow desat kills the blue floor too");
    CHECK(o3[0] >= 1 && o3[1] <= 9 && o3[2] <= 9, "shadow desat keeps the level");
    /* legacy (ccm NULL) keeps shadow desat off — dark pixel bit-exact */
    cp_xform_init(&t, w1, NULL, id);
    cp_apply_xform(&t, 2, 10, 8, o3, NULL);
    CHECK(o3[0] == 2 && o3[1] == 10 && o3[2] == 8, "legacy dark passthrough");

    /* M47⑤h: under a CCM the WB tables stay unclamped to 1020 so the CCM
     * and the highlight desat see true channel ratios (gains cap at 4x);
     * legacy keeps the clamp-at-255 bit-exact behavior */
    cp_xform_init(&t, (const float[3]){2.6f, 1.0f, 1.0f}, CP_CCM_D65, id);
    CHECK_EQ_I(t.wr[100], 260, "ccm-mode wb table exceeds 255");
    CHECK_EQ_I(t.wg[100], 100, "ccm-mode wb table unity is exact");
    cp_xform_init(&t, (const float[3]){2.6f, 1.0f, 1.0f}, NULL, id);
    CHECK_EQ_I(t.wr[100], 255, "legacy wb table clamps at 255");
    CHECK_EQ_I(t.shadow, 0, "legacy shadow desat off");

    /* M47⑤j clipped-highlight neutral (see CP_NEUTRAL_CLIP in campix.h): a
     * pixel with TWO OR MORE sites pinned at the sensor clip carries no
     * usable chroma — the pinned pair's true ratio is destroyed, unequal WB
     * gains would dye it and the hue-preserving desat would KEEP the dye
     * (the pink-center device artifact 2026-09-05). CCM mode collapses every
     * such pixel to achromatic; legacy keeps the old per-channel clamp. */
    const float wbg[3] = {1.3f, 1.0f, 1.9f};
    cp_xform_init(&t, wbg, CP_CCM_D65, id);
    cp_apply_xform(&t, 255, 255, 255, o3, NULL);
    /* hand: wb tables 331/255/484 -> n=356 -> desat s=183 -> 356*183+128>>8
     * = 255 on all three channels */
    CHECK(o3[0] == o3[1] && o3[1] == o3[2] && o3[0] >= 240,
          "clipped triad renders achromatic near-white");
    /* every nc==2 pair: only one channel is live, its "chroma" is a
     * saturation artifact -> achromatic too */
    cp_apply_xform(&t, 255, 255, 100, o3, NULL);
    CHECK(o3[0] == o3[1] && o3[1] == o3[2] && o3[0] >= 240,
          "clipped R+G pair renders achromatic near-white");
    cp_apply_xform(&t, 255, 100, 255, o3, NULL);
    CHECK(o3[0] == o3[1] && o3[1] == o3[2] && o3[0] >= 240,
          "clipped R+B pair renders achromatic near-white");
    cp_apply_xform(&t, 100, 255, 255, o3, NULL);
    CHECK(o3[0] == o3[1] && o3[1] == o3[2] && o3[0] >= 240,
          "clipped G+B pair renders achromatic near-white");
    cp_apply_xform(&t, 250, 250, 250, o3, NULL);
    CHECK(o3[0] == o3[1] && o3[1] == o3[2], "edge-of-clip triad achromatic");
    /* a single clipped channel still carries chroma -> normal CCM path
     * (M47⑤s: the old vector 200/220/255 is input-near-neutral — ratio
     * 1.28 — i.e. self-luminous cool-white content and now (correctly)
     * collapses; this vector, 150/210/255 ratio 1.7, is genuinely
     * chromatic single-clip) */
    cp_apply_xform(&t, 150, 210, 255, o3, NULL);
    CHECK(o3[0] != o3[1] || o3[1] != o3[2], "single-clip keeps ccm chroma");
    cp_xform_init(&t, wbg, NULL, id);
    cp_apply_xform(&t, 250, 250, 250, o3, NULL);
    CHECK(!(o3[0] == o3[1] && o3[1] == o3[2]),
          "legacy clip keeps per-channel clamp (bit-exact)");

    /* M47⑤j fringe ramp: the halo band around a blown warm lamp (device
     * 2026-09-05, inverted render chain) is bright but NEAR-NEUTRAL post-WB —
     * (226,211,223) inner / (189,175,188) outer, max/min 1.07-1.18 — while
     * its raw ratio rides off the illuminant locus, which the CCM blooms
     * into a magenta ring. The ramp must collapse it toward achromatic;
     * genuinely saturated bright colors (ratio ~3x) and dimmer pixels keep
     * their chroma untouched. */
    const float wbl[3] = {1.43f, 1.0f, 2.58f};
    uint8_t gam[256];
    cp_lut_gamma(gam, 2.2); /* device-visible domain for these */
    cp_xform_init(&t, wbl, CP_CCM_INC, gam);
    /* inner halo: raw (158,211,86) -> post-WB (226,211,222), s=213 ->
     * blend 83% toward the mean -> INC -> gamma (239,237,238) */
    cp_apply_xform(&t, 158, 211, 86, o3, NULL);
    int pmax = o3[0] > o3[2] ? o3[0] : o3[2];
    int pmin = o3[0] < o3[2] ? o3[0] : o3[2];
    if (o3[1] > pmax) pmax = o3[1];
    if (o3[1] < pmin) pmin = o3[1];
    CHECK(pmax - pmin <= 6, "inner halo collapses to near-neutral");
    CHECK(o3[1] >= 210, "inner halo keeps its level");
    /* outer fringe, mild ramp: raw (132,175,73) -> post-WB (189,175,188),
     * s=41 -> gamma (226,213,226): chroma shrinks where the ramp is partial */
    cp_apply_xform(&t, 132, 175, 73, o3, NULL);
    pmax = o3[0] > o3[2] ? o3[0] : o3[2];
    pmin = o3[0] < o3[2] ? o3[0] : o3[2];
    if (o3[1] > pmax) pmax = o3[1];
    if (o3[1] < pmin) pmin = o3[1];
    CHECK(pmax - pmin <= 15, "outer fringe chroma shrinks");
    /* saturated bright red (a lamp/LED): post-WB (365,120,232), ratio 3.0 —
     * the gate must fail, the color must survive */
    cp_apply_xform(&t, 255, 120, 90, o3, NULL);
    CHECK(o3[0] - o3[1] >= 120, "saturated red keeps its chroma");
    /* below the knee: the dim room floor of the same scene keeps chroma */
    cp_apply_xform(&t, 75, 104, 42, o3, NULL);
    CHECK(o3[2] - o3[1] >= 4, "dim pixels untouched by the fringe ramp");

    /* M47⑤j: the CCM is keyed on the (smoothed) colour temperature — the
     * vendor brackets' midpoints are the interpolation knots (cp_ccm_for_ct) */
    float m[9];
    cp_ccm_for_ct(8000, m);
    CHECK(0 == memcmp(m, CP_CCM_D65, sizeof m), "ct 8000 -> D65");
    cp_ccm_for_ct(20000, m);
    CHECK(0 == memcmp(m, CP_CCM_D65, sizeof m), "ct 20000 -> D65 clamp");
    cp_ccm_for_ct(4100, m);
    CHECK(0 == memcmp(m, CP_CCM_TL84, sizeof m), "ct 4100 -> TL84");
    cp_ccm_for_ct(2800, m);
    CHECK(0 == memcmp(m, CP_CCM_INC, sizeof m), "ct 2800 -> INC");
    cp_ccm_for_ct(1000, m);
    CHECK(0 == memcmp(m, CP_CCM_INC, sizeof m), "ct 1000 -> INC clamp");
    cp_ccm_for_ct(3600, m); /* INC/TL84 knot midpoint */
    int mid = 1;
    for (int i = 0; i < 9; i++) {
        float want = 0.5f * (CP_CCM_INC[i] + CP_CCM_TL84[i]);
        if (fabsf(m[i] - want) > 1e-4) mid = 0;
    }
    CHECK(mid, "ct 3600 -> INC/TL84 midpoint");
    cp_ccm_for_ct(5275, m); /* TL84/D65 knot midpoint */
    mid = 1;
    for (int i = 0; i < 9; i++) {
        float want = 0.5f * (CP_CCM_TL84[i] + CP_CCM_D65[i]);
        if (fabsf(m[i] - want) > 1e-4) mid = 0;
    }
    CHECK(mid, "ct 5275 -> TL84/D65 midpoint");

    /* end-to-end through the debayer pass: site_const interior (100,128,160)
     * + D65 + identity lut matches cp_apply_xform's hand values */
    uint8_t g[8 * 8];
    site_const(g, 8, 8);
    uint8_t out[8 * 8 * 3];
    cp_debayer_rot(g, 8, 8, w1, CP_CCM_D65, id, 0, 8, 8, out, NULL);
    int ok = 1;
    for (uint32_t y = 1; y < 7; y++)
        for (uint32_t x = 1; x < 7; x++) {
            const uint8_t *p = out + (y * 8 + x) * 3;
            if (abs(p[0] - 80) > 2 || abs(p[1] - 126) > 2 || abs(p[2] - 179) > 2) ok = 0;
        }
    CHECK(ok, "debayer+D65 interior hand values");
}

/* --- M47⑤i: threaded split + divide-kill tables + blocked rotated walk --- */

/* cp_rot_rows over arbitrary partitions must be bit-exact with the
 * cp_debayer_rot one-call form, the precomputed desat tables must equal
 * the integer divides they replace, and the cache-blocked rotated walk
 * must equal the naive row walk byte for byte — that equivalence is what
 * makes the threaded pixel chain a restructure, not a regrade. */
static void test_rot_split(void)
{
    static uint8_t g[40 * 30];
    static uint8_t a[40 * 30 * 3], b[40 * 30 * 3], c[40 * 30 * 3];
    static uint16_t a5[40 * 30], b5[40 * 30], c5[40 * 30];
    unsigned long rng = 20260905;
    #define RND() (rng = rng * 6364136223846793005UL + 1442695040888963407UL, \
                   (uint32_t)(rng >> 33))
    uint8_t id[256];
    for (int v = 0; v < 256; v++) id[v] = (uint8_t)v;

    int ok = 1;
    for (int trial = 0; trial < 200; trial++) {
        uint32_t w = 4 + 2 * (RND() % 18);   /* even 4..38 */
        uint32_t h = 4 + 2 * (RND() % 13);
        for (size_t i = 0; i < (size_t)w * h; i++) g[i] = RND() & 0xff;
        float wb[3];
        for (int k = 0; k < 3; k++) wb[k] = 1.0f + (RND() % 300) / 100.0f;
        int rot = (int[3]){0, 90, 270}[RND() % 3];
        uint32_t rw = (rot == 90 || rot == 270) ? h : w;
        uint32_t rh = (rot == 90 || rot == 270) ? w : h;
        uint32_t ow = 2 + RND() % rw;
        uint32_t oh = 2 + RND() % rh;
        const float *ccm = (RND() & 1) ? CP_CCM_D65 : NULL;

        cp_debayer_rot(g, w, h, wb, ccm, id, rot, ow, oh, a, a5);
        struct cp_rot R;
        if (!cp_rot_init(&R, w, h, wb, ccm, id, rot, ow, oh)) {
            CHECK(0, "rot_init failed on valid dims");
            break;
        }
        /* two disjoint ranges at a random split point */
        uint32_t k = RND() % oh;
        cp_rot_rows(&R, g, 0, k, b, b5);
        cp_rot_rows(&R, g, k, oh, b, b5);
        cp_rot_free(&R);
        /* naive reference: the blocked walk must equal it on every byte */
        struct cp_rot Rn;
        cp_rot_init(&Rn, w, h, wb, ccm, id, rot, ow, oh);
        cp_rot_rows_naive(&Rn, g, 0, oh, c, c5);
        cp_rot_free(&Rn);
        if (memcmp(a, b, (size_t)ow * oh * 3) ||
            memcmp(a5, b5, (size_t)ow * oh * 2) ||
            memcmp(a, c, (size_t)ow * oh * 3) ||
            memcmp(a5, c5, (size_t)ow * oh * 2)) {
            printf("FAIL: split/blocked mismatch trial %d (%ux%u rot%d -> %ux%u)\n",
                   trial, w, h, rot, ow, oh);
            ok = 0;
            break;
        }
    }
    CHECK(ok, "rot split + blocked walk bit-exact at random partitions");

    /* divide-kill tables == the formulas they replace */
    struct cp_xform t;
    const float w1[3] = {1.0f, 1.0f, 1.0f};
    cp_xform_init(&t, w1, CP_CCM_D65, id);
    CHECK_EQ_I(t.shadow, CP_SHADOW_KNEE, "ccm shadow knee on");
    int tab = 1;
    for (int i = 0; i < CP_SHADOW_KNEE; i++)
        if (t.f8[i] != (uint16_t)((i << 8) / CP_SHADOW_KNEE)) tab = 0;
    CHECK(tab, "f8 table == (i<<8)/shadow");
    tab = 1;
    for (int mx = 256; mx < CP_DESAT_MAX; mx++)
        if (t.s8[mx] != (uint16_t)((255 << 8) / mx)) tab = 0;
    CHECK(tab, "s8 table == (255<<8)/mx");
    /* a hot bright pixel (highlight desat path) through the s8 table:
     * M47⑤s note: the old vector 200/220/240 is input-near-neutral (ratio
     * 1.2) and now collapses achromatic under the two-domain fringe law —
     * correct for self-luminous content. This one, 120/220/240 ratio 2.0,
     * is chromatic in BOTH domains and rides the plain CCM+desat path:
     * wb (1,2,4) -> (120,440,960) -> CCM out (-128, 389, 1270) ->
     * s = 65280/1270 = 51 -> (0*51, 389*51, 1270*51 each +128 >>8)
     * = (0, 77, 253), hand-worked against the same Q14 quantization the
     * matrix init does */
    cp_xform_init(&t, (const float[3]){1.0f, 2.0f, 4.0f}, CP_CCM_D65, id);
    uint8_t o3[3];
    uint8_t gam2[256];
    cp_lut_gamma(gam2, 2.2);
    cp_apply_xform(&t, 120, 220, 240, o3, NULL);
    CHECK_EQ_I(o3[0], 0, "hot px R");
    CHECK_EQ_I(o3[1], 77, "hot px G (desat table)");
    CHECK_EQ_I(o3[2], 253, "hot px B (desat pins near max)");

    /* M47⑤s self-luminous halo: input-neutral bright pixel under gains
     * that dye it (the red-circle repro — an emissive screen's halo skirt
     * in a dim warm room; raw neutral, post-WB 286/200/514 under the
     * tungsten-ish correction). The input-domain branch of the fringe
     * gate must collapse it achromatic even though the POST-WB ratio
     * (2.57) would have failed the ⑤j test and let the CCM bloom a
     * saturated ring. */
    cp_xform_init(&t, (const float[3]){1.43f, 1.0f, 2.58f}, CP_CCM_INC, gam2);
    cp_apply_xform(&t, 200, 200, 200, o3, NULL);
    CHECK(o3[0] == o3[1] && o3[1] == o3[2] && o3[0] >= 240,
          "input-neutral bright halo collapses achromatic (red circle)");
}

/* --- M47⑤j CT temporal smoothing (libcamera libipa/awb.cpp port) --- */
static void test_smooth(void)
{
    struct cp_ct_smooth s;
    cp_ct_smooth_init(&s);
    CHECK(!s.primed && fabs(s.ct - 5000.0) < 1e-9,
          "smooth starts unprimed @ 5000K (kDefaultColourTemperature)");
    cp_ct_smooth_step(&s, 4100.0, 0.5);
    CHECK(!s.primed, "dark stats freeze (AwbStats::valid)");
    cp_ct_smooth_step(&s, 4100.0, 50.0);
    CHECK(s.primed && fabs(s.ct - 4100.0) < 1e-9,
          "first valid measurement seeds");
    cp_ct_smooth_step(&s, 6000.0, 60.0);
    CHECK(fabs(s.ct - (6000.0 * 0.2 + 4100.0 * 0.8)) < 1e-9,
          "ema speed 0.2 toward the measurement");
    cp_ct_smooth_step(&s, 9000.0, 1.0);
    CHECK(fabs(s.ct - (6000.0 * 0.2 + 4100.0 * 0.8)) < 1e-9,
          "dark freezes mid-stream");
    cp_ct_smooth_step(&s, 0.0, 60.0);
    CHECK(fabs(s.ct - (6000.0 * 0.2 + 4100.0 * 0.8)) < 1e-9,
          "no-search-result (ct 0) freezes");
}

/* --- M47⑤j bayes CT search (libcamera awb_bayes coarseSearch on the
 * vendor imx363 ct_curve) --- */
static void test_bayes(void)
{
    /* on-locus recovery: means sitting exactly on a curve knot must come
     * back at that knot (the curve gains zero the error there) */
    int ok = 1;
    for (int i = 0; i < CP_AWB_N; i++) {
        double m[3] = {CP_AWB_R[i] * 100.0, 100.0, CP_AWB_B[i] * 100.0};
        double ct = cp_awb_search(m);
        if (fabs(ct - CP_AWB_CT[i]) > 0.03 * CP_AWB_CT[i]) ok = 0;
    }
    CHECK(ok, "per-knot means recover their CT");

    /* off-locus: resp (0.4, 1, 0.35) matches NO real illuminant (R says
     * ~6400K, B says ~2650K) — the curve-constrained search must land it in
     * the warm half (hand: balance ~3850K), never let the gains run free */
    double mg[3] = {40.0, 100.0, 35.0};
    double ct = cp_awb_search(mg);
    CHECK(ct >= CP_AWB_CT[CP_AWB_N - 1] && ct <= 5200.0,
          "off-locus green lands in the warm half");

    /* invalid stats: black frame -> 0, the caller freezes on it */
    double z[3] = {0.0, 0.0, 0.0};
    CHECK(cp_awb_search(z) == 0.0, "zero means -> ct 0 (freeze signal)");

    /* gains are a pure function of CT off the vendor curve: 6500K ->
     * (1/0.39831, 1, 1/0.68913) = (2.5106, 1, 1.4511) */
    float wb[3];
    cp_awb_gains(6500.0, wb);
    CHECK(fabsf(wb[0] - 2.5106f) < 0.01f && fabsf(wb[1] - 1.0f) < 1e-6f &&
          fabsf(wb[2] - 1.4511f) < 0.01f,
          "gains at 6500K match the vendor curve");
}

/* --- M47⑤k tone (RPi contrast stretch port) --- */
static void test_tone(void)
{
    struct cp_tone t;
    uint8_t id[256], gam[256];
    for (int v = 0; v < 256; v++) id[v] = (uint8_t)v;
    cp_lut_gamma(gam, 2.2);

    /* no stats: gamma only */
    cp_tone_step(&t, NULL, 0, gam, 1.0, 0.0);
    CHECK(0 == memcmp(t.lut, gam, 256), "tone no-hist = gamma");
    uint32_t h0[256] = {0};
    cp_tone_step(&t, h0, 0, gam, 1.0, 0.0);
    CHECK(0 == memcmp(t.lut, gam, 256), "tone empty-hist = gamma");

    /* quantile semantics: hist{4:300, 128:6000, 242:3700}, total 10000 ->
     * q01 want=100 -> 4 (acc 300 first >= 100), q50 want=5000 -> 128
     * (6300), q95 want=9500 -> 242 (10000) */
    uint32_t h[256] = {0};
    h[4] = 300; h[128] = 6000; h[242] = 3700;
    CHECK_EQ_I(cp_hist_quantile(h, 10000, 0.01), 4, "q01 -> 4");
    CHECK_EQ_I(cp_hist_quantile(h, 10000, 0.5), 128, "q50 -> 128");
    CHECK_EQ_I(cp_hist_quantile(h, 10000, 0.95), 242, "q95 -> 242");

    /* histogram already AT the target levels + identity gamma + contrast 1:
     * knots ks == vs -> the composed LUT is the identity exactly */
    cp_tone_step(&t, h, 10000, id, 1.0, 0.0);
    int ident = 1;
    for (int i = 0; i < 256; i++) if (t.lut[i] != (uint8_t)i) ident = 0;
    CHECK(ident, "tone at-target is identity");

    /* manual contrast/brightness ride on top (identity stretch case):
     * lut[128] = 128*1 + 2 + .5 -> 130; ends clamp */
    cp_tone_step(&t, h, 10000, id, 1.08, 2.0);
    CHECK_EQ_I(t.lut[128], 130, "manual contrast @128");
    CHECK_EQ_I(t.lut[255], 255, "manual contrast clamps hi");
    CHECK_EQ_I(t.lut[0], 0, "manual contrast clamps lo");

    /* dark frame (all mass at 6, identity gamma): 1st percentile crushes
     * toward black — min(6, 4+2)=6 -> level 4, so 6 maps to 4 */
    memset(h, 0, sizeof h);
    h[6] = 10000;
    cp_tone_step(&t, h, 10000, id, 1.0, 0.0);
    CHECK_EQ_I(t.lut[6], 4, "dark crush 6 -> 4");
    CHECK_EQ_I(t.lut[0], 0, "dark keeps black at 0");

    /* THE RPi law: the median is PINNED ("limit the apparent amount of
     * global brightness shift") — median linear 50 through gam 2.2 lands
     * at gam[50] and the composed LUT maps 50 back to exactly gam[50] */
    memset(h, 0, sizeof h);
    h[50] = 10000;
    cp_tone_step(&t, h, 10000, gam, 1.0, 0.0);
    CHECK_EQ_I(t.lut[50], gam[50], "median pinned through gamma");
}

/* --- M47⑤k denoise (hqdn3d port) --- */
static void test_nr(void)
{
    /* precalc: on/off flag + f==0 center */
    int16_t *ct = (int16_t *)malloc(sizeof(int16_t) * CP_NR_TAB);
    cp_nr_precalc(0.0, ct);
    CHECK_EQ_I(ct[0], 0, "precalc 0 = off flag");
    cp_nr_precalc(4.0, ct);
    CHECK_EQ_I(ct[0], 1, "precalc 4 = on flag");
    /* center i=0: ffmpeg's f carries a +15 dither ((0*32+15)/512), so the
     * zero-diff entry is small but NOT zero — hand: 0.999885^87.7 * 256 *
     * 15/512 = 7.4 -> 7 (the 16-bit correction; 127+7 stays inside the
     * same output byte, which is why identical pixels still pass through) */
    CHECK_EQ_I(ct[(256 << CP_NR_LUT_BITS) + 0], 7, "precalc center dither = 7");
    free(ct);

    /* uniform frame: identical pixels pass through lowpass bit-exact
     * (coef[0] == 0), first AND later frames */
    static uint8_t f[32 * 32 * 3];
    struct cp_nr n;
    CHECK(cp_nr_init(&n, 32, 32, 4.0, 3.0, 6.0, 4.5), "nr init 32x32");
    memset(f, 100, sizeof f);
    cp_nr_frame(&n, f);
    cp_nr_frame(&n, f);
    int uni = 1;
    for (size_t i = 0; i < sizeof f; i++) if (f[i] != 100) uni = 0;
    CHECK(uni, "uniform frame unchanged");
    cp_nr_free(&n);

    /* step edge (100 | 200): diffs are 0 or 100 — both far past the
     * similarity knee, correction ~0, frame stays bit-exact (motion and
     * edges keep their sharpness: the hqdn3d selling point) */
    static uint8_t e[16 * 16 * 3], e0[16 * 16 * 3];
    for (uint32_t y = 0; y < 16; y++)
        for (uint32_t x = 0; x < 16; x++) {
            uint8_t v = x < 8 ? 100 : 200;
            for (int c = 0; c < 3; c++) e[(y * 16 + x) * 3 + c] = v;
        }
    memcpy(e0, e, sizeof e);
    CHECK(cp_nr_init(&n, 16, 16, 4.0, 3.0, 6.0, 4.5), "nr init 16x16");
    cp_nr_frame(&n, e);
    cp_nr_frame(&n, e);
    CHECK(0 == memcmp(e, e0, sizeof e), "hard edge preserved bit-exact");
    cp_nr_free(&n);

    /* noise collapse: +-6 uniform noise (var 14) over base 100, fresh
     * noise per frame. Measured steady state of the port at ffmpeg
     * defaults (ls4/lt6): frame 0 (spatial + seed) var ~12, then the
     * temporal lowpass converges to var ~4.5-5.0 — the temporal correction
     * saturates near 1.6 px/frame at large diffs, so this IS the honest
     * hqdn3d-default floor (2.4x variance kill, sigma 1.55x). Stronger
     * denoising is a --nr strength call on device, not a port bug. */
    #define W 64
    #define H 48
    static uint8_t g[W * H * 3];
    unsigned long rng = 20260905;
    CHECK(cp_nr_init(&n, W, H, 4.0, 3.0, 6.0, 4.5), "nr init noise");
    for (int fr = 0; fr < 3; fr++) {
        for (size_t i = 0; i < sizeof g; i++) {
            rng = rng * 6364136223846793005UL + 1442695040888963407UL;
            g[i] = (uint8_t)(100 + (int)((rng >> 33) % 13) - 6);
        }
        cp_nr_frame(&n, g);
    }
    cp_nr_free(&n);
    double mean = 0, var = 0;
    size_t cnt = 0;
    for (uint32_t y = 8; y < H - 8; y++)
        for (uint32_t x = 8; x < W - 8; x++) {
            mean += g[((size_t)y * W + x) * 3];
            cnt++;
        }
    mean /= (double)cnt;
    for (uint32_t y = 8; y < H - 8; y++)
        for (uint32_t x = 8; x < W - 8; x++) {
            double d = (double)g[((size_t)y * W + x) * 3] - mean;
            var += d * d;
        }
    var /= (double)cnt;
    CHECK(fabs(mean - 100.0) < 3.0, "nr keeps the mean");
    CHECK(var < 6.0, "nr collapses noise variance to the hqdn3d floor");

    CHECK(!cp_nr_init(&n, 0, 0, 4, 3, 6, 4.5), "nr rejects zero dims");
    cp_nr_free(&n);

    /* band equivalence (the parallel form the viewfinder ships): two
     * independent states, same frame sequence, one walked whole-plane,
     * one prime + disjoint row bands — must be bit-exact (same discipline
     * test_rot_split pins for the debayer walk). Spatial stays off: banded
     * spatial is impossible and cp_nr_rows guards it. */
    static uint8_t gA[W * H * 3], gB[W * H * 3];
    struct cp_nr a, b;
    CHECK(cp_nr_init(&a, W, H, 0.0, 0.0, 12.0, 0.0), "nr band init A");
    CHECK(cp_nr_init(&b, W, H, 0.0, 0.0, 12.0, 0.0), "nr band init B");
    CHECK(!a.coefs[0][0] && !b.coefs[0][0], "nr band spatial off");
    int band_ok = 1;
    for (int fr = 0; fr < 3; fr++) {
        for (size_t i = 0; i < sizeof gA; i++) {
            rng = rng * 6364136223846793005UL + 1442695040888963407UL;
            gA[i] = gB[i] = (uint8_t)(100 + (int)((rng >> 33) % 13) - 6);
        }
        cp_nr_frame(&a, gA);                    /* whole-plane form */
        cp_nr_prime(&b, gB);                    /* single-threaded prime */
        uint32_t k = 7 + (fr * 11) % (H - 14);  /* a fresh split each frame */
        cp_nr_rows(&b, gB, 0, k);
        cp_nr_rows(&b, gB, k, H);
        if (memcmp(gA, gB, sizeof gA)) band_ok = 0;
    }
    CHECK(band_ok, "nr banded == whole bit-exact");
    cp_nr_free(&a);
    cp_nr_free(&b);
    /* the spatial guard: rows with spatial on must leave the frame alone */
    CHECK(cp_nr_init(&a, W, H, 4.0, 0.0, 12.0, 0.0), "nr guard init");
    memcpy(gB, gA, sizeof gA);
    cp_nr_prime(&a, gB);
    cp_nr_rows(&a, gB, 0, H);
    CHECK(0 == memcmp(gB, gA, sizeof gA), "nr_rows refuses spatial (no-op)");
    cp_nr_free(&a);
}

/* --- M47⑤l noise-factor laws: sharpen scaling + denoise re-knee --- */
static void test_noise(void)
{
    /* nf: sqrt of the total AEC gain, clamped to >= 1 (RPi modeFactor law) */
    CHECK(fabs(cp_noise_factor(1.0, 1.0) - 1.0) < 1e-9, "nf 1x = 1");
    CHECK(fabs(cp_noise_factor(16.0, 2.0) - sqrt(32.0)) < 1e-9,
          "nf darkest rung sqrt(32)");
    CHECK(fabs(cp_noise_factor(0.5, 1.0) - 1.0) < 1e-9, "nf clamps low");
    CHECK(fabs(cp_noise_factor(4.0, 0.25) - 1.0) < 1e-9, "nf dgain<1 clamps");

    /* sharpen scaling = sharpen.cpp prepare() at userStrength 1:
     * thr *= nf, strength /= nf, limit /= nf */
    int sq8 = 128, thr = 4, lim = 30;
    cp_sharp_adapt(1.0, &sq8, &thr, &lim);
    CHECK(sq8 == 128 && thr == 4 && lim == 30, "sharp adapt nf=1 identity");
    sq8 = 128; thr = 4; lim = 30;
    cp_sharp_adapt(4.0, &sq8, &thr, &lim);
    CHECK(thr == 16, "sharp thr *= nf (16)");
    CHECK(sq8 == 32, "sharp strength /= nf (32)");
    CHECK(lim == 8, "sharp limit /= nf (8)");

    /* denoise re-knee (⑤m spatial): adapt(n, k*ls, k*cs) must land EXACTLY
     * on a fresh init(k*ls, k*cs) — the spatial tables are the knee, and
     * the line state must reset to the calloc'd zeros (it chains rows, so
     * it cannot survive a table swap) */
    struct cp_nr a, b, c;
    CHECK(cp_nr_init(&a, 16, 16, 4.0, 3.0, 0.0, 0.0), "adapt init a");
    a.line[0][5] = 0xBEEF;               /* stale line state must not survive */
    a.line[2][9] = 0xBEEF;
    CHECK(cp_nr_init(&b, 16, 16, 16.0, 12.0, 0.0, 0.0), "adapt init b");
    cp_nr_adapt(&a, 4.0 * 4.0, 3.0 * 4.0);   /* nf = 4 (gain 16) */
    CHECK(0 == memcmp(a.coefs[0], b.coefs[0], sizeof(int16_t) * CP_NR_TAB),
          "adapt ls*4 == fresh init ls*4");
    CHECK(0 == memcmp(a.coefs[1], b.coefs[1], sizeof(int16_t) * CP_NR_TAB),
          "adapt cs*4 == fresh init cs*4");
    CHECK(0 == memcmp(a.coefs[2], b.coefs[2], sizeof(int16_t) * CP_NR_TAB),
          "adapt leaves the off temporal table off");
    {
        int zero = 1;
        for (int p = 0; p < 3 && zero; p++)
            for (int x = 0; x < 16; x++)
                if (a.line[p][x]) { zero = 0; break; }
        CHECK(zero, "adapt zeroes line_ant (tables swapped under it)");
    }
    /* and it really moved: the re-kneed table differs from the base one */
    CHECK(cp_nr_init(&c, 16, 16, 4.0, 3.0, 0.0, 0.0), "adapt init c");
    CHECK(0 != memcmp(a.coefs[0], c.coefs[0], sizeof(int16_t) * CP_NR_TAB),
          "re-knee actually changed the table");
    cp_nr_free(&a);
    cp_nr_free(&b);
    cp_nr_free(&c);
}

/* --- M47⑤k look: saturation / sharpen / 565 pack --- */
static void test_look(void)
{
    uint8_t id[256];
    for (int v = 0; v < 256; v++) id[v] = (uint8_t)v;
    const float w1[3] = {1.0f, 1.0f, 1.0f};

    /* saturation Q8 294 (1.15x): neutral in -> neutral out EXACTLY; a tinted
     * pixel spreads from its luma — hand: y=123 -> (97,129,165) */
    struct cp_xform t;
    cp_xform_init(&t, w1, NULL, id);
    t.sat = 294;
    uint8_t o3[3];
    cp_apply_xform(&t, 128, 128, 128, o3, NULL);
    CHECK(o3[0] == 128 && o3[1] == 128 && o3[2] == 128, "sat neutral exact");
    cp_apply_xform(&t, 100, 128, 160, o3, NULL);
    CHECK_EQ_I(o3[0], 97, "sat R hand");
    CHECK_EQ_I(o3[1], 129, "sat G hand");
    CHECK_EQ_I(o3[2], 165, "sat B hand");
    cp_xform_init(&t, w1, NULL, id); /* sat back to 256 = off */

    /* sharpen (out-of-place, row-banded): flat frame bit-exact (correction
     * 0 <= threshold -> src copied); vertical edge 90|190 with threshold 4 /
     * strength 256 / limit 30 — hand: x=8 correction 4*190-(90+190+190+190)
     * =100 -> adj 100 clamps to +30 -> 220; x=7 4*90-(90+190+90+90)=-100 ->
     * -30 -> 60; x=6/9 correction 0 -> untouched; top/bottom rows self-
     * clamp vertically -> same +30 */
    static uint8_t f[16 * 12 * 3], d[16 * 12 * 3];
    memset(f, 90, sizeof f);
    cp_sharpen(f, d, 16, 12, 0, 12, 256, 4, 30);
    int flat = 1;
    for (size_t i = 0; i < sizeof f; i++) if (d[i] != 90) flat = 0;
    CHECK(flat, "sharpen flat frame bit-exact");
    for (uint32_t y = 0; y < 12; y++)
        for (uint32_t x = 0; x < 16; x++) {
            uint8_t v = x < 8 ? 90 : 190;
            for (int c = 0; c < 3; c++) f[(y * 16 + x) * 3 + c] = v;
        }
    cp_sharpen(f, d, 16, 12, 0, 12, 256, 4, 30);
    CHECK_EQ_I(d[(5 * 16 + 8) * 3], 220, "sharpen edge +30 clamped");
    CHECK_EQ_I(d[(5 * 16 + 7) * 3], 60, "sharpen pre-edge -30");
    CHECK_EQ_I(d[(5 * 16 + 6) * 3], 90, "sharpen flat left untouched");
    CHECK_EQ_I(d[(5 * 16 + 9) * 3], 190, "sharpen flat right untouched");
    CHECK_EQ_I(d[(0 * 16 + 8) * 3], 220, "sharpen top edge row clamps");
    CHECK_EQ_I(d[(11 * 16 + 8) * 3], 220, "sharpen bottom row clamps");

    /* band equivalence: disjoint row ranges into one dst == the whole walk
     * into another — the parallel form must be a restructure, not a regrade */
    static uint8_t f2[37 * 23 * 3], dw[37 * 23 * 3], db[37 * 23 * 3];
    unsigned long srng = 991;
    for (int i = 0; i < (int)sizeof f2; i++) {
        srng = srng * 6364136223846793005UL + 1442695040888963407UL;
        f2[i] = (uint8_t)(srng >> 33);
    }
    cp_sharpen(f2, dw, 37, 23, 0, 23, 384, 3, 24);
    cp_sharpen(f2, db, 37, 23, 0, 9, 384, 3, 24);
    cp_sharpen(f2, db, 37, 23, 9, 17, 384, 3, 24);
    cp_sharpen(f2, db, 37, 23, 17, 23, 384, 3, 24);
    CHECK(0 == memcmp(dw, db, sizeof dw), "sharpen banded == whole bit-exact");

    /* pack565 agrees with the inline pack in cp_apply_xform byte for byte */
    static uint8_t rgb[64 * 3];
    static uint16_t p5[64], p5ref[64];
    unsigned long rng = 4711;
    for (int i = 0; i < 64 * 3; i++) {
        rng = rng * 6364136223846793005UL + 1442695040888963407UL;
        rgb[i] = (uint8_t)(rng >> 33);
    }
    cp_xform_init(&t, w1, NULL, id);
    for (int i = 0; i < 64; i++)
        cp_apply_xform(&t, rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2], NULL, &p5ref[i]);
    cp_pack565(rgb, 64, p5);
    CHECK(0 == memcmp(p5, p5ref, sizeof p5), "pack565 == inline pack");
}

/* ⑤o area-average box: scale-1 identity, naive-reference equivalence, and
 * the banded contract (disjoint row ranges bit-exact with the whole walk) */
static void test_box(void)
{
    /* scale 1: every span is one pixel, both outputs copy the source */
    static uint16_t s1[9 * 7];
    static uint16_t i5[9 * 7];
    static uint8_t i8[9 * 7 * 3];
    unsigned long rng = 7717;
    for (int i = 0; i < 9 * 7; i++) {
        rng = rng * 6364136223846793005UL + 1442695040888963407UL;
        s1[i] = (uint16_t)(rng >> 49);
    }
    struct cp_box b1;
    CHECK(cp_box_init(&b1, 9, 7, 9, 7), "box init 1:1");
    cp_box_rows(&b1, s1, 0, 7, i5, i8);
    int ident = 1;
    for (int i = 0; i < 9 * 7; i++) {
        uint16_t v = s1[i];
        uint32_t rv = (v >> 11) & 0x1F, gv = (v >> 5) & 0x3F, bv = v & 0x1F;
        uint32_t r = (rv << 3) | (rv >> 2), g = (gv << 2) | (gv >> 4),
                 b = (bv << 3) | (bv >> 2);
        if (i5[i] != (uint16_t)(((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)))
            ident = 0;
        if (i8[i * 3] != r || i8[i * 3 + 1] != g || i8[i * 3 + 2] != b)
            ident = 0;
    }
    CHECK(ident, "box scale-1 == unpack identity");
    cp_box_free(&b1);

    /* general ratio: naive per-pixel reference vs the row walk, plus the
     * banded contract; 37x23 -> 19x11 spans 1-2 px per dim */
    static uint16_t src[37 * 23], w5[19 * 11], b5[19 * 11];
    static uint8_t w8[19 * 11 * 3], b8[19 * 11 * 3];
    rng = 31337;
    for (int i = 0; i < 37 * 23; i++) {
        rng = rng * 6364136223846793005UL + 1442695040888963407UL;
        src[i] = (uint16_t)(rng >> 49);
    }
    struct cp_box b2;
    CHECK(cp_box_init(&b2, 37, 23, 19, 11), "box init 37->19");
    CHECK(!cp_box_init(&b2, 3, 3, 4, 3), "box rejects upscale");
    cp_box_init(&b2, 37, 23, 19, 11);
    cp_box_rows(&b2, src, 0, 11, w5, w8);
    cp_box_rows(&b2, src, 0, 4, b5, b8);
    cp_box_rows(&b2, src, 4, 9, b5, b8);
    cp_box_rows(&b2, src, 9, 11, b5, b8);
    CHECK(0 == memcmp(w5, b5, sizeof w5) && 0 == memcmp(w8, b8, sizeof w8),
          "box banded == whole bit-exact");
    int naive_ok = 1;
    for (uint32_t oy = 0; oy < 11 && naive_ok; oy++) {
        uint32_t sy0 = oy * 23 / 11;
        uint32_t sy1 = ((oy + 1) * 23 + 11 - 1) / 11;
        for (uint32_t ox = 0; ox < 19 && naive_ok; ox++) {
            uint32_t x0 = ox * 37 / 19;
            uint32_t x1 = ((ox + 1) * 37 + 19 - 1) / 19;
            uint32_t sr = 0, sg = 0, sb = 0;
            for (uint32_t sy = sy0; sy < sy1; sy++)
                for (uint32_t sx = x0; sx < x1; sx++) {
                    uint16_t v = src[sy * 37 + sx];
                    uint32_t rv = (v >> 11) & 0x1F, gv = (v >> 5) & 0x3F,
                             bv = v & 0x1F;
                    sr += (rv << 3) | (rv >> 2);
                    sg += (gv << 2) | (gv >> 4);
                    sb += (bv << 3) | (bv >> 2);
                }
            uint32_t cnt = (x1 - x0) * (sy1 - sy0);
            uint32_t r = (sr + cnt / 2) / cnt, g = (sg + cnt / 2) / cnt,
                     b = (sb + cnt / 2) / cnt;
            if (w8[(oy * 19 + ox) * 3] != (uint8_t)r ||
                w8[(oy * 19 + ox) * 3 + 1] != (uint8_t)g ||
                w8[(oy * 19 + ox) * 3 + 2] != (uint8_t)b)
                naive_ok = 0;
        }
    }
    CHECK(naive_ok, "box == naive area reference");
    cp_box_free(&b2);
}

/* ⑤v linear-domain fold: identity == the one-stage rot walk (bit-exact),
 * naive absolute-coordinate reference (pins the staged-strip offset math),
 * the banded contract, and the domain-order delta vs the ⑤o two-stage chain
 * (flat frame: 565 IDENTICAL, 888 within the 565 round-trip's own
 * quantization; gradient: within the gamma-curvature + quantization bound). */
static void test_fold(void)
{
    uint8_t lut[256];
    cp_lut_gamma(lut, 2.2);
    const float wb[3] = {1.3f, 1.0f, 1.7f};

    /* identity dims: every bin is one sample -> fold == cp_rot_rows exactly,
     * both outputs, all three rotations (CCM + gamma live; the rot-270 leg
     * also carries the caller-side sat override through the fold's xform) */
    static uint8_t g1[24 * 18];
    static uint8_t r8[24 * 18 * 3], f8[24 * 18 * 3];
    static uint16_t r5[24 * 18], f5[24 * 18];
    unsigned long rng = 4242;
    for (int i = 0; i < 24 * 18; i++) {
        rng = rng * 6364136223846793005UL + 1442695040888963407UL;
        g1[i] = (uint8_t)(rng >> 33);
    }
    int ident = 1;
    for (int ri = 0; ri < 3; ri++) {
        int rot = ri == 0 ? 0 : (ri == 1 ? 90 : 270);
        uint32_t fw = rot ? 18 : 24, fh = rot ? 24 : 18;
        struct cp_rot R;
        struct cp_fold F;
        if (!cp_rot_init(&R, 24, 18, wb, CP_CCM_D65, lut, rot, fw, fh) ||
            !cp_fold_init(&F, 24, 18, wb, CP_CCM_D65, lut, rot, fw, fh))
            ident = 0;
        if (rot == 270) {
            F.R.xf.sat = 300;
            R.xf.sat = 300;
        }
        cp_rot_rows(&R, g1, 0, fh, r8, r5);
        cp_fold_rows(&F, g1, 0, fh, f8, f5);
        if (memcmp(r8, f8, (size_t)fw * fh * 3) ||
            memcmp(r5, f5, (size_t)fw * fh * 2))
            ident = 0;
        cp_rot_free(&R);
        cp_fold_free(&F);
    }
    CHECK(ident, "fold identity == rot walk bit-exact (0/90/270)");
    struct cp_fold Fr;
    CHECK(!cp_fold_init(&Fr, 24, 18, wb, NULL, lut, 0, 25, 10),
          "fold rejects upscale");

    /* general ratio, rot 90: banded == whole, plus single-sided output
     * calls (the optional out / out565 guards) */
    static uint8_t g2[64 * 48];
    static uint8_t w8[30 * 42 * 3], b8[30 * 42 * 3];
    static uint16_t w5[30 * 42], b5[30 * 42];
    rng = 909;
    for (int i = 0; i < 64 * 48; i++) {
        rng = rng * 6364136223846793005UL + 1442695040888963407UL;
        g2[i] = (uint8_t)(rng >> 33);
    }
    struct cp_fold F2;
    CHECK(cp_fold_init(&F2, 64, 48, wb, CP_CCM_D65, lut, 90, 30, 42),
          "fold init 64x48 rot90 -> 30x42");
    cp_fold_rows(&F2, g2, 0, 42, w8, w5);
    cp_fold_rows(&F2, g2, 0, 15, b8, b5);
    cp_fold_rows(&F2, g2, 15, 40, b8, b5);
    cp_fold_rows(&F2, g2, 40, 42, b8, b5);
    CHECK(0 == memcmp(w8, b8, sizeof w8) && 0 == memcmp(w5, b5, sizeof w5),
          "fold banded == whole bit-exact");
    memset(b8, 0xAA, sizeof b8);
    cp_fold_rows(&F2, g2, 0, 42, NULL, b5); /* 565-only: 888 plane untouched */
    CHECK(0 == memcmp(w5, b5, sizeof w5) && 0xAA == b8[0] &&
              0xAA == b8[sizeof b8 - 1],
          "fold 565-only == both-sides 565");
    memset(b5, 0, sizeof b5);
    cp_fold_rows(&F2, g2, 0, 42, b8, NULL); /* 888-only */
    CHECK(0 == memcmp(w8, b8, sizeof w8), "fold 888-only == both-sides 888");
    cp_fold_free(&F2);

    /* naive absolute-coordinate reference, rot 90 AND 270 downscale — the
     * impl's staged strip re-bases coordinates per group; this walk uses the
     * source grid directly, so any offset/slip breaks the compare */
    static uint8_t g3[61 * 43];
    static uint8_t q8[29 * 41 * 3];
    static uint16_t q5[29 * 41];
    rng = 5150;
    for (int i = 0; i < 61 * 43; i++) {
        rng = rng * 6364136223846793005UL + 1442695040888963407UL;
        g3[i] = (uint8_t)(rng >> 33);
    }
    int naive_ok = 1;
    for (int ri = 1; ri <= 2 && naive_ok; ri++) {
        int rot = ri == 1 ? 90 : 270;
        struct cp_fold F3;
        if (!cp_fold_init(&F3, 61, 43, wb, CP_CCM_D65, lut, rot, 29, 41)) {
            naive_ok = 0;
            break;
        }
        cp_fold_rows(&F3, g3, 0, 41, q8, q5);
        for (uint32_t oy = 0; oy < 41 && naive_ok; oy++)
            for (uint32_t ox = 0; ox < 29 && naive_ok; ox++) {
                uint32_t xs0 = ox * 43 / 29,
                         xs1 = ((ox + 1) * 43 + 28) / 29;
                uint32_t ys0 = oy * 61 / 41,
                         ys1 = ((oy + 1) * 61 + 40) / 41;
                uint32_t sr = 0, sg = 0, sb = 0;
                for (uint32_t ry = ys0; ry < ys1; ry++) {
                    int a, b, c, q;
                    cp_rot_yscalar(&F3.R, ry, &a, &b, &c, &q);
                    for (uint32_t rx = xs0; rx < xs1; rx++) {
                        int Rv, Gv, Bv;
                        cp_px_lin(g3, 61, a, b, c, q,
                                  F3.R.cmap[rx], F3.R.cmap[43 + rx],
                                  F3.R.cmap[2 * 43 + rx], F3.R.cmap[3 * 43 + rx],
                                  &Rv, &Gv, &Bv);
                        sr += (uint32_t)Rv;
                        sg += (uint32_t)Gv;
                        sb += (uint32_t)Bv;
                    }
                }
                uint32_t cnt = (xs1 - xs0) * (ys1 - ys0);
                uint8_t e8[3];
                uint16_t e5;
                cp_apply_xform(&F3.R.xf, (int)((sr + cnt / 2) / cnt),
                               (int)((sg + cnt / 2) / cnt),
                               (int)((sb + cnt / 2) / cnt), e8, &e5);
                if (q8[(oy * 29 + ox) * 3] != e8[0] ||
                    q8[(oy * 29 + ox) * 3 + 1] != e8[1] ||
                    q8[(oy * 29 + ox) * 3 + 2] != e8[2] ||
                    q5[oy * 29 + ox] != e5)
                    naive_ok = 0;
            }
        cp_fold_free(&F3);
    }
    CHECK(naive_ok, "fold == naive absolute-coordinate reference (90/270)");

    /* domain-order delta vs the ⑤o two-stage chain. FLAT frame: every site
     * reconstructs to (c,c,c) so both xforms see identical inputs; the only
     * difference is the old path's 565 pack->unpack round-trip inside the
     * mean (<=7 R/B, <=3 G) — 565 comes back IDENTICAL (pack∘unpack = id
     * on 565 components). GRADIENT: averaging after gamma (old) vs before
     * (fold) differs by the LUT's Jensen gap plus quantization, bounded
     * here at 24 on a [32,255] ramp (structural breakage runs to 255). */
    static uint8_t gf[40 * 30];
    static uint16_t old5[30 * 40], pl[30 * 40], new5[30 * 40];
    static uint8_t old8[30 * 40 * 3], new8[30 * 40 * 3];
    static const int flats[4] = {16, 77, 200, 255};
    int flat5 = 1, flat8 = 1;
    for (int fi = 0; fi < 4; fi++) {
        memset(gf, (uint8_t)flats[fi], sizeof gf);
        struct cp_rot Ro;
        struct cp_box Bo;
        struct cp_fold Ff;
        cp_rot_init(&Ro, 40, 30, wb, CP_CCM_D65, lut, 90, 30, 40);
        cp_rot_rows(&Ro, gf, 0, 40, NULL, pl);
        cp_rot_free(&Ro);
        cp_box_init(&Bo, 30, 40, 20, 26);
        cp_box_rows(&Bo, pl, 0, 26, old5, old8);
        cp_box_free(&Bo);
        cp_fold_init(&Ff, 40, 30, wb, CP_CCM_D65, lut, 90, 20, 26);
        cp_fold_rows(&Ff, gf, 0, 26, new8, new5);
        cp_fold_free(&Ff);
        for (int i = 0; i < 20 * 26; i++) {
            if (old5[i] != new5[i])
                flat5 = 0;
            for (int c = 0; c < 3; c++)
                if (old8[i * 3 + c] - new8[i * 3 + c] > 8 ||
                    new8[i * 3 + c] - old8[i * 3 + c] > 8)
                    flat8 = 0;
        }
    }
    CHECK(flat5, "fold flat: 565 identical to two-stage");
    CHECK(flat8, "fold flat: 888 within 565 round-trip quant");
    for (uint32_t y = 0; y < 30; y++)
        for (uint32_t x = 0; x < 40; x++)
            gf[y * 40 + x] = (uint8_t)(32 + x * 223 / 39);
    {
        const float idw[3] = {1.0f, 1.0f, 1.0f};
        struct cp_rot Ro;
        struct cp_box Bo;
        struct cp_fold Ff;
        cp_rot_init(&Ro, 40, 30, idw, NULL, lut, 90, 30, 40);
        cp_rot_rows(&Ro, gf, 0, 40, NULL, pl);
        cp_rot_free(&Ro);
        cp_box_init(&Bo, 30, 40, 20, 26);
        cp_box_rows(&Bo, pl, 0, 26, old5, old8);
        cp_box_free(&Bo);
        cp_fold_init(&Ff, 40, 30, idw, NULL, lut, 90, 20, 26);
        cp_fold_rows(&Ff, gf, 0, 26, new8, new5);
        cp_fold_free(&Ff);
        int grad_ok = 1;
        for (int i = 0; i < 20 * 26; i++) {
            for (int c = 0; c < 3; c++)
                if (old8[i * 3 + c] - new8[i * 3 + c] > 24 ||
                    new8[i * 3 + c] - old8[i * 3 + c] > 24)
                    grad_ok = 0;
            uint32_t orv = (old5[i] >> 11) & 0x1F, ogv = (old5[i] >> 5) & 0x3F,
                     obv = old5[i] & 0x1F;
            uint32_t nrv = (new5[i] >> 11) & 0x1F, ngv = (new5[i] >> 5) & 0x3F,
                     nbv = new5[i] & 0x1F;
            int dr = (int)((orv << 3) | (orv >> 2)) - (int)((nrv << 3) | (nrv >> 2));
            int dg = (int)((ogv << 2) | (ogv >> 4)) - (int)((ngv << 2) | (ngv >> 4));
            int db = (int)((obv << 3) | (obv >> 2)) - (int)((nbv << 3) | (nbv >> 2));
            if (dr > 25 || -dr > 25 || dg > 25 || -dg > 25 || db > 25 || -db > 25)
                grad_ok = 0;
        }
        CHECK(grad_ok, "fold gradient: within domain-order + quant bound");
    }
}

int main(void)
{
    test_luts();
    test_extract();
    test_wb();
    test_debayer();
    test_crop();
    test_ccm();
    test_bayes();
    test_smooth();
    test_rot_split();
    test_tone();
    test_nr();
    test_noise();
    test_look();
    test_box();
    test_fold();
    if (fails) {
        printf("campix_test: %d FAIL\n", fails);
        return 1;
    }
    printf("campix_test: PASS (luts/extract/wb/debayer-rot-scale/crop/ccm/clip/ct-ccm/bayes/smooth/rot-split-blocked/tone/nr/look/box/fold)\n");
    return 0;
}
