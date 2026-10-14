/* U3H — stage3 细分仪器（阶梯 3，2026-10-14 夜）
 *
 * u3g 判 stage3（门铃清+OCS=0，响应 UPIU byte0≠0x21 或 byte6≠0）。u3h 命令体
 * =u3g/u3e 原样，只把响应头拆成四类，等待秒数直编（CNTVCT）：
 *
 *   byte0==0x00（响应未写）              等 32s
 *   byte0==0x21 且 byte6(response)≠0     等 62s
 *   byte0==0x3F（REJECT UPIU）           等 92s
 *   byte0 其它                           等 122s
 *   —— 后备（u3g 已排除，留作防跑偏）——
 *   门铃 1s 超时 152s / OCS≠0 182s / status≠GOOD 212s / 数据零 242s / 过 2s
 *   仍冻 = CPU 中止
 *
 * 判读（3 轮 + 基线 40–100s）：32→136–196s、62→226–286s、92→316–376s、
 * 122→406–466s；后备 152→496–556s 起。
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

    /* 计时基：x20 = CNTFRQ（0 则兜底 19.2MHz）；x21 = 等待秒数 */
    mrs    x20, cntfrq_el0
    cbnz   x20, 1f
    movz   x20, #0x4c00
    movk   x20, #0x0124, lsl #16   /* 19200000 */
1:
    isb
    mrs    x12, cntvct_el0
    add    x12, x12, x20           /* 门铃截止 = now + 1s */
2:  ldr    w10, [x3, #0x58]
    tbz    w10, #0, 3f
    isb
    mrs    x13, cntvct_el0
    cmp    x13, x12
    b.lo   2b
    mov    x21, #152               /* 门铃超时（后备） */
    b      8f
3:  ldr    w10, [x8, #8]
    and    w10, w10, #0x0F
    mov    x21, #182               /* OCS≠0（后备） */
    cbnz   w10, 8f

    /* 响应 UPIU @UCD+0x200 四分 */
    ldrb   w10, [x9, #0x200]
    mov    x21, #32                /* byte0==0：响应未写 */
    cbz    w10, 8f
    mov    x21, #92                /* byte0==0x3F：REJECT */
    cmp    w10, #0x3F
    b.eq   8f
    mov    x21, #122               /* byte0 其它 */
    cmp    w10, #0x21
    b.ne   8f
    ldrb   w10, [x9, #0x206]
    mov    x21, #62                /* 0x21 但 response≠0 */
    cbnz   w10, 8f
    ldrb   w10, [x9, #0x207]
    mov    x21, #212               /* status≠GOOD（后备） */
    cbnz   w10, 8f

    ldr    w27, [x25]
    ldr    w28, [x25, #4]
    orr    w27, w27, w28
    mov    x21, #242               /* 数据零（后备） */
    cbz    w27, 8f
    mov    x21, #2                 /* 过 */

    /* 等 x21 秒 → PS_HOLD ← 0 */
8:  mul    x11, x21, x20
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
