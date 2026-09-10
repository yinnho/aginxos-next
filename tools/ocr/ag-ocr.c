// ag-ocr — M45 本地 OCR CLI（PP-OCR det+rec 骑裸 ORT C API，单文件 C）。
// 用法: ag-ocr [options] <jpg> [--json]
//       --rec-only <jpg>     调试：跳过 det，整图当一行过 rec
//                             （只对手工裁出的文字行图有意义）
//       --rot auto|0|90|180|270   旋转后再识别（缺省 auto）
// 出:   默认每行 "text\tconf"（tab 分隔）；--json 出 {"lines":[...]}；
//       计时行打 stderr。exit 0=识别出文字 / 1=没字 / 2=错误（同 agqr 约定）。
// 模型: AG_OCR_DIR（缺省 /var/models/ocr）下 det.onnx rec.onnx dict.txt。
//
// 旋转：cam-shot 传感器横向安装，手机竖握拍出的图里文字转了 90°（M45 实拍
// 收据：det 照样出框、rec 全灭）。--rot auto 先按 90 试（竖握是产品常态），
// 横排 kept>=2 行即收；否则 0/270/180 逐个试，择优主键 = 横排行数 hkept
//（quad 裁剪会把竖排文字转正读出，但那是页面被转 90° 的假成功——行序是
// 列序）。box 坐标是旋转后图的坐标系。
//
// 管线常数逐条对齐 RapidOCR v3.9.2（ch_ppocr_det / ch_ppocr_rec 源码核对，
// 2026-09-04 提取；模型 PP-OCRv5 mobile，ModelScope RapidAI/RapidOCR）：
//   - 入网 BGR（cv2 惯例；stb 出 RGB，这里换序）
//   - 归一化 (x/255 - 0.5) / 0.5，CHW float32
//   - 全局预缩放：最长边 > 2000 封顶（Global.max_side_len）
//   - det：限边 736 / min 型（短边 <736 才放大）；先 int() 截断再 round 到
//     32 的倍数；prob>0.3 二值 → 2×2 膨胀（config use_dilation=true）→
//     连通域；box_thresh 0.5；unclip_ratio 1.6（dist=area*1.6/perimeter）；
//     min_size 3 / unclip 后 5 / 映射回原图后 w,h>3；行分组 dy>=10px
//   - rec：h=48，动态宽 imgw=int(48*max(320/48, w/h))，右零填；
//     CTC：id0=blank、1..N=dict 行、N+1=space（先 append space 再插 blank）
//   - 行置信度 < 0.5 丢（Global.text_score）
// 框几何（S2，上游 DBPostProcess 对齐）：连通域���界点 → Andrew 凸包 →
// minAreaRect（枚举 hull 边方向）→ unclip 于矩形（pyclipper JT_ROUND 于
// 矩形 ≡ 同中心同角度 (w+2d,h+2d) 外扩，上游 unclip 输入本就是 minAreaRect
// 四点矩形，等价裁决 2026-09-10）→ 四角映射回工作图 → rec 透视裁剪
// （Heckbert 方→四边形有理映射 + 钳位双线性；平行四边形自然退化为仿射）。
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>

#ifdef __ANDROID__
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <sched.h>
#endif

#define STB_IMAGE_IMPLEMENTATION
#include "stb_image.h"

#include "onnxruntime_c_api.h"

// ---- RapidOCR v3.9.2 管线常数（见文件头） ----
#define PRE_MAX_SIDE   2000
#define DET_LIMIT_LEN  736
#define DET_BIN_THRESH 0.3f
#define DET_BOX_THRESH 0.5f
#define DET_UNCLIP     1.6f
#define REC_H          48
#define REC_W          320
#define TEXT_SCORE     0.5f
#define SORT_Y_LINE    10.0f

static const OrtApi *ort;

static void die(const char *msg) {
    fprintf(stderr, "ag-ocr: %s\n", msg);
    exit(2);
}
static void check(OrtStatus *st, const char *what) {
    if (st) {
        fprintf(stderr, "ag-ocr: %s: %s\n", what, ort->GetErrorMessage(st));
        exit(2);
    }
}
static double now_ms(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (double)tv.tv_sec * 1000.0 + (double)tv.tv_usec / 1000.0;
}

// ---- ORT 会话包装 ----
typedef struct {
    OrtSession *s;
    char *in_name, *out_name; // allocator 拥有，进程退出统一释放可不管
} Sess;

static void sess_open(Sess *se, OrtEnv *env, const char *path) {
    OrtSessionOptions *opt;
    check(ort->CreateSessionOptions(&opt), "session options");
    ort->SetIntraOpNumThreads(opt, 2);
    ort->SetSessionLogSeverityLevel(opt, 3); // ORT_LOGGING_LEVEL_ERROR
    check(ort->CreateSession(env, path, opt, &se->s), path);
    ort->ReleaseSessionOptions(opt);

    OrtAllocator *alloc = NULL;
    check(ort->GetAllocatorWithDefaultOptions(&alloc), "allocator");
    check(ort->SessionGetInputName(se->s, 0, alloc, &se->in_name), "input name");
    check(ort->SessionGetOutputName(se->s, 0, alloc, &se->out_name),
          "output name");
}

typedef struct {
    OrtValue *val;
    float *data;
    int64_t dims[8];
    int ndim;
} Out;

