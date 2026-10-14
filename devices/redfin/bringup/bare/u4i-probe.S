/* U4I — 第三条命令 vs LBA2 分诊 + 敲门铃前控制器状态旁读（阶梯 3，2026-10-14 夜）
 *
 * u4h 判 1097s=332s 类=读 #2 门铃超时（缓冲已改 0xB0003000→地址无罪）；同时
 * 证读 #1 过、头健全、头 CRC32 中（读盘逐字节对+设备上 CRC 例程对）。读 #2
 * 是同槽第三条命令——过关者皆只发两条。UFSDxe 敲铃前清 IS(=5)+查 RSR==1，
 * 等待中盯 IS 错误位 0x31804（UE/DFES/UTPES/HCFES/SBFES），出错 UTRLCLR 清槽，
 * 完成后写 IS=1；我方全没做。
 *
 * 行为变量：顺序改为 SSU → 读 LBA2（第 2 条）→ 读 LBA1（第 3 条），去 CRC。
 * 仪器（只读不改）：两次读在敲铃前读 RSR(0x60) 与 IS(0x20)。
 * 结局等待秒数（CNTVCT）：
 *   两读皆 GOOD                         2s
 *   读 LBA2（第 2 条）门铃超时          302s
 *   读 LBA1（第 3 条）门铃超时          332s
 *   敲铃前 RSR bit0==0（列表已停）      362s
 *   敲铃前 IS & 0x31804 ≠ 0（错误挂起） 392s
 *   READ CHECK CONDITION 122 / SSU 被拒 152 / SSU 超时 182 / OCS 212 / byte0 242
 * 判读（3 轮 + 基线 78–104s）：2→84–109 / 302→984–1010 / 332→1074–1100 /
 * 362→1164–1190 / 392→1254–1280s。
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
    mov    x23, #0                 /* tag 计数器 */

    /* 计时基：x20 = CNTFRQ（0 则兜底 19.2MHz） */
    mrs    x20, cntfrq_el0
    cbnz   x20, 1f
    movz   x20, #0x4c00
    movk   x20, #0x0124, lsl #16   /* 19200000 */
