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

/* --- WB + yavg --- */
static void test_wb(void)
{
    float wb[3];
    double y;
    uint8_t g[8 * 4];
    /* neutral uniform */
    memset(g, 128, sizeof g);
    cp_wb_measure(g, 8, 4, wb, &y);
    CHECK(wb[0] == 1.0f && wb[1] == 1.0f && wb[2] == 1.0f, "neutral gains 1");
    CHECK(fabs(y - 128.0) < 0.01, "neutral yavg 128");
    /* black frame: no gains, yavg 0 */
    memset(g, 0, sizeof g);
    cp_wb_measure(g, 8, 4, wb, &y);
    CHECK(wb[0] == 1.0f && wb[1] == 1.0f && wb[2] == 1.0f, "black gains 1");
    CHECK(y == 0.0, "black yavg 0");
    /* red-tinted: R sites 200, others 100 -> g,b lifted to 2x */
    memset(g, 100, sizeof g);
    for (uint32_t yy = 0; yy < 4; yy += 2)
        for (uint32_t xx = 0; xx < 8; xx += 2)
            g[yy * 8 + xx] = 200;
    cp_wb_measure(g, 8, 4, wb, &y);
    CHECK(fabsf(wb[0] - 1.0f) < 1e-4, "tinted r gain 1");
    CHECK(fabsf(wb[1] - 2.0f) < 1e-4, "tinted g gain 2");
    CHECK(fabsf(wb[2] - 2.0f) < 1e-4, "tinted b gain 2");
    /* 0.299*200 + 0.587*100 + 0.114*100 = 129.9 */
    CHECK(fabs(y - 129.9) < 0.05, "tinted yavg 129.9");
    /* extreme tint caps at 4x, never darkens */
    memset(g, 10, sizeof g);
    for (uint32_t yy = 0; yy < 4; yy += 2)
        for (uint32_t xx = 0; xx < 8; xx += 2)
            g[yy * 8 + xx] = 250;
    cp_wb_measure(g, 8, 4, wb, &y);
    CHECK(fabsf(wb[0] - 1.0f) < 1e-4, "extreme r gain 1");
    CHECK(fabsf(wb[1] - 4.0f) < 1e-4, "extreme g gain capped 4");
    CHECK(fabsf(wb[2] - 4.0f) < 1e-4, "extreme b gain capped 4");

    /* M47⑤h emissive soft-weight: top quad-row a clipped "screen" (all
     * 250), bottom the lit pattern R 80 / G 108 / B 53 — the white-cable
     * sensor ratios from the device (2026-09-05). Emissive quads carry
     * 0.5 weight, so the gains land BETWEEN the lit point (1.35/2.038
     * hard-exclusion) and the all-in gray point (1.085/1.182):
     * mr 193.3 / mg 202.7 / mb 184.3 -> (1.048, 1.0, 1.099). yavg stays
     * whole-frame (mr 165 / mg 179 / mb 151.5 -> 171.7). */
    for (uint32_t yy = 0; yy < 4; yy++)
        for (uint32_t xx = 0; xx < 8; xx++) {
            int site = !(yy & 1) ? (!(xx & 1) ? 0 : 1) : (!(xx & 1) ? 1 : 2);
            g[yy * 8 + xx] = (uint8_t)(yy < 2 ? 250
                : site == 0 ? 80 : site == 2 ? 53 : 108);
        }
    cp_wb_measure(g, 8, 4, wb, &y);
    CHECK(fabsf(wb[0] - 1.0483f) < 0.01, "emissive soft-weight: r blends toward lit");
    CHECK(fabsf(wb[1] - 1.0f) < 1e-4, "emissive soft-weight: g gain 1");
    CHECK(fabsf(wb[2] - 1.0994f) < 0.01, "emissive soft-weight: b blends toward lit");
    CHECK(wb[0] < 1.085f && wb[2] < 1.182f, "emissive soft-weight beats all-in green");
    CHECK(fabs(y - 171.7) < 0.1, "yavg stays whole-frame");
    /* all-emissive frame (white wall): neutral white -> gains 1 regardless
     * of weighting */
    memset(g, 250, sizeof g);
    cp_wb_measure(g, 8, 4, wb, &y);
    CHECK(wb[0] == 1.0f && wb[1] == 1.0f && wb[2] == 1.0f, "all-emissive stays neutral");
    CHECK(fabs(y - 250.0) < 0.01, "all-emissive yavg 250");

    CHECK(fabs(cp_yavg((uint8_t[4]){10, 10, 10, 10}, 4) - 10.0) < 0.01,
          "cp_yavg flat");
    CHECK(cp_yavg(g, 0) == 0.0, "cp_yavg empty");
}