// 注意：Run 的 output 槽是 _Inout_ 不是 _Out_ —— 非 NULL 会被当成调用者
// 预分配的 OrtValue 直接写入（原子引用计数炸垃圾指针）。必须清零后传入。
static void sess_run(Sess *se, const float *in, const int64_t *shape, int rank,
                     Out *out) {
    memset(out, 0, sizeof *out);
    OrtMemoryInfo *mi;
    check(ort->CreateCpuMemoryInfo(OrtDeviceAllocator, OrtMemTypeDefault, &mi),
          "memory info");
    size_t n = 1;
    for (int i = 0; i < rank; i++) n *= (size_t)shape[i];
    OrtValue *iv = NULL;
    check(ort->CreateTensorWithDataAsOrtValue(
              mi, (void *)in, n * sizeof(float), shape, rank,
              ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT, &iv),
          "input tensor");
    const char *ins[1] = {se->in_name}, *outs[1] = {se->out_name};
    check(ort->Run(se->s, NULL, ins, &iv, 1, outs, 1, &out->val), "run");
    ort->ReleaseValue(iv);
    ort->ReleaseMemoryInfo(mi);

    OrtTensorTypeAndShapeInfo *ti;
    check(ort->GetTensorTypeAndShape(out->val, &ti), "output shape");
    size_t nd = 0;
    check(ort->GetDimensionsCount(ti, &nd), "output rank");
    if (nd > 8) die("output rank too high");
    out->ndim = (int)nd;
    check(ort->GetDimensions(ti, out->dims, nd), "output dims");
    check(ort->GetTensorMutableData(out->val, (void **)&out->data),
          "output data");
    ort->ReleaseTensorTypeAndShapeInfo(ti);
}

// ---- 图像：RGB u8，3 字节/像素，行主序 ----
typedef struct {
    int w, h;
    uint8_t *px;
} Img;

static void *xmalloc(size_t n) {
    void *p = malloc(n);
    if (!p) die("out of memory");
    return p;
}

// 双线性，采样中心对齐 ((d+0.5)*s-0.5)，同 cv2 INTER_LINEAR。
static Img img_resize(const Img *src, int dw, int dh) {
    Img d;
    d.w = dw;
    d.h = dh;
    d.px = (uint8_t *)xmalloc((size_t)dw * dh * 3);
    float sx = (float)src->w / dw, sy = (float)src->h / dh;
    for (int y = 0; y < dh; y++) {
        float fy = (y + 0.5f) * sy - 0.5f;
        int y0 = (int)floorf(fy), y1 = y0 + 1;
        float wy = fy - y0;
        if (y0 < 0) { y0 = 0; }
        if (y1 >= src->h) { y1 = src->h - 1; }
        if (y0 > y1) { y0 = y1; }
        for (int x = 0; x < dw; x++) {
            float fx = (x + 0.5f) * sx - 0.5f;
            int x0 = (int)floorf(fx), x1 = x0 + 1;
            float wx = fx - x0;
            if (x0 < 0) { x0 = 0; }
            if (x1 >= src->w) { x1 = src->w - 1; }
            if (x0 > x1) { x0 = x1; }
            const uint8_t *a = src->px + ((size_t)y0 * src->w + x0) * 3;
            const uint8_t *b = src->px + ((size_t)y0 * src->w + x1) * 3;
            const uint8_t *c = src->px + ((size_t)y1 * src->w + x0) * 3;
            const uint8_t *e = src->px + ((size_t)y1 * src->w + x1) * 3;
            uint8_t *o = d.px + ((size_t)y * dw + x) * 3;
            for (int ch = 0; ch < 3; ch++) {
                float v = (1 - wx) * (1 - wy) * a[ch] + wx * (1 - wy) * b[ch] +
                          (1 - wx) * wy * c[ch] + wx * wy * e[ch];
                int iv = (int)(v + 0.5f);
                o[ch] = (uint8_t)(iv < 0 ? 0 : (iv > 255 ? 255 : iv));
            }
        }
    }
    return d;
}

// RGB u8 → CHW float，通道序 BGR（PP-OCR cv2 惯例）。
static float *norm_chw_bgr(const uint8_t *rgb, int w, int h) {
    size_t pl = (size_t)w * h;
    float *t = (float *)xmalloc(pl * 3 * sizeof(float));
    for (size_t i = 0; i < pl; i++) {
        t[i] = (rgb[i * 3 + 2] / 255.0f - 0.5f) / 0.5f;
        t[pl + i] = (rgb[i * 3 + 1] / 255.0f - 0.5f) / 0.5f;
        t[2 * pl + i] = (rgb[i * 3] / 255.0f - 0.5f) / 0.5f;
    }
    return t;
}

// 90° 旋转（dir=1 顺时针 / -1 逆时针）与 180° 翻转。像素级拷贝，无插值。
static Img img_rot90(const Img *src, int dir) {
    Img d;
    d.w = src->h;
    d.h = src->w;
    d.px = (uint8_t *)xmalloc((size_t)d.w * d.h * 3);
    for (int y = 0; y < src->h; y++) {
        for (int x = 0; x < src->w; x++) {
            int nx, ny; // 顺时针: (x,y)->(h-1-y, x)；逆时针: (x,y)->(y, w-1-x)
            if (dir > 0) { nx = src->h - 1 - y; ny = x; }
            else         { nx = y;              ny = src->w - 1 - x; }
            memcpy(d.px + ((size_t)ny * d.w + nx) * 3,
                   src->px + ((size_t)y * src->w + x) * 3, 3);
        }
    }
    return d;
}
static Img img_rot180(const Img *src) {
    Img d;
    d.w = src->w;
    d.h = src->h;
    d.px = (uint8_t *)xmalloc((size_t)d.w * d.h * 3);
    for (int y = 0; y < src->h; y++)
        for (int x = 0; x < src->w; x++)
            memcpy(d.px + ((size_t)(src->h - 1 - y) * d.w + (src->w - 1 - x)) * 3,
                   src->px + ((size_t)y * src->w + x) * 3, 3);
    return d;
}

// ---- det ----

// 限边 736 / min 型：短边 <736 放大；int() 截断后 round 到 32 倍（上游顺序）。
static Img det_resize(const Img *src) {
    int h = src->h, w = src->w;
    int m = h < w ? h : w;
    float ratio = m < DET_LIMIT_LEN ? (float)DET_LIMIT_LEN / m : 1.0f;
    int rh = (int)(h * ratio), rw = (int)(w * ratio);
    rh = (int)(lroundf(rh / 32.0f) * 32);
    rw = (int)(lroundf(rw / 32.0f) * 32);
    if (rh < 32 || rw < 32) die("det resize collapsed");
    return img_resize(src, rw, rh);
}