1:
    /* UTRLBA（32 位写）→ RSR */
    movz   w10, #0xB000, lsl #16
    str    w10, [x3, #0x50]
    str    wzr, [x3, #0x54]
    mov    w10, #1
    str    w10, [x3, #0x60]

    /* ① SSU ACTIVE → W-LUN 0xD0，UA 重试 1 次 */
    mov    x22, #0
20: bl     prep
    movz   w10, #0x1000, lsl #16   /* DW0 = 0x10000000（CT1、DD 无） */
    str    w10, [x8]
    movz   w10, #0x0100, lsl #16   /* DW7 = PRDT off 0x100 | 长 0 */
    str    w10, [x8, #28]
    movz   w10, #0x00D0, lsl #16
    movk   w10, #0x0001            /* LE 0x00D00001 → 01 00 D0 00 */
    str    w10, [x9]
    mov    w10, #0x1B              /* CDB[0] = START STOP UNIT */
    strb   w10, [x9, #16]
    mov    w10, #0x10              /* CDB[4] = power condition 1<<4 */
    strb   w10, [x9, #20]
    mov    x29, #182               /* SSU 超时码 */
    bl     issue
    cbz    w0, 30f                 /* GOOD → 读 */
    mov    x21, #152               /* SSU 被拒 */
    cmp    w1, #6
    b.ne   8f
    cbnz   x22, 8f
    mov    x22, #1
    b      20b

    /* ② 读 LBA2 → 0xB0003000（第 2 条命令） */
30: mov    w4, #2
    mov    w5, #1
    movz   x6, #0xB000, lsl #16
    movk   x6, #0x3000
    mov    x29, #302
    bl     read
    /* ③ 读 LBA1 → 0xB0002000（第 3 条命令） */
    mov    w4, #1
    mov    w5, #1
    mov    x6, x25
    mov    x29, #332
    bl     read
    mov    x21, #2                 /* 两读皆 GOOD */
    b      8f

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

/* read：w4=LBA、w5=块数（4096B）、x6=缓冲；先清缓冲、READ(10)、UA 重试 ≤2；
 * 只在 GOOD 时返回，其余 x21=122 跳 8。链接寄存器存 x24。 */
read:
    mov    x24, x30
    mov    x22, #0
15: bl     prep
    lsl    w7, w5, #12             /* 字节数 */
    mov    x11, x6
    lsr    w14, w7, #3
16: str    xzr, [x11], #8
    subs   w14, w14, #1
    b.ne   16b
    movz   w10, #0x1400, lsl #16   /* DW0 = 0x14000000 */
    str    w10, [x8]
    movz   w10, #0x0100, lsl #16
    movk   w10, #0x0001            /* DW7 = PRDT off 0x100 | 长 1 */
    str    w10, [x8, #28]
    movz   w10, #0x4001            /* 01 40 00 tag */
    str    w10, [x9]
    rev    w10, w7                 /* exp_len BE32 */
    str    w10, [x9, #12]
    mov    w10, #0x28              /* CDB[0] READ(10) */
    strb   w10, [x9, #16]
    lsr    w10, w4, #24
    strb   w10, [x9, #18]          /* CDB[2..5] LBA BE */
    lsr    w10, w4, #16
    strb   w10, [x9, #19]
    lsr    w10, w4, #8
    strb   w10, [x9, #20]
    strb   w4, [x9, #21]
    lsr    w10, w5, #8
    strb   w10, [x9, #23]          /* CDB[7..8] 块数 BE */
    strb   w5, [x9, #24]
    str    x6, [x9, #0x400]        /* PRDT base */
    sub    w10, w7, #1
    str    w10, [x9, #0x40c]       /* PRDT size = 字节−1 */
    ldr    w10, [x3, #0x60]        /* 旁读 RSR */
    mov    x21, #362
    tbz    w10, #0, 8b
    ldr    w10, [x3, #0x20]        /* 旁读 IS 错误位 0x31804 */
    movz   w11, #0x1804
    movk   w11, #0x0003, lsl #16
    mov    x21, #392
    tst    w10, w11
    b.ne   8b
    bl     issue
    cbz    w0, 17f
    mov    x21, #122
    cmp    w1, #6
    b.ne   8b
    cmp    x22, #2
    b.hs   8b
    add    x22, x22, #1
    b      15b
17: mov    x30, x24
    ret

/* prep（叶子）：清 UCD 0x600 + 数据 8B；UTRD 清零后 DW2=0x0F、DW4=UCD、DW6=0x00800080 */
prep:
    mov    x11, x9
    mov    x14, #0xC0
10: str    xzr, [x11], #8
    subs   x14, x14, #1
    b.ne   10b
    str    xzr, [x25]
    stp    xzr, xzr, [x8]
    stp    xzr, xzr, [x8, #16]
    mov    w10, #0x0F
    str    w10, [x8, #8]
    str    w9, [x8, #16]           /* DW4 = UCD 低 32（0xB0001000），DW5 = 0 */
    movz   w10, #0x0080, lsl #16
    movk   w10, #0x0080
    str    w10, [x8, #24]          /* DW6 = RUO 0x80 | RUL 0x80 */
    ret

/* issue（叶子）：填 tag、敲门铃、等 1s；超时/OCS/byte0 直接跳 8（后备）；
 * 返回 w0=0 GOOD；w0=1 CHECK CONDITION，w1 = sense key（非 0x70 则 0xFF） */
issue:
    strb   w23, [x9, #3]
    add    x23, x23, #1
    dsb    sy
    mov    w10, #1
    str    w10, [x3, #0x58]
    isb
    mrs    x12, cntvct_el0
    add    x12, x12, x20
11: ldr    w10, [x3, #0x58]
    tbz    w10, #0, 12f
    isb
    mrs    x13, cntvct_el0
    cmp    x13, x12
    b.lo   11b
    mov    x21, x29                /* 门铃超时：分命令码 */
    b      8b
12: ldr    w10, [x8, #8]
    and    w10, w10, #0x0F
    mov    x21, #212               /* OCS≠0（后备） */
    cbnz   w10, 8b
    ldrb   w10, [x9, #0x200]
    mov    x21, #242               /* byte0≠0x21（后备） */
    cmp    w10, #0x21
    b.ne   8b
    ldrb   w10, [x9, #0x206]
    mov    w0, #0
    cbz    w10, 13f
    mov    w0, #1
    mov    w1, #0xFF
    ldrb   w10, [x9, #0x222]
    and    w10, w10, #0x7F
    cmp    w10, #0x70
    b.ne   13f
    ldrb   w1, [x9, #0x224]
    and    w1, w1, #0x0F
13: ret
