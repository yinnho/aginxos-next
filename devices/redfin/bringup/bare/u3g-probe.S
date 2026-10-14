/* U3G — 时延编码仪器（阶梯 3，2026-10-14 夜）
 *
 * 动机：blink/冻 一比特仪器把「门铃超时 / OCS≠0 / CHECK CONDITION / 数据未落
 * / CPU 中止」五种结局混成一个「冻」。u3g 命令体=u3e 原样（DW0 0x14000000、
 * DW1 0、DW7 0x01000001、头 01 40 00 00、exp_len BE 8、CDB 0x25、UTRLBA 32 位
 * 写），只换尾部：任何结局都 PS_HOLD 复位，阶段编码进复位前的等待时长
 * （CNTVCT/CNTFRQ 真秒）：
 *
 *   stage0 过（数据非零）            等 2s
 *   stage1 门铃 1s 不清（超时）      等 32s
 *   stage2 OCS≠0                     等 62s
 *   stage3 响应 trans≠0x21 或 response(byte6)≠0   等 92s
 *   stage4 SCSI status(byte7)≠GOOD（如 CHECK CONDITION/UA）  等 122s
 *   stage5 状态全好但数据为零        等 152s
 *   仍冻 = CPU 本身中止/挂死（新信号）
 *
 * 响应 UPIU @UCD+0x200：byte0 trans、byte6 response、byte7 status
 * （Linux be32(dword_1)&0xFFFF；UFSDxe FUN_d1c4 同；旧档 +0x205 错位）。
 * 判读：3 轮复位后 adb 回归时刻 ≈ 基线 + 3×等待，阶段间隔 ≥90s；
 * 用户秒表量 logo→黑 亦可（单轮间隔 30s）。
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

    /* UCD 清零 0x600 + UTRD 清零 32B + 数据缓冲 8B 清零（非零门防假阳） */
    mov    x11, #0xC0
1:  str    xzr, [x9], #8
    subs   x11, x11, #1
    b.ne   1b
    sub    x9, x9, #0x600
    stp    xzr, xzr, [x8]
    stp    xzr, xzr, [x8, #16]
    str    xzr, [x25]

    /* UTRD：DW0=0x14000000（type1|DD读）；DW1=8B；DW2 预填 OCS=0x0F */
    movz   x10, #0x1400, lsl #16   /* DW0 = 0x14000000 修① */
    str    x10, [x8]
    str    wzr, [x8, #4]           /* DW1 = DUNL（crypto 关=保留 0，ufshcd.c:3631；非数据长度） */
    mov    w10, #0x0F              /* DW2 = invalid OCS 预填 */
    str    w10, [x8, #8]
    str    wzr, [x8, #12]
    movz   x10, #0xB000, lsl #16
    movk   x10, #0x1000            /* DW4/5 = UCD 地址 */
    str    x10, [x8, #16]
    str    wzr, [x8, #20]
    movz   x10, #0x0080, lsl #16
    movk   x10, #0x0080            /* DW6 = RUL 0x80 | RUO 0x80（0x200>>2） */
    str    x10, [x8, #24]
    movz   x10, #0x0100, lsl #16   /* DW7 高半 = PRDT offset 0x100（0x400>>2） */
    movk   x10, #0x0001            /* DW7 低半 = PRDT length 1（ufshci.h:560 length 在 +28） */
    str    w10, [x8, #28]

    /* 命令 UPIU：头=01 40 00 00（trans/读flags/LUN0/tag0 修③）*/
    movz   x10, #0x4001            /* LE32 0x00004001 → 内存 01 40 00 00 */
    str    w10, [x9]
    str    wzr, [x9, #4]           /* dword1 = cmd_set_type SCSI=0 + 保留 */
    str    wzr, [x9, #8]           /* dword2 = EHS/data seg 长度 = 0 */
    mov    w10, #8                 /* exp_len BE32=8 → rev 通式 修② */
    rev    w10, w10                /* 0x00000008 → 0x08000000 */
    str    w10, [x9, #12]
    mov    w10, #0x25              /* CDB[0] = READ CAPACITY(10)，其余零 */
    strb   w10, [x9, #16]

    /* PRDT：base 0xB0002000、size=8−1=7 */
    str    x25, [x9, #0x400]
    str    wzr, [x9, #0x408]
    mov    w10, #7
    str    w10, [x9, #0x40c]

    /* 发单：UTRLBA → RSR → DBR */
    movz   x10, #0xB000, lsl #16
    str    w10, [x3, #0x50]        /* UTRLBA 32 位写（u3e 唯一变量） */
    str    wzr, [x3, #0x54]
    mov    w10, #1
    str    w10, [x3, #0x60]
    str    w10, [x3, #0x58]

    /* 计时基：x20 = CNTFRQ（0 则兜底 19.2MHz） */
    mrs    x20, cntfrq_el0
    cbnz   x20, 1f
    movz   x20, #0x4c00
    movk   x20, #0x0124, lsl #16   /* 19200000 */
1:
    /* 轮询门铃自清：截止 = now + 1s */
    isb
    mrs    x12, cntvct_el0
    add    x12, x12, x20
2:  ldr    w10, [x3, #0x58]
    tbz    w10, #0, 3f
    isb
    mrs    x13, cntvct_el0
    cmp    x13, x12
    b.lo   2b
    mov    x21, #1                 /* stage1 门铃超时 */
    b      8f
3:  ldr    w10, [x8, #8]
    and    w10, w10, #0x0F
    mov    x21, #2                 /* stage2 OCS≠0 */
    cbnz   w10, 8f

    /* 响应 UPIU @UCD+0x200 */
    ldrb   w10, [x9, #0x200]
    mov    x21, #3                 /* stage3 trans≠0x21 / response≠0 */
    cmp    w10, #0x21
    b.ne   8f
    ldrb   w10, [x9, #0x206]
    cbnz   w10, 8f
    ldrb   w10, [x9, #0x207]
    mov    x21, #4                 /* stage4 SCSI status≠GOOD */
    cbnz   w10, 8f

    /* 非零门：last_lba | block_size ≠ 0 */
    ldr    w27, [x25]
    ldr    w28, [x25, #4]
    orr    w27, w27, w28
    mov    x21, #5                 /* stage5 数据零 */
    cbz    w27, 8f
    mov    x21, #0                 /* stage0 过 */

    /* 等 (2 + 30×stage) 秒 → PS_HOLD ← 0 */
8:  mov    x11, #30
    mul    x11, x11, x21
    add    x11, x11, #2
    mul    x11, x11, x20           /* 秒 × 频率 = ticks */
    isb
    mrs    x12, cntvct_el0
    add    x12, x12, x11
4:  isb
    mrs    x13, cntvct_el0
    cmp    x13, x12
    b.lo   4b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]
9:  b      9b