typedef struct {
    int x0, y0, x1, y1; // AABB（含端点）：排序/行分组/JSON 出口
    float qx[4], qy[4]; // 四角 quad（工作图坐标 tl,tr,br,bl）：rec 裁剪入口
    int idx;            // 稳定排序用
    int line;           // 行分组结果
} Box;

typedef struct {
    int x0, y0, x1, y1;
    long cnt;
    double sum;
} Comp;

static int cmp_box_y(const void *a, const void *b) {
    const Box *p = (const Box *)a, *q = (const Box *)b;
    if (p->y0 != q->y0) return p->y0 < q->y0 ? -1 : 1;
    if (p->x0 != q->x0) return p->x0 < q->x0 ? -1 : 1;
    return p->idx - q->idx;
}
static int cmp_box_line(const void *a, const void *b) {
    const Box *p = (const Box *)a, *q = (const Box *)b;
    if (p->line != q->line) return p->line - q->line;
    if (p->x0 != q->x0) return p->x0 < q->x0 ? -1 : 1;
    return p->idx - q->idx;
}

// ---- quad 几何（S2，上游 DBPostProcess 对齐） ----

typedef struct { float x, y; } P2;

static int cmp_p2(const void *a, const void *b) {
    const P2 *p = (const P2 *)a, *q = (const P2 *)b;
    if (p->x != q->x) return p->x < q->x ? -1 : 1;
    if (p->y != q->y) return p->y < q->y ? -1 : 1;
    return 0;
}

// Andrew 单调链凸包（去共线）。返回 hull 点数。
static int convex_hull(P2 *p, int n, P2 *h) {
    qsort(p, (size_t)n, sizeof(P2), cmp_p2);
    int k = 0;
    for (int i = 0; i < n; i++) { // 下链：cross<=0 弹栈
        while (k >= 2 &&
               (h[k-1].x - h[k-2].x) * (p[i].y - h[k-2].y) -
               (h[k-1].y - h[k-2].y) * (p[i].x - h[k-2].x) <= 0) k--;
        h[k++] = p[i];
    }
    for (int i = n - 2, t = k + 1; i >= 0; i--) { // 上链
        while (k >= t &&
               (h[k-1].x - h[k-2].x) * (p[i].y - h[k-2].y) -
               (h[k-1].y - h[k-2].y) * (p[i].x - h[k-2].x) <= 0) k--;
        h[k++] = p[i];
    }
    k--; // 首点在尾重复
    return k;
}

// minAreaRect：枚举 hull 每条边方向为候选轴（O(h²)），投影取面积最小。
// 长边归一为 u 轴（文本行方向）；u 规约到 +x 半平面（垂直文本取 +y）——
// 裁剪读向稳定不镜像。
static void min_area_rect(const P2 *h, int n, float *cx, float *cy,
                          float *ux, float *uy, float *rw, float *rh) {
    float besta = 1e30f;
    for (int i = 0; i < n; i++) {
        float dx = h[(i + 1) % n].x - h[i].x, dy = h[(i + 1) % n].y - h[i].y;
        float len = hypotf(dx, dy);
        if (len < 1e-6f) continue;
        float ax = dx / len, ay = dy / len;
        float umin = 1e30f, umax = -1e30f, vmin = 1e30f, vmax = -1e30f;
        for (int j = 0; j < n; j++) {
            float pu = h[j].x * ax + h[j].y * ay;
            float pv = -h[j].x * ay + h[j].y * ax;
            if (pu < umin) umin = pu;
            if (pu > umax) umax = pu;
            if (pv < vmin) vmin = pv;
            if (pv > vmax) vmax = pv;
        }
        float w = umax - umin, v = vmax - vmin;
        float area = w * v;
        if (area < besta) {
            besta = area;
            float cu = (umin + umax) / 2, cv = (vmin + vmax) / 2;
            *cx = cu * ax - cv * ay; // 中心在原 (ax,ay) 基下换算，物理点不变
            *cy = cu * ay + cv * ax;
            if (v > w) { *ux = -ay; *uy = ax; *rw = v; *rh = w; } // 长边为 u
            else       { *ux = ax;  *uy = ay; *rw = w; *rh = v; }
        }
    }
    if (*ux < 0 || (*ux == 0 && *uy < 0)) { *ux = -*ux; *uy = -*uy; }
}

