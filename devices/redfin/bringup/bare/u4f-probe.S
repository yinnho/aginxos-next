/* U4F — 无内核读盘首证：READ(10) LUN0 LBA1 → 「EFI PART」magic 门（阶梯 3，2026-10-14 夜）
 *
 * u3j 过（SSU ACTIVE→W-LUN 0xD0 唤醒后 READ CAPACITY 数据非零）。u4f 唤醒序
 * 同 u3j，把 READ CAPACITY 换成 READ(10)：
 *   CDB 28 00 00 00 00 01 00 00 01 00（LBA1、1 块）；LUN0 块=4096B（L0 侧 sda
 *   logical_block_size=4096）→ exp_len BE32 4096（rev 通式）、PRDT size 0xFFF；
 *   数据缓冲 0xB0002000 先清 4096B；DW0 0x14000000、DW7 0x01000001、头 01 40 00 tag。
 *   判据：缓冲首 8 字节 == "EFI PART"（LE u64 0x5452415020494645，GPT 头）。
 *
 * 结局等待秒数（CNTVCT）：
 *   首 8 字节 == "EFI PART"            2s   ← 读盘首证
 *   READ 重试后仍 UA                  32s
 *   READ NOT READY                    62s
 *   READ ILLEGAL REQUEST              92s
 *   READ 其它 key / sense 非 0x70     122s
 *   SSU 被拒（非 UA）                 152s
 *   后备：门铃 1s 超时 182 / OCS≠0 212 / byte0≠0x21 242 / GOOD 但全零 272 /
 *         GOOD 非零但非 magic 302
 *   仍冻 = CPU 中止
 * 判读（3 轮 + 基线 78–103s）：2→84–109 / 32→174–199 / … / 272→894–919 /
 * 302→984–1009s。
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
    bl     issue
    cbz    w0, 30f                 /* GOOD → 读 */
    mov    x21, #152               /* SSU 被拒 */
    cmp    w1, #6
    b.ne   8f
    cbnz   x22, 8f
    mov    x22, #1
    b      20b

    /* ② READ(10) LUN0 LBA1 1 块（4096B），UA 重试至多 2 次 */
30: mov    x22, #0
31: bl     prep
    mov    x11, x25                /* 清数据缓冲 4096B */
    mov    x14, #0x200
14: str    xzr, [x11], #8
    subs   x14, x14, #1
    b.ne   14b
    movz   w10, #0x1400, lsl #16   /* DW0 = 0x14000000 */
    str    w10, [x8]
    movz   w10, #0x0100, lsl #16
    movk   w10, #0x0001            /* DW7 = PRDT off 0x100 | 长 1 */
    str    w10, [x8, #28]
    movz   w10, #0x4001            /* LE 0x00004001 → 01 40 00 00 */
    str    w10, [x9]
    mov    w10, #4096
    rev    w10, w10                /* exp_len BE32 = 4096 */
    str    w10, [x9, #12]
    mov    w10, #0x28              /* CDB[0] = READ(10) */
    strb   w10, [x9, #16]
    mov    w10, #1
    strb   w10, [x9, #21]          /* CDB[5] = LBA 低字节 = 1（CDB[2..5] BE） */
    strb   w10, [x9, #24]          /* CDB[8] = 块数低字节 = 1（CDB[7..8] BE） */
    str    x25, [x9, #0x400]       /* PRDT：base */
    mov    w10, #0xFFF
    str    w10, [x9, #0x40c]       /* size = 4096−1 */
    bl     issue
    cbnz   w0, 40f
    ldr    x27, [x25]              /* GOOD：magic 门 */
    movz   x28, #0x4645
    movk   x28, #0x2049, lsl #16
    movk   x28, #0x4150, lsl #32
    movk   x28, #0x5452, lsl #48   /* "EFI PART" LE = 0x5452415020494645 */
    mov    x21, #2                 /* 过：读盘首证 */
    cmp    x27, x28
    b.eq   8f
    ldr    x28, [x25, #8]
    orr    x27, x27, x28
    mov    x21, #272               /* 全零（首 16B） */
    cbz    x27, 8f
    mov    x21, #302               /* 非零但非 magic */
    b      8f
40: cmp    w1, #6
    b.ne   41f
    mov    x21, #32                /* UA 重试耗尽 */
    cmp    x22, #2
    b.hs   8f
    add    x22, x22, #1
    b      31b
41: mov    x21, #62                /* NOT READY */
    cmp    w1, #2
    b.eq   8f
    mov    x21, #92                /* ILLEGAL REQUEST */
    cmp    w1, #5
    b.eq   8f
    mov    x21, #122               /* 其它 key / 非 0x70 */
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
    mov    x21, #182               /* 门铃超时（后备） */
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
