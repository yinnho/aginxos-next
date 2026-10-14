/* U3 — UFS UTR 往返探针（阶梯 3 刀 3，2026-10-14）
 *
 * 全路径首证：DRAM 布阵 UTRD/UCD/PRDT → READ CAPACITY(10) 8B →
 * 门铃发单 → 轮询完成 → OCS==0 → blink；任何一步不成 → 冻死。
 * 布阵：UTRDL @0xB0000000（1KB 对齐）、UCD @0xB0001000（命令 UPIU
 * +0 / 响应 +0x200 / PRDT +0x400）、数据 @0xB0002000。
 * 契约见 TRUTH.md §7（DW6=0x00800080、DW7=0x01000001、PRDT size=字节−1）。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x01d8, lsl #16
    movk   x3, #0x4000             /* x3 = UFSHCI 0x1d84000 */
    movz   x8, #0xB000, lsl #16    /* x8 = UTRDL 0xB0000000 */
    movz   x9, #0xB000, lsl #16
    movk   x9, #0x1000             /* x9 = UCD 0xB0001000 */

    /* UCD 清零 0x600 */
    mov    x11, #0xC0
1:  str    xzr, [x9], #8
    subs   x11, x11, #1
    b.ne   1b
    sub    x9, x9, #0x600
    /* UTRD 清零 32B */
    stp    xzr, xzr, [x8]
    stp    xzr, xzr, [x8, #16]

    /* UTRD */
    movz   x10, #0x0500, lsl #16   /* DW0 = DD|INT = 0x05000000 */
    str    x10, [x8]
    mov    w10, #8                 /* DW1 = 数据 8B */
    str    w10, [x8, #4]
    mov    w10, #0x0F              /* DW2 = invalid OCS 预填 */
    str    w10, [x8, #8]
    str    wzr, [x8, #12]          /* DW3 */
    movz   x10, #0xB000, lsl #16
    movk   x10, #0x1000            /* DW4 = UCD 地址 */
    str    x10, [x8, #16]
    str    wzr, [x8, #20]          /* DW5 */
    movz   x10, #0x0080, lsl #16
    movk   x10, #0x0080            /* DW6 = resp_len 0x80 | resp_off 0x80 */
    str    x10, [x8, #24]
    movz   x10, #0x0100, lsl #16
    movk   x10, #0x0001            /* DW7 = prd_len 1 | prd_off 0x100 */
    str    w10, [x8, #28]

    /* 命令 UPIU @UCD+0 */
    movz   x10, #0x0140, lsl #16
    movk   x10, #0x0001            /* BE32: 01 40 00 01 = COMMAND|READ|LUN0|tag1 */
    str    w10, [x9]
    str    wzr, [x9, #4]
    str    wzr, [x9, #8]
    movz   x10, #0x0800, lsl #16   /* exp_len BE32=8 → LE 落 0x08000000 */
    str    w10, [x9, #12]
    mov    w10, #0x25              /* CDB[0] = READ CAPACITY(10) */
    strb   w10, [x9, #16]

    /* PRDT @UCD+0x400 */
    movz   x10, #0xB000, lsl #16
    movk   x10, #0x2000
    str    x10, [x9, #0x400]
    str    wzr, [x9, #0x404]
    str    wzr, [x9, #0x408]
    mov    w10, #7                 /* size = 8−1 */
    str    w10, [x9, #0x40c]

    /* 发单：UTRLBA → RSR → DBR */
    movz   x10, #0xB000, lsl #16
    str    w10, [x3, #0x50]
    str    wzr, [x3, #0x54]
    mov    w10, #1
    str    w10, [x3, #0x60]
    str    w10, [x3, #0x58]

    /* 轮询门铃自清（超时 1e8） */
    movz   x12, #0xE100
    movk   x12, #0x05F5, lsl #16
2:  ldr    w10, [x3, #0x58]
    tbz    w10, #0, 3f
    subs   x12, x12, #1
    b.ne   2b
    b      9f                      /* 超时 → 冻死 */
3:  ldr    w10, [x8, #8]
    and    w10, w10, #0x0F
    cbnz   w10, 9f                 /* OCS≠0 → 冻死 */

    /* pass：delay 1e9 → PS_HOLD ← 0 */
    movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16
4:  subs   x12, x12, #1
    b.ne   4b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]
9:  b      9b