// Heckbert 方→四边形裁剪：unit square (s,t)→quad 四角(tl,tr,br,bl) 的
// 有理映射（分母系数 G,H 由 (1,1) 角点两方程直接解出，免矩阵求逆）。
// dst 网格逐点正向映射回源坐标 + 钳位双线性（同 img_resize 内核纪律）。
// 平行四边形（正拍、pred→work 各向异性映射后的矩形）分母项为零，自然
// 退化为仿射——单一入口两种形状；角点被图界裁过的才是真透视四边形。
static Img crop_quad(const Img *src, const float *qx, const float *qy, int dw,
                     int dh) {
    Img d;
    d.w = dw;
    d.h = dh;
    d.px = (uint8_t *)xmalloc((size_t)dw * dh * 3);
    float skx = qx[0] - qx[1] + qx[2] - qx[3];
    float sky = qy[0] - qy[1] + qy[2] - qy[3];
    float A, B, C, D, E, F, G, H;
    C = qx[0];
    F = qy[0];
    if (fabsf(skx) < 1e-6f && fabsf(sky) < 1e-6f) {
        G = H = 0;
        A = qx[1] - qx[0];
        B = qx[3] - qx[0];
        D = qy[1] - qy[0];
        E = qy[3] - qy[0];
    } else {
        float dx1 = qx[1] - qx[2], dx2 = qx[3] - qx[2];
        float dy1 = qy[1] - qy[2], dy2 = qy[3] - qy[2];
        float det = dx1 * dy2 - dx2 * dy1;
        if (fabsf(det) < 1e-9f) { // 近退化四边形：仿射兜底
            G = H = 0;
            A = qx[1] - qx[0];
            B = qx[3] - qx[0];
            D = qy[1] - qy[0];
            E = qy[3] - qy[0];
        } else {
            G = (skx * dy2 - dx2 * sky) / det;
            H = (dx1 * sky - skx * dy1) / det;
            A = qx[1] * (1 + G) - qx[0];
            B = qx[3] * (1 + H) - qx[0];
            D = qy[1] * (1 + G) - qy[0];
            E = qy[3] * (1 + H) - qy[0];
        }
    }
    for (int j = 0; j < dh; j++) {
        float t = (j + 0.5f) / dh;
        for (int i = 0; i < dw; i++) {
            float s = (i + 0.5f) / dw;
            float den = G * s + H * t + 1;
            float x = (A * s + B * t + C) / den;
            float y = (D * s + E * t + F) / den;
            int x0 = (int)floorf(x), x1 = x0 + 1;
            int y0 = (int)floorf(y), y1 = y0 + 1;
            float wx = x - x0, wy = y - y0;
            if (x0 < 0) x0 = 0;
            if (x1 >= src->w) x1 = src->w - 1;
            if (x0 > x1) x0 = x1;
            if (y0 < 0) y0 = 0;
            if (y1 >= src->h) y1 = src->h - 1;
            if (y0 > y1) y0 = y1;
            const uint8_t *a = src->px + ((size_t)y0 * src->w + x0) * 3;
            const uint8_t *b = src->px + ((size_t)y0 * src->w + x1) * 3;
            const uint8_t *c = src->px + ((size_t)y1 * src->w + x0) * 3;
            const uint8_t *e = src->px + ((size_t)y1 * src->w + x1) * 3;
            uint8_t *o = d.px + ((size_t)j * dw + i) * 3;
            for (int ch = 0; ch < 3; ch++) {
                float vv = (1 - wx) * (1 - wy) * a[ch] + wx * (1 - wy) * b[ch] +
                           (1 - wx) * wy * c[ch] + wx * wy * e[ch];
                int iv = (int)(vv + 0.5f);
                o[ch] = (uint8_t)(iv < 0 ? 0 : (iv > 255 ? 255 : iv));
            }
        }
    }
    return d;
}

// ---- 栏序（S3，单沟 XY-cut 深度 1） ----
// 双栏页（论文/摘要常态）：行分组只认 y，左右栏同 y 段交错成伪行。
// 占位计数找沟：每 x 列数被几个 box 的 AABB 盖住，沟 = 内部「覆盖 ≤1」
// （至多被通栏元素盖住）的最宽 run，且左右邻列覆盖 ≥2——页边距只有单
// 侧内容，天然出局。纯区间合并法的死穴：居中窄标题把真沟和右栏焊成
// 一块，最宽内沟退化成左栏~标题间的细缝；计数法下标题盖住的沟段覆盖
// 仍为 1，真沟现形。门全过才切：沟宽 ≥ max(1.5×中位行高, 2%页宽) 且
// 两侧各 ≥2 个 box。
// 分类按 box（不按行组：左右行 y 对齐时同组，组级门必死）；输出序 =
// 跨沟 box（标题，按 y）→ 左栏 → 右栏，各段内重跑 dy>=10 分行。
// 无沟/门不过 = 原序不动。深度 1 已知降级：页中通栏块（图注/表格）被
// 当标题提前；沟被 ≥2 个通栏元素盖住、或斜拍 AABB 挤占沟 → 不切（原
// 序）。3 栏页降级一次二分。stdout 行序即读序。

static int cmp_int(const void *a, const void *b) {
    return *(const int *)a - *(const int *)b;
}

// 段内重分行（同 det_post 主律：y 稳定排 + dy>=10 分行 + 行内 x 排）
static void regroup(Box *b, int n) {
    qsort(b, (size_t)n, sizeof(Box), cmp_box_y);
    float prev_y = -1e9f;
    int line = 0;
    for (int i = 0; i < n; i++) {
        if (prev_y > -1e8f && (float)b[i].y0 - prev_y >= SORT_Y_LINE) line++;
        b[i].line = line;
        prev_y = (float)b[i].y0;
    }
    qsort(b, (size_t)n, sizeof(Box), cmp_box_line);
}

static void order_columns(Box *boxes, int n, int W) {
    if (n < 4) return; // 两侧各 ≥2 box 是硬门，n<4 必不过
    enum { MAXN = 1024 };

    // 中位行高（box 高中位数）
    int hs[MAXN];
    for (int i = 0; i < n; i++) hs[i] = boxes[i].y1 - boxes[i].y0 + 1;
    qsort(hs, (size_t)n, sizeof(int), cmp_int);
    int medh = hs[n / 2];

    // x 列占位计数（AABB 覆盖；x1 由 quad 钳位 ≤ W-1）
    int *cov = (int *)calloc((size_t)W, sizeof(int));
    if (!cov) die("out of memory");
    for (int i = 0; i < n; i++)
        for (int x = boxes[i].x0; x <= boxes[i].x1; x++) cov[x]++;

    // 内部低覆盖 run（cov<=1；两端邻列 cov>=2 才算沟——页边距出局）
    int best_gw = 0, g0 = 0, g1 = -1, x = 0;
    while (x < W) {
        if (cov[x] > 1) { x++; continue; }
        int s = x;
        while (x < W && cov[x] <= 1) x++;
        if (s > 0 && x < W && cov[s - 1] >= 2 && cov[x] >= 2) {
            int gw = x - s; // run [s, x)
            if (gw > best_gw) { best_gw = gw; g0 = s; g1 = x - 1; }
        }
    }
    free(cov);
    if (best_gw <= 0) return;
    if (best_gw < medh * 3 / 2 || best_gw < W / 50) return; // 沟宽门：max(1.5×行高, 2%页宽)

    // 按 box 分类：全左 / 全右 / 跨沟
    Box span[MAXN], left[MAXN], right[MAXN], out[MAXN];
    int ns = 0, nl = 0, nr = 0;
    for (int i = 0; i < n; i++) {
        if (boxes[i].x1 < g0) left[nl++] = boxes[i];
        else if (boxes[i].x0 > g1) right[nr++] = boxes[i];
        else span[ns++] = boxes[i];
    }
    if (nl < 2 || nr < 2) return; // 两侧行门

    // 段内各自行序，拼回：跨沟 → 左 → 右。line 以段基址平移重编
    //（regroup 后各段 line 从 0 起），idx 按最终序重排。
    regroup(span, ns);
    regroup(left, nl);
    regroup(right, nr);
    Box *segs[3] = {span, left, right};
    int cnts[3] = {ns, nl, nr};
    int m = 0, base = 0;
    for (int s = 0; s < 3; s++) {
        int last = -1;
        for (int i = 0; i < cnts[s]; i++) {
            segs[s][i].line = base + segs[s][i].line;
            last = segs[s][i].line;
            out[m++] = segs[s][i];
        }
        base = last + 1;
    }
    for (int i = 0; i < m; i++) out[i].idx = i;
    memcpy(boxes, out, (size_t)m * sizeof(Box));
}

