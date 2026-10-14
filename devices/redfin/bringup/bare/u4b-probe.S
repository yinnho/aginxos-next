/* U4B — 512 字节世界问法（阶梯 3 刀 4-b，2026-10-14）
 *
 * 前账：u3 全往返绿（OCS==0）；u4 4096 问法冻死（tries=2+眼判+救机）。
 * 主嫌=尺寸雷：512 世界上 DW1=4096 配 1 块 = OCS 3「尺寸不匹配」
 * （TRUTH §7 OCS 表）。本刀 = u4 唯改尺寸三处：
 *   DW1 4096→512、UPIU exp_len 0x1000→0x200、PRDT size 0xFFF→0x1FF。
 * CDB 仍是 READ(10) LBA1 ×1；magic 仍查 buffer 首 8B=="EFI PART"
 * （GPT 头在两种世界都在 LBA1 开头）。
 *
 * 判读：blink×3 = 512 世界坐实 + 无内核读盘首证（收官）；
 *       冻死 = 4096 世界正确，u4 冻因在 magic/LUN → 下一刀 u4c。
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
    movz   x25, #0xB000, lsl #16
    movk   x25, #0x2000            /* x25 = 数据 0xB0002000 */

    /* UCD 清零 0x600 + UTRD 清零 32B */
    mov    x11, #0xC0
1:  str    xzr, [x9], #8
    subs   x11, x11, #1
    b.ne   1b
    sub    x9, x9, #0x600
    stp    xzr, xzr, [x8]
    stp    xzr, xzr, [x8, #16]

    /* UTRD（DW1=512 —— 唯一改①） */
    movz   x10, #0x0500, lsl #16
    str    x10, [x8]
    movz   x10, #0x0200            /* DW1 = 512B */
    str    w10, [x8, #4]
    mov    w10, #0x0F
    str    w10, [x8, #8]
    str    wzr, [x8, #12]
    movz   x10, #0xB000, lsl #16
    movk   x10, #0x1000
    str    x10, [x8, #16]
    str    wzr, [x8, #20]
    movz   x10, #0x0080, lsl #16
    movk   x10, #0x0080            /* DW6 */
    str    x10, [x8, #24]
    movz   x10, #0x0100, lsl #16
    movk   x10, #0x0001            /* DW7 */
    str    w10, [x8, #28]

    /* 命令 UPIU：exp_len BE32=0x200（改②）、CDB=READ(10) LBA1 ×1 */
    movz   x10, #0x0100, lsl #16
    movk   x10, #0x4001
    str    x10, [x9]
    str    wzr, [x9, #4]
    str    wzr, [x9, #8]
    movz   x10, #0x0200, lsl #16   /* exp_len BE32=0x200 → LE 0x02000000 */
    str    x10, [x9, #12]
    mov    w10, #0x28              /* CDB[0] = READ(10) */
    strb   w10, [x9, #16]
    movz   x10, #0x0100, lsl #16   /* CDB[2..5] BE 00000001 → LE 0x01000000 */
    str    x10, [x9, #0x12]
    movz   x10, #0x0100            /* CDB[7..8] BE 0001 → LE 0x0100 */
    strh   w10, [x9, #0x17]

    /* PRDT：数据 0xB0002000、size=512−1（改③） */
    str    x25, [x9, #0x400]
    str    wzr, [x9, #0x404]
    str    wzr, [x9, #0x408]
    movz   x10, #0x01FF
    str    w10, [x9, #0x40c]

    /* 发单 */
    movz   x10, #0xB000, lsl #16
    str    x10, [x3, #0x50]
    str    wzr, [x3, #0x54]
    mov    w10, #1
    str    w10, [x3, #0x60]
    str    w10, [x3, #0x58]

    /* 轮询门铃自清 */
    movz   x12, #0xE100
    movk   x12, #0x05F5, lsl #16
2:  ldr    w10, [x3, #0x58]
    tbz    w10, #0, 3f
    subs   x12, x12, #1
    b.ne   2b
    b      9f
3:  ldr    w10, [x8, #8]
    and    w10, w10, #0x0F
    cbnz   w10, 9f                 /* OCS≠0 → 冻死 */

    /* magic：bytes "EFI PART" → LE dwords 0x20494645 / 0x54524150 */
    ldr    w27, [x25]
    movz   x28, #0x4645
    movk   x28, #0x2049, lsl #16
    cmp    w27, w28
    b.ne   9f
    ldr    w27, [x25, #4]
    movz   x28, #0x4150
    movk   x28, #0x5452, lsl #16
    cmp    w27, w28
    b.ne   9f

    /* pass：delay 1e9 → PS_HOLD ← 0 */
    movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16
4:  subs   x12, x12, #1
    b.ne   4b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]
9:  b      9b
