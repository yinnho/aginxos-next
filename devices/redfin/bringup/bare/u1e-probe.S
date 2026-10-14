/* U1e — 外域可达性判别器（阶梯 3 刀 1e，2026-10-14）
 *
 * GICD @0x17a00000 读探针（外域、Linux 最早碰的块、DT 真源）。读
 * 不看值，blink。分岔：
 *   blink×3 → 外域在 MMU-off 制度下可达 → UFS/GCC 是各块自己的病
 *             （时钟/域电源），继续块级刀；
 *   冻死    → AOSS 岛外全不可达 → MMU-off 制度是病（Linux 碰外域时
 *             恒等映射已开），下一刀=恒等映射页表探针。
 * 背景：U1a/U1c/U1d 冷热启动皆冻（0x1d84000 与 0x100000 读挂；SPMI
 * /PS_HOLD 岛内全活——病分界在常开岛边界）。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x17A0, lsl #16    /* 0x17A0<<16 = 0x17a00000（8位！） */    /* GICD 0x17a00000 */

    ldr    w20, [x3]               /* 读不挂即过 */

    movz   x12, #0xCA00            /* delay 1e9 */
    movk   x12, #0x3B9A, lsl #16
1:  subs   x12, x12, #1
    b.ne   1b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]               /* PS_HOLD ← 0 */
2:  b      2b