// DB 后处理：二值→膨胀→连通域→过滤→unclip→映射回原图→排序+栏序。
// 返回 box 数（写入 boxes[]，容量 cap）；pred 为 det 输出图（pw×ph）。
static int det_post(const float *pred, int pw, int ph, int W, int H,
                    Box *boxes, int cap) {
    size_t np = (size_t)pw * ph;
    uint8_t *bin = (uint8_t *)xmalloc(np), *dil = (uint8_t *)xmalloc(np);
    for (size_t i = 0; i < np; i++) bin[i] = pred[i] > DET_BIN_THRESH ? 1 : 0;

    // 2×2 膨胀，anchor(1,1)：out(x,y)=max(bin[x-1..x][y-1..y])
    for (int y = 0; y < ph; y++) {
        for (int x = 0; x < pw; x++) {
            int v = bin[(size_t)y * pw + x];
            if (!v && x > 0) v = bin[(size_t)y * pw + x - 1];
            if (!v && y > 0) v = bin[(size_t)(y - 1) * pw + x];
            if (!v && x > 0 && y > 0) v = bin[(size_t)(y - 1) * pw + x - 1];
            dil[(size_t)y * pw + x] = (uint8_t)v;
        }
    }

    // 连通域（8 邻接，BFS），栈容量 = 像素总数上界
    int32_t *lab = (int32_t *)xmalloc(np * sizeof(int32_t));
    memset(lab, 0, np * sizeof(int32_t));
    int32_t *stack = (int32_t *)xmalloc(np * sizeof(int32_t));
    // 边界点缓冲（凸包输入，O 周长）与凸包工作区：按需翻倍
    int bcap = 4096;
    P2 *pts = (P2 *)xmalloc((size_t)bcap * sizeof(P2));
    P2 *hull = (P2 *)xmalloc((size_t)bcap * sizeof(P2));
    int nbp = 0;
    int nbox = 0;
    for (int sy = 0; sy < ph; sy++) {
        for (int sx = 0; sx < pw; sx++) {
            size_t seed = (size_t)sy * pw + sx;
            if (!dil[seed] || lab[seed]) continue;
            int sp = 0;
            stack[sp++] = (int32_t)seed;
            lab[seed] = 1;
            Comp c;
            memset(&c, 0, sizeof c);
            c.x0 = c.x1 = sx;
            c.y0 = c.y1 = sy;
            nbp = 0;
            while (sp > 0) {
                int32_t p = stack[--sp];
                int px = p % pw, py = p / pw;
                c.cnt++;
                c.sum += pred[p];
                if (px < c.x0) c.x0 = px;
                if (px > c.x1) c.x1 = px;
                if (py < c.y0) c.y0 = py;
                if (py > c.y1) c.y1 = py;
                // 边界点：4 邻任一在 mask 外/图外 → 凸包候选（短路保证
                // 不越界：前四条任一真即跳过 dil 寻址，全假则坐标全在界内）
                if (px == 0 || py == 0 || px + 1 == pw || py + 1 == ph ||
                    !dil[(size_t)py * pw + px - 1] ||
                    !dil[(size_t)(py + 1) * pw + px] ||
                    !dil[(size_t)py * pw + px + 1] ||
                    !dil[(size_t)(py - 1) * pw + px]) {
                    if (nbp >= bcap) {
                        bcap *= 2;
                        pts = (P2 *)realloc(pts, (size_t)bcap * sizeof(P2));
                        hull = (P2 *)realloc(hull, (size_t)bcap * sizeof(P2));
                        if (!pts || !hull) die("out of memory");
                    }
                    pts[nbp].x = (float)px;
                    pts[nbp].y = (float)py;
                    nbp++;
                }
                for (int dy = -1; dy <= 1; dy++) {
                    for (int dx = -1; dx <= 1; dx++) {
                        int nx = px + dx, ny = py + dy;
                        if (nx < 0 || ny < 0 || nx >= pw || ny >= ph)
                            continue;
                        size_t q = (size_t)ny * pw + nx;
                        if (dil[q] && !lab[q]) {
                            lab[q] = 1;
                            stack[sp++] = (int32_t)q;
                        }
                    }
                }
            }

            int bw = c.x1 - c.x0 + 1, bh = c.y1 - c.y0 + 1;
            if (bw < 3 || bh < 3) continue; // min_size（廉价前置：CC bbox）
            double score = c.sum / (double)c.cnt;
            if (score < DET_BOX_THRESH) continue;

            // 凸包 → minAreaRect（上游 get_mini_boxes）
            int nh = convex_hull(pts, nbp, hull);
            float ccx, ccy, ux, uy, rw, rh;
            min_area_rect(hull, nh, &ccx, &ccy, &ux, &uy, &rw, &rh);
            if (rw < 3 || rh < 3) continue; // min_size（矩形短边）

            // unclip：dist = w*h*1.6/(2*(w+h)) 于 minAreaRect——上游 unclip
            // 作用的正是该矩形，JT_ROUND ≡ 同中心同角度 (w+2d, h+2d) 外扩
            float dist = rw * rh * DET_UNCLIP / (2.0f * (rw + rh));
            float w2 = rw + 2 * dist, h2 = rh + 2 * dist;
            if (w2 + 1 < 5 || h2 + 1 < 5) continue; // min_size+2

            // 四角（tl,tr,br,bl）映射回工作图，逐点裁到界内（上游同律）
            float vx = -uy, vy = ux; // v = perp(u)：u=(1,0) 时 +y 向下
            float hw = w2 / 2, hh = h2 / 2;
            static const float cu[4] = {-1, 1, 1, -1};
            static const float cv[4] = {-1, -1, 1, 1};
            float kx = (float)W / pw, ky = (float)H / ph;
            if (nbox >= cap) break;
            int bx0 = 1 << 30, by0 = 1 << 30, bx1 = -(1 << 30), by1 = -(1 << 30);
            for (int k = 0; k < 4; k++) {
                float X = (ccx + cu[k] * hw * ux + cv[k] * hh * vx) * kx;
                float Y = (ccy + cu[k] * hw * uy + cv[k] * hh * vy) * ky;
                if (X < 0) X = 0;
                if (X > W - 1) X = (float)(W - 1);
                if (Y < 0) Y = 0;
                if (Y > H - 1) Y = (float)(H - 1);
                boxes[nbox].qx[k] = X;
                boxes[nbox].qy[k] = Y;
                int xi = (int)lroundf(X), yi = (int)lroundf(Y);
                if (xi < bx0) bx0 = xi;
                if (xi > bx1) bx1 = xi;
                if (yi < by0) by0 = yi;
                if (yi > by1) by1 = yi;
            }
            if (bx1 - bx0 + 1 <= 3 || by1 - by0 + 1 <= 3) continue;
            boxes[nbox].x0 = bx0;
            boxes[nbox].y0 = by0;
            boxes[nbox].x1 = bx1;
            boxes[nbox].y1 = by1;
            boxes[nbox].idx = nbox;
            boxes[nbox].line = 0;
            nbox++;
        }
    }

    // 排序：y 稳定排 → 相邻 dy>=10 分行 → 行内 x 排（上游 sorted_boxes）
    qsort(boxes, (size_t)nbox, sizeof(Box), cmp_box_y);
    float prev_y = -1e9f;
    int line = 0;
    for (int i = 0; i < nbox; i++) {
        if (prev_y > -1e8f && (float)boxes[i].y0 - prev_y >= SORT_Y_LINE)
            line++;
        boxes[i].line = line;
        prev_y = (float)boxes[i].y0;
    }
    qsort(boxes, (size_t)nbox, sizeof(Box), cmp_box_line);
    order_columns(boxes, nbox, W);

    free(bin);
    free(dil);
    free(lab);
    free(stack);
    free(pts);
    free(hull);
    return nbox;
}

