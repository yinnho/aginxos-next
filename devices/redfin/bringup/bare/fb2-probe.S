/* fb2-probe.S —— B18b 白带判别探针（2026-10-14）
 *
 * B18 机械面全通（paint→10s→复位×3→回滚，fb 写不崩）但双带不可见。
 * 两色字 byte0 均=0x00——若格式 ARGB（byte0=A）则全透明。本桩单变量：
 * 纯白带 0xFFFFFFFF（任何 RGBA/BGRA/ARGB/ABGR 下皆不透明白）。
 *   白带现 → 地址对，透明度坑（B18c 定格式）
 *   白带不现 → 0xA0000000 非扫描源，fb 判活线降级
 * 几何同 B18：行 500–599 整宽。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    movz   x1, #0xFD00
    movk   x1, #0x20, lsl #16        /* +0x20FD00（行 500）*/
    movz   x2, #0xA5E0
    movk   x2, #0x1, lsl #16         /* 108000 像素 */
    mov    x3, #-1                   /* 0xFFFFFFFF 纯白 */
1:  str    w3, [x1], #4
    subs   x2, x2, #1
    b.ne   1b

    movz   x12, #0x3B9A
    movk   x12, #0xCA00, lsl #16     /* 10s 展示窗 */
2:  subs   x12, x12, #1
    b.ne   2b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]
3:  b      3b
