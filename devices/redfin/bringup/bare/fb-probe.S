/* fb-probe.S —— B18 splash fb 判活通道探针（2026-10-14，RUNG2 D2）
 *
 * 真 DT：cont_splash_region=0xA0000000/0x2300000（无 no-map 纯 DRAM，
 * ABL 的 logo 就画在此，显示管线 ABL 留着）。裸件直接写像素=零依赖屏上
 * 输出——RUNG2 的判活信号（稳态与冻死同形的解药）。
 *
 * 本桩零 SPMI：涂两条判读带 → 10s 展示 → PS_HOLD 复位（blink×3 自动
 * 回滚回 Linux）。
 *   band1 0xFFFF0000：RGBA=蓝 / BGRA=红
 *   band2 0xFFFFFF00：RGBA=青 / BGRA=黄
 *   （两带同现+颜色报回=fb 可写+格式定谳；啥都没有=fb 路不通）
 * 几何：1080×2340 stride=0x10E0；band1=行 500–599，band2=行 700–799，
 * 各 100 行整宽。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    /* —— band1：0xA0000000 + 500*0x10E0 = +0x20FD00，色 0xFFFF0000 —— */
    movz   x1, #0xFD00
    movk   x1, #0x20, lsl #16        /* 偏移 0x20FD00 */
    movz   x2, #0xA5E0
    movk   x2, #0x1, lsl #16         /* 108000 像素（100 行×1080）*/
    movz   x3, #0x0
    movk   x3, #0xFFFF, lsl #16      /* 0xFFFF0000 */
1:  str    w3, [x1], #4
    subs   x2, x2, #1
    b.ne   1b

    /* —— band2：+0x2E1D80（行 700），色 0xFFFFFF00 —— */
    movz   x1, #0x1D80
    movk   x1, #0x2E, lsl #16
    movz   x2, #0xA5E0
    movk   x2, #0x1, lsl #16
    movz   x3, #0xFF00
    movk   x3, #0xFFFF, lsl #16      /* 0xFFFFFF00 */
2:  str    w3, [x1], #4
    subs   x2, x2, #1
    b.ne   2b

    /* —— 10s 展示窗（1e9 延时）→ PS_HOLD —— */
    movz   x12, #0x3B9A
    movk   x12, #0xCA00, lsl #16
3:  subs   x12, x12, #1
    b.ne   3b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]
4:  b      4b