// ---- dict 与 rec ----

static char **load_dict(const char *path, int *n_out) {
    FILE *f = fopen(path, "rb");
    if (!f) die("dict.txt not found");
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *buf = (char *)xmalloc((size_t)sz + 1);
    if (fread(buf, 1, (size_t)sz, f) != (size_t)sz) die("dict read");
    fclose(f);
    buf[sz] = 0;

    int cap = 20000, n = 0;
    char **dict = (char **)xmalloc((size_t)cap * sizeof(char *));
    char *p = buf;
    while (1) {
        char *nl = strchr(p, '\n');
        size_t len = nl ? (size_t)(nl - p) : strlen(p);
        while (len > 0 && (p[len - 1] == '\r' || p[len - 1] == '\n')) len--;
        if (n >= cap) die("dict larger than expected");
        char *ent = (char *)xmalloc(len + 1);
        memcpy(ent, p, len);
        ent[len] = 0;
        dict[n++] = ent;
        if (!nl) break;
        p = nl + 1;
        if (*p == 0) break; // 文件以 \n 结尾：无尾部空行
    }
    *n_out = n;
    return dict;
}

typedef struct {
    char *text; // NULL = 未过 text_score 门槛
    float conf;
} Line;

// 单行裁剪 → 张量 → run → CTC 贪心。返回 Line（text 需 free）。
static Line rec_line(Sess *rec, const Img *work, const Box *b, char **dict,
                     int ndict, int expected_c) {
    Line ln = {NULL, 0.0f};
    // 透视裁剪：dst 尺寸取 quad 对边长度均值（get_rotate_crop_image 同形
    // ——连续矩形尺寸，非含端点计数）。轴对齐 quad 时有理式退化为仿射。
    float wtop = hypotf(b->qx[1] - b->qx[0], b->qy[1] - b->qy[0]);
    float wbot = hypotf(b->qx[2] - b->qx[3], b->qy[2] - b->qy[3]);
    float hlef = hypotf(b->qx[3] - b->qx[0], b->qy[3] - b->qy[0]);
    float hrig = hypotf(b->qx[2] - b->qx[1], b->qy[2] - b->qy[1]);
    int cw = (int)lroundf((wtop + wbot) / 2), chh = (int)lroundf((hlef + hrig) / 2);
    if (cw < 1) cw = 1;
    if (chh < 1) chh = 1;
    Img crop = crop_quad(work, b->qx, b->qy, cw, chh);

    // 上游 resize_norm_img（单图批）：imgw=int(48*max(320/48,w/h))，
    // resized_w = ceil(48*ratio) 超 imgw 则取 imgw。
    float ratio = (float)cw / (float)chh;
    float maxr = (float)REC_W / (float)REC_H;
    if (ratio > maxr) maxr = ratio;
    int imgw = (int)((float)REC_H * maxr);
    float target = (float)REC_H * ratio;
    int rw = ceilf(target) > imgw ? imgw : (int)ceilf(target);

    Img r = img_resize(&crop, rw, REC_H);
    free(crop.px);

    size_t pl = (size_t)REC_H * imgw;
    float *t = (float *)calloc(pl * 3, sizeof(float)); // 右侧零填
    if (!t) die("out of memory");
    for (size_t i = 0; i < (size_t)REC_H * rw; i++) {
        size_t row = i / (size_t)rw, col = i % (size_t)rw;
        size_t dst = row * (size_t)imgw + col;
        t[dst] = (r.px[i * 3 + 2] / 255.0f - 0.5f) / 0.5f;
        t[pl + dst] = (r.px[i * 3 + 1] / 255.0f - 0.5f) / 0.5f;
        t[2 * pl + dst] = (r.px[i * 3] / 255.0f - 0.5f) / 0.5f;
    }
    free(r.px);

    int64_t shape[4] = {1, 3, REC_H, imgw};
    Out out;
    sess_run(rec, t, shape, 4, &out);
    if (out.ndim != 3) die("rec output rank != 3");

    int T = (int)out.dims[1], C = (int)out.dims[2];
    if (C != expected_c)
        fprintf(stderr, "ag-ocr: warn rec classes %d != dict+2 %d\n", C,
                expected_c);

    char *text = (char *)xmalloc((size_t)T * 8 + 1);
    size_t tl = 0;
    float csum = 0;
    int ccnt = 0, prev = -1;
    for (int tt = 0; tt < T; tt++) {
        const float *rowp = out.data + (size_t)tt * C;
        int best = 0;
        float bp = rowp[0];
        for (int c = 1; c < C; c++)
            if (rowp[c] > bp) { bp = rowp[c]; best = c; }
        if (tt > 0 && best == prev) { prev = best; continue; } // 去重
        prev = best;
        if (best == 0) continue; // blank
        const char *s = "?";
        if (best >= 1 && best <= ndict) s = dict[best - 1];
        else if (best == ndict + 1) s = " ";
        size_t sl = strlen(s);
        if (sl > 8) sl = 8;
        memcpy(text + tl, s, sl);
        tl += sl;
        csum += bp;
        ccnt++;
    }
    text[tl] = 0;
    ort->ReleaseValue(out.val);
    free(t);

    ln.conf = ccnt ? csum / ccnt : 0.0f;
    if (ccnt == 0 || ln.conf < TEXT_SCORE || tl == 0) {
        free(text);
        return ln;
    }
    ln.text = text;
    return ln;
}

