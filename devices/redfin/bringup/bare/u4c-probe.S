/* U4C — UFS 首块读·PRDT 对账版（阶梯 3 刀 4-c，2026-10-14）
 *
 * u4c = u4（LBA1 + 头 0x01004001 两修已在）唯改一处：PRDT 从 +0x400
 * 搬到 UCD+0x100 —— DW7=0x01000001 所指处（Linux ufshcd 同布局：
 * 响应@+0x80、PRDT@+0x100）。u3b 若证 READ CAPACITY 送达（PRDT 错位
 * = 全部冻死根因），本刀即 READ(10) LBA1 首证冲线：4096 世界 ×1 块，
 * magic 首八字节 "EFI PART"。
 *
 * 判读：blink×3 = 无内核读盘首证（GPT 头到手）；
 *       冻死 = 尺寸世界疑（→ u4d 512 版同修）或更深协议层。
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

    /* UTRD（DW1=4096） */
    movz   x10, #0x0500, lsl #16
    str    x10, [x8]
    movz   x10, #0x1000            /* DW1 = 4096B */
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
    movk   x10, #0x0001            /* DW7 = prd_len 1 | prd_off 0x100 */
    str    w10, [x8, #28]

    /* 命令 UPIU：头 0x01004001、exp_len BE32=4096、CDB=READ(10) LBA1 ×1 */
    movz   x10, #0x0100, lsl #16
    movk   x10, #0x4001            /* 内存 01 40 00 01 = flags40·LUN0·tag1 */
    str    w10, [x9]
    str    wzr, [x9, #4]
    str    wzr, [x9, #8]
    movz   x10, #0x0010, lsl #16   /* exp_len BE32=0x1000 → LE 0x00100000 */
    str    x10, [x9, #12]
    mov    w10, #0x28              /* CDB[0] = READ(10) */
    strb   w10, [x9, #16]
    movz   x10, #0x0100, lsl #16   /* CDB[2..5] BE 00000001 → LE 0x01000000 */
    str    x10, [x9, #0x12]
    movz   w10, #0x0100            /* CDB[7..8] BE 0001 → LE 0x0100 */
    strh   w10, [x9, #0x17]

    /* PRDT 唯改：UCD+0x100（DW7 所指处）、size=4096−1 */
    str    x25, [x9, #0x100]       /* base = 0xB0002000 */
    str    wzr, [x9, #0x108]       /* reserved */
    movz   x10, #0x0FFF
    str    w10, [x9, #0x10c]       /* size = 4096−1 */

    /* 发单：UTRLBA → RSR → DBR */
    movz   x10, #0xB000, lsl #16
    str    x10, [x3, #0x50]
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