/* --- debayer / rotate / scale --- */

/* 8x8 with per-site constants: R(even,even)=100, G=128, B(odd,odd)=160.
 * Every interior reconstruction is exactly (100,128,160) regardless of
 * site — the site scheme cancels out. */
static void site_const(uint8_t *g, uint32_t w, uint32_t h)
{
    for (uint32_t y = 0; y < h; y++)
        for (uint32_t x = 0; x < w; x++) {
            int site = !(y & 1) ? (!(x & 1) ? 0 : 1) : (!(x & 1) ? 1 : 2);
            g[y * w + x] = (uint8_t)(site == 0 ? 100 : site == 2 ? 160 : 128);
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

    /* half scale, rot 0: output px -> src (2ox+1, 2oy+1), all B sites */
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
     * neighbor PAIR — the gradient makes the pair's average exact:
     *   rot90 CW:  out(ox,oy) samples src(oy, 7-ox); oy even -> x even,
     *              7-ox odd -> G-on-B-row, R = (u+d)/2 = oy*16+(7-ox)
     *   rot270 CCW: out(ox,oy) samples src(7-oy, ox); 7-oy odd -> x odd,
     *              ox even -> G-on-R-row, R = (l+r)/2 = (7-oy)*16+ox
     * Positions with ox,oy in {2,4,6} keep both pair members in bounds. */
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

    /* warmth pick: neutral wb -> D65; b-gain 4 (tungsten) -> INC; 1.4 ->
     * the D65/TL84 midpoint */
    float m[9];
    cp_ccm_for_wb((const float[3]){1, 1, 1}, m);
    CHECK(0 == memcmp(m, CP_CCM_D65, sizeof m), "warmth 1 -> D65");
    cp_ccm_for_wb((const float[3]){1, 1, 4}, m);
    CHECK(0 == memcmp(m, CP_CCM_INC, sizeof m), "warmth 4 -> INC");
    cp_ccm_for_wb((const float[3]){1, 1, 1.4f}, m);
    int mid = 1;
    for (int i = 0; i < 9; i++) {
        float want = 0.5f * (CP_CCM_D65[i] + CP_CCM_TL84[i]);
        if (fabsf(m[i] - want) > 1e-4) mid = 0;
    }
    CHECK(mid, "warmth 1.4 -> D65/TL84 midpoint");

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
     * wb (1,2,4) on (200,220,240) -> CCM out (0, 373, 1268) -> s=65280/1268
     * =51 -> (0*51, 373*51, 1268*51 each +128 >>8) = (0, 74, 253), hand-worked
     * against the same Q14 quantization the matrix init does */
    cp_xform_init(&t, (const float[3]){1.0f, 2.0f, 4.0f}, CP_CCM_D65, id);
    uint8_t o3[3];
    cp_apply_xform(&t, 200, 220, 240, o3, NULL);
    CHECK_EQ_I(o3[0], 0, "hot px R");
    CHECK_EQ_I(o3[1], 74, "hot px G (desat table)");
    CHECK_EQ_I(o3[2], 253, "hot px B (desat pins near max)");
}

int main(void)
{
    test_luts();
    test_extract();
    test_wb();
    test_debayer();
    test_crop();
    test_ccm();
    test_rot_split();
    if (fails) {
        printf("campix_test: %d FAIL\n", fails);
        return 1;
    }
    printf("campix_test: PASS (luts/extract/wb/debayer-rot-scale/crop/ccm/rot-split-blocked)\n");
    return 0;
}
