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

    /* rot 0 identity scale: interior pixels reconstruct exactly */
    uint8_t out[8 * 8 * 3];
    cp_debayer_rot(g, 8, 8, w1, id, 0, 8, 8, out);
    int ok = 1;
    for (uint32_t y = 1; y < 7; y++)
        for (uint32_t x = 1; x < 7; x++) {
            const uint8_t *p = out + (y * 8 + x) * 3;
            if (p[0] != 100 || p[1] != 128 || p[2] != 160) ok = 0;
        }
    CHECK(ok, "debayer rot0 interior (100,128,160)");

    /* half scale, rot 0: output px -> src (2ox+1, 2oy+1), all B sites */
    uint8_t half[4 * 4 * 3];
    cp_debayer_rot(g, 8, 8, w1, id, 0, 4, 4, half);
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
    cp_debayer_rot(q, 8, 8, w1, id, 90, 8, 8, rot);
    ok = 1;
    for (uint32_t oy = 2; oy <= 6; oy += 2)
        for (uint32_t ox = 2; ox <= 6; ox += 2) {
            const uint8_t *p = rot + (oy * 8 + ox) * 3;
            if (p[0] != (uint8_t)(oy * 16 + (7 - ox))) ok = 0;
        }
    CHECK(ok, "rot90 CW maps out(ox,oy) to src(oy, 7-ox)");
    cp_debayer_rot(q, 8, 8, w1, id, 270, 8, 8, rot);
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
    cp_debayer_rot(wide, 8, 4, w1, id, 90, 4, 8, pr);
    CHECK(pr[0] == 0 && pr[(8 * 4 - 1) * 3] == 0, "rot dims no crash");

    /* WB gains multiply before the lut, saturating at 255 */
    uint8_t sat[8 * 8 * 3];
    const float w2[3] = {1.0f, 2.0f, 4.0f};
    cp_debayer_rot(g, 8, 8, w2, id, 0, 8, 8, sat);
    /* interior: G=128*2=256 -> 255; B=160*4=640 -> 255 */
    const uint8_t *p = sat + ((3 * 8 + 3) * 3);
    CHECK_EQ_I(p[0], 100, "wb+sat R");
    CHECK_EQ_I(p[1], 255, "wb+sat G clamps");
    CHECK_EQ_I(p[2], 255, "wb+sat B clamps");

    /* gamma LUT rides the tail: 100 -> ~167 at 2.2 (0.392^0.4545) */
    uint8_t gam[256];
    cp_lut_gamma(gam, 2.2);
    cp_debayer_rot(g, 8, 8, w1, gam, 0, 8, 8, out);
    p = out + ((3 * 8 + 3) * 3);
    CHECK(abs((int)p[0] - 167) <= 1, "gamma on R channel");
}

/* --- crop geometry (REAL viewfinder dims) --- */
static void test_crop(void)
{
    uint32_t x0, y0, cw, ch;
    /* 2016x1136 sensor, viewfinder 1080:1456 portrait OUT -> sensor-domain
     * aspect 1456:1080 -> 1531x1136 centered, then rot90 -> 1136x1531
     * (0.7420 vs 1080/1456 = 0.7418) */
    cp_crop_for_aspect(2016, 1136, 1456, 1080, &x0, &y0, &cw, &ch);
    CHECK_EQ_I(cw, 1531, "vf crop width");
    CHECK_EQ_I(ch, 1136, "vf crop height");
    CHECK_EQ_I(x0, 242, "vf crop centered x");
    CHECK_EQ_I(y0, 0, "vf crop y0");
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
    CHECK_EQ_I(y0, 5, "row-crop centered");
}

int main(void)
{
    test_luts();
    test_extract();
    test_wb();
    test_debayer();
    test_crop();
    if (fails) {
        printf("campix_test: %d FAIL\n", fails);
        return 1;
    }
    printf("campix_test: PASS (luts/extract/wb/debayer-rot-scale/crop)\n");
    return 0;
}