// ---- 输出 ----

static void json_print(const char *s) {
    for (const unsigned char *p = (const unsigned char *)s; *p; p++) {
        if (*p == '"' || *p == '\\') {
            putchar('\\');
            putchar(*p);
        } else if (*p < 0x20) {
            printf("\\u%04x", *p);
        } else {
            putchar(*p);
        }
    }
}

// 一轮 det+rec（在给定朝向的图上）。行文本的 malloc 由进程退出收尾。
typedef struct {
    Box boxes[1024];
    Line lines[1024];
    int nbox, nlines, kept;
    int hkept; // kept 里 quad 长边为横向的行数（择优主键，见旋转轮注释）
    int vw_w, vw_h; // 本轮朝向的图尺寸
    double det_ms, rec_ms, conf_sum;
} Pipes;

static void run_pipes(Sess *det, Sess *rec, const Img *work, char **dict,
                      int ndict, int expected_c, int rec_only, Pipes *r) {
    memset(r, 0, sizeof *r);
    r->vw_w = work->w;
    r->vw_h = work->h;
    double t0 = now_ms();
    if (rec_only) {
        Box *b0 = &r->boxes[0];
        b0->x0 = 0;
        b0->y0 = 0;
        b0->x1 = work->w - 1;
        b0->y1 = work->h - 1;
        b0->idx = 0;
        b0->line = 0;
        b0->qx[0] = 0;                    b0->qy[0] = 0;
        b0->qx[1] = (float)(work->w - 1); b0->qy[1] = 0;
        b0->qx[2] = (float)(work->w - 1); b0->qy[2] = (float)(work->h - 1);
        b0->qx[3] = 0;                    b0->qy[3] = (float)(work->h - 1);
        r->nbox = 1;
    } else {
        Img di = det_resize(work);
        float *t = norm_chw_bgr(di.px, di.w, di.h);
        int64_t shape[4] = {1, 3, di.h, di.w};
        Out out;
        sess_run(det, t, shape, 4, &out);
        if (out.ndim < 2) die("det output rank < 2");
        int64_t ph = out.dims[out.ndim - 2], pw = out.dims[out.ndim - 1];
        r->det_ms = now_ms() - t0;
        r->nbox = det_post(out.data, (int)pw, (int)ph, work->w, work->h,
                           r->boxes, (int)(sizeof r->boxes / sizeof r->boxes[0]));
        ort->ReleaseValue(out.val);
        free(t);
        free(di.px);
    }
    double t1 = now_ms();
    for (int i = 0; i < r->nbox && r->nlines < 1024; i++)
        r->lines[r->nlines++] = rec_line(rec, work, &r->boxes[i], dict, ndict,
                                         expected_c);
    r->rec_ms = now_ms() - t1;
    for (int i = 0; i < r->nlines; i++) {
        if (r->lines[i].text) {
            r->kept++;
            r->conf_sum += r->lines[i].conf;
            // 长边方向（tr+br−tl−bl）：|ux|>=|uy| 判横排。quad 裁剪会把
            // 竖排文字转正读出——旋转轮原本靠 rec 全灭当失败信号，几何
            // 升级后没了；竖排胜出 = 页面被转了 90°，行序实为列序。
            float dux = r->boxes[i].qx[1] + r->boxes[i].qx[2] -
                        r->boxes[i].qx[0] - r->boxes[i].qx[3];
            float duy = r->boxes[i].qy[1] + r->boxes[i].qy[2] -
                        r->boxes[i].qy[0] - r->boxes[i].qy[3];
            if (fabsf(dux) >= fabsf(duy)) r->hkept++;
        }
    }
}

int main(int argc, char **argv) {
    int json = 0, rec_only = 0, rot = -1; // -1 = auto（见文件头）
    const char *path = NULL;
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--json") == 0) json = 1;
        else if (strcmp(argv[i], "--rec-only") == 0) rec_only = 1;
        else if (strcmp(argv[i], "--rot") == 0 && i + 1 < argc) {
            const char *a = argv[++i];
            if (!strcmp(a, "auto")) rot = -1;
            else if (!strcmp(a, "0")) rot = 0;
            else if (!strcmp(a, "90")) rot = 90;
            else if (!strcmp(a, "180")) rot = 180;
            else if (!strcmp(a, "270")) rot = 270;
            else die("--rot expects auto|0|90|180|270");
        } else if (!path) path = argv[i];
        else die("usage: ag-ocr [--rot auto|0|90|180|270] <jpg> [--json] | "
                "ag-ocr --rec-only <jpg>");
    }
    if (!path) die("usage: ag-ocr [--rot auto|0|90|180|270] <jpg> [--json] | "
                   "ag-ocr --rec-only <jpg>");

#ifdef __ANDROID__
    {
        cpu_set_t set;
        CPU_ZERO(&set);
        CPU_SET(6, &set);
        CPU_SET(7, &set);
        if (sched_setaffinity(0, sizeof set, &set) != 0)
            fprintf(stderr, "ag-ocr: affinity fallback to all cores\n");
    }
#endif

    const char *dir = getenv("AG_OCR_DIR");
    if (!dir || !*dir) dir = "/var/models/ocr";
    char det_p[512], rec_p[512], dict_p[512];
    snprintf(det_p, sizeof det_p, "%s/det.onnx", dir);
    snprintf(rec_p, sizeof rec_p, "%s/rec.onnx", dir);
    snprintf(dict_p, sizeof dict_p, "%s/dict.txt", dir);

    int ndict = 0;
    char **dict = load_dict(dict_p, &ndict);
    int expected_c = ndict + 2; // blank + dict + space

    int w, h, ch;
    uint8_t *rgb = stbi_load(path, &w, &h, &ch, 3);
    if (!rgb) die("stbi_load failed (not an image?)");
    Img work = {w, h, rgb};

    // 全局预缩放：最长边 > 2000 封顶（RapidOCR Global.max_side_len）
    int mx = work.w > work.h ? work.w : work.h;
    if (mx > PRE_MAX_SIDE) {
        int nw = (int)lroundf((float)work.w * PRE_MAX_SIDE / mx);
        int nh = (int)lroundf((float)work.h * PRE_MAX_SIDE / mx);
        Img s = img_resize(&work, nw, nh);
        stbi_image_free(work.px);
        work = s;
    }

    ort = OrtGetApiBase()->GetApi(ORT_API_VERSION);
    OrtEnv *env;
    check(ort->CreateEnv((OrtLoggingLevel)3 /*ORT_LOGGING_LEVEL_ERROR*/, "ag-ocr",
                         &env),
          "env");
    Sess det, rec;
    sess_open(&det, env, det_p);
    sess_open(&rec, env, rec_p);

    // 朝向循环：auto 先 90（竖握是产品常态——传感器横装，竖页文字在图里
    // 转 90°，M45 收据），横排 kept>=2 行即收；否则 0/270/180 全试，择优
    // 主键是横排行数（竖排 quad 也能转正读出，但行序是列序——只当并列
    // 时的次级信号）。指定角度只跑该角度。计时累计各轮。
    static Pipes best, cur;
    int best_rot = rot >= 0 ? rot : 90, have = 0;
    double sum_det = 0, sum_rec = 0;
    const int order[4] = {90, 0, 270, 180};
    for (int oi = 0; oi < 4; oi++) {
        int r = order[oi];
        if (rot >= 0 && r != rot) continue;
        Img vw = work;
        if (r == 90) vw = img_rot90(&work, 1);
        else if (r == 270) vw = img_rot90(&work, -1);
        else if (r == 180) vw = img_rot180(&work);
        run_pipes(&det, &rec, &vw, dict, ndict, expected_c, rec_only, &cur);
        if (vw.px != work.px) free(vw.px);
        sum_det += cur.det_ms;
        sum_rec += cur.rec_ms;
        if (!have || cur.hkept > best.hkept ||
            (cur.hkept == best.hkept && cur.kept > best.kept) ||
            (cur.hkept == best.hkept && cur.kept == best.kept &&
             cur.conf_sum > best.conf_sum)) {
            best = cur;
            best_rot = r;
            have = 1;
        }
        if (rot >= 0 || cur.hkept >= 2) break;
    }

    if (json) {
        printf("{\"lines\":[");
        int first = 1;
        for (int i = 0; i < best.nlines; i++) {
            if (!best.lines[i].text) continue;
            if (!first) printf(",");
            first = 0;
            printf("{\"box\":[%d,%d,%d,%d],\"conf\":%.4f,\"text\":\"",
                   best.boxes[i].x0, best.boxes[i].y0, best.boxes[i].x1,
                   best.boxes[i].y1, best.lines[i].conf);
            json_print(best.lines[i].text);
            printf("\"}");
        }
        printf("]}\n");
    } else {
        for (int i = 0; i < best.nlines; i++)
            if (best.lines[i].text)
                printf("%s\t%.4f\n", best.lines[i].text, best.lines[i].conf);
    }
    fprintf(stderr, "ag-ocr: rot %d, det %.0fms, rec %d box %.0fms, "
                    "kept %d/%d horiz %d, img %dx%d, dict %d\n",
            best_rot, sum_det, best.nbox, sum_rec, best.kept, best.nlines,
            best.hkept, best.vw_w, best.vw_h, ndict);
    return best.kept > 0 ? 0 : 1;
}
