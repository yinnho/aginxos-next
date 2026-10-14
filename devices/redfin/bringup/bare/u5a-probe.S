/* U5A — 写路径首证：boot_a 尾 scratch 块写+回读（阶梯 3→5，2026-10-15）
 *
 * u4j 流程（SSU 唤醒 → GPT 头+条目数组双 CRC32）之后：
 *   ① 在已校验条目数组按名找 "boot_a"（+56 "boot" / +64 0x0061005F "_a\0\0"）；
 *      start=+32、end=+40（高 32 位须 0）；target = start + 16384；须 ≤ end。
 *      地址不硬编码——无校验表/找不到/越界一律不写。期望 start 8712、end 33287、
 *      target 25096（boot_a 内 64MB 处，距 boot_b 8191 块；L0 侧刷前 md5=全零）。
 *   ② 0xB0004000 生成 4K 图样：w0="AGXWRT01"、w1=CNTVCT 戳、w[i≥2]=i·
 *      0x9E3779B97F4A7C15 ⊕ 戳（每轮唯一，L0 侧可逐字复算）。
 *   ③ WRITE(10) 1 块：CDB 2A 08(FUA) LBA BE … 00 01；DW0 0x12000000（CT1、DD
 *      host→device）；UPIU flags 0x20(W)；块数硬编码 1。
 *   ④ READ(10) 回读到 0xB0005000，512 字逐字比。
 * 双边验收：裸件时延码 + L0 侧 dd LBA 25096 宿主逐字复算（裸件写、Linux 读）。
 *
 * 结局等待秒数（CNTVCT）：
 *   写+回读逐字中                      2s
 *   找不到 boot_a                      272s
 *   boot_a 字段越 32 位 / target 越界  92s
 *   写或回读门铃超时                   362s
 *   回读不中                           392s
 *   WRITE CHECK CONDITION sense 7（DATA PROTECT，写保护）  422s
 *   余同 u4j：头 CRC 32/数组 CRC 62/头不健全 92/其它 CHECK CONDITION 122/
 *   SSU 被拒 152/超时 SSU 182·读#1 302·读#2 332/OCS 212/byte0 242
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

    /* ② GPT 头：READ LBA1 1 块 → 0xB0002000 */
30: mov    w4, #1
    mov    w5, #1
    mov    x6, x25
    mov    x29, #302               /* 读 #1 超时码 */
    bl     read
    mov    x21, #92                /* 头不健全 */
    ldr    x27, [x25]
    movz   x28, #0x4645
    movk   x28, #0x2049, lsl #16
    movk   x28, #0x4150, lsl #32
    movk   x28, #0x5452, lsl #48   /* "EFI PART" */
    cmp    x27, x28
    b.ne   8f
    ldr    w15, [x25, #12]         /* HeaderSize */
    cmp    w15, #92
    b.lo   8f
    cmp    w15, #4096
    b.hi   8f
    ldr    w16, [x25, #84]         /* SizeOfEntry */
    cmp    w16, #128
    b.ne   8f
    ldr    w17, [x25, #80]         /* NumberOfEntries */
    cbz    w17, 8f
    cmp    w17, #512
    b.hi   8f
    lsl    w18, w17, #7            /* 数组字节数 N·128 */
    cmp    w18, #4096              /* 0xB0003000 只留 1 块：超出按头不健全 */
    b.hi   8f
    ldr    x19, [x25, #72]         /* PartitionEntryLBA */
    lsr    x10, x19, #32
    cbnz   x10, 8f
    cbz    w19, 8f

    /* ③ 头 CRC32 */
    ldr    w26, [x25, #16]
    str    wzr, [x25, #16]
    mov    x0, x25
    mov    w1, w15
    bl     crc
    mov    x21, #32                /* 头 CRC 不中 */
    cmp    w2, w26
    b.ne   8f

    /* ④ 条目数组：READ LBA=elba、块数=ceil(len/4096) → 0xB0003000 */
    ldr    w26, [x25, #88]         /* PartitionEntryArrayCRC32 */
    add    w5, w18, #0xFFF
    lsr    w5, w5, #12
    mov    w4, w19
    movz   x6, #0xB000, lsl #16
    movk   x6, #0x3000             /* 读 #2 缓冲 0xB0003000（唯一行为变量） */
    mov    x29, #332               /* 读 #2 超时码 */
    bl     read
    mov    x0, x6
    mov    w1, w18
    bl     crc
    mov    x21, #62                /* 数组 CRC 不中 */
    cmp    w2, w26
    b.ne   8f
    /* ⑤ 按名找 boot_a（x6=已校验条目数组 0xB0003000） */
    ldr    w1, [x25, #80]          /* N（crc 踩了 w17，从头缓冲重取） */
    mov    x0, x6
    movz   x27, #0x0062
    movk   x27, #0x006F, lsl #16
    movk   x27, #0x006F, lsl #32
    movk   x27, #0x0074, lsl #48   /* "boot" UTF-16LE */
    movz   x28, #0x005F
    movk   x28, #0x0061, lsl #16   /* "_a\0\0" UTF-16LE */
22: ldr    x10, [x0, #56]
    cmp    x10, x27
    b.ne   23f
    ldr    x10, [x0, #64]
    cmp    x10, x28
    b.eq   24f
23: add    x0, x0, #128
    subs   w1, w1, #1
    b.ne   22b
    mov    x21, #272               /* 找不到 boot_a */
    b      8f
24: ldr    x10, [x0, #32]          /* StartingLBA */
    ldr    x11, [x0, #40]          /* EndingLBA */
    mov    x21, #92
    orr    x12, x10, x11
    lsr    x12, x12, #32
    cbnz   x12, 8f                 /* 越 32 位 */
    mov    x27, #16384
    add    x27, x10, x27           /* target = start + 16384 */
    cmp    x27, x11
    b.hi   8f                      /* target > end → 不写 */

    /* ⑥ 图样 → 0xB0004000：w0 magic、w1 戳、w[i≥2]=i·K ⊕ 戳 */
    movz   x6, #0xB000, lsl #16
    movk   x6, #0x4000
    isb
    mrs    x19, cntvct_el0         /* 戳 */
    movz   x10, #0x4741
    movk   x10, #0x5758, lsl #16
    movk   x10, #0x5452, lsl #32
    movk   x10, #0x3130, lsl #48   /* "AGXWRT01" */
    str    x10, [x6]
    str    x19, [x6, #8]
    movz   x12, #0x7C15
    movk   x12, #0x7F4A, lsl #16
    movk   x12, #0x79B9, lsl #32
    movk   x12, #0x9E37, lsl #48   /* K */
    mov    x11, #2
54: mul    x13, x11, x12
    eor    x13, x13, x19
    str    x13, [x6, x11, lsl #3]
    add    x11, x11, #1
    cmp    x11, #512
    b.lo   54b

    /* ⑦ WRITE(10) 1 块 → target */
    mov    w4, w27
    mov    w5, #1
    mov    x29, #362               /* 写超时码 */
    bl     write
    /* ⑧ 回读 → 0xB0005000 */
    mov    w4, w27
    mov    w5, #1
    movz   x6, #0xB000, lsl #16
    movk   x6, #0x5000
    mov    x29, #362               /* 回读超时码 */
    bl     read
    /* ⑨ 512 字逐字比 */
    movz   x0, #0xB000, lsl #16
    movk   x0, #0x4000
    mov    x1, x6
    mov    x2, #512
    mov    x21, #392               /* 回读不中 */
55: ldr    x10, [x0], #8
    ldr    x11, [x1], #8
    cmp    x10, x11
    b.ne   8f
    subs   x2, x2, #1
    b.ne   55b
    mov    x21, #2                 /* 过：写+回读逐字中 */
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

/* write：w4=LBA、w5=块数、x6=源缓冲（不清）；WRITE(10) FUA；UA 重试 ≤2；
 * sense 7 DATA PROTECT → x21=422；其它 CHECK CONDITION → 122。返址存 x24。 */
write:
    mov    x24, x30
    mov    x22, #0
50: bl     prep
    lsl    w7, w5, #12             /* 字节数 */
    movz   w10, #0x1200, lsl #16   /* DW0 = 0x12000000（CT1、DD host→device） */
    str    w10, [x8]
    movz   w10, #0x0100, lsl #16
    movk   w10, #0x0001            /* DW7 = PRDT off 0x100 | 长 1 */
    str    w10, [x8, #28]
    movz   w10, #0x2001            /* 01 20 00 tag（flags W） */
    str    w10, [x9]
    rev    w10, w7                 /* exp_len BE32 */
    str    w10, [x9, #12]
    mov    w10, #0x2A              /* CDB[0] WRITE(10) */
    strb   w10, [x9, #16]
    mov    w10, #0x08              /* CDB[1] FUA */
    strb   w10, [x9, #17]
    lsr    w10, w4, #24
    strb   w10, [x9, #18]          /* CDB[2..5] LBA BE */
    lsr    w10, w4, #16
    strb   w10, [x9, #19]
    lsr    w10, w4, #8
    strb   w10, [x9, #20]
    strb   w4, [x9, #21]
    mov    w10, #1
    strb   w10, [x9, #24]          /* CDB[7..8] 块数=1（硬编码） */
    str    x6, [x9, #0x400]        /* PRDT base */
    sub    w10, w7, #1
    str    w10, [x9, #0x40c]       /* PRDT size = 字节−1 */
    bl     issue
    cbz    w0, 52f
    mov    x21, #422
    cmp    w1, #7
    b.eq   8b                      /* DATA PROTECT */
    mov    x21, #122
    cmp    w1, #6
    b.ne   8b
    cmp    x22, #2
    b.hs   8b
    add    x22, x22, #1
    b      50b
52: mov    x30, x24
    ret

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

/* crc（叶子）：x0=ptr、x1=len → w2=CRC32（反射 0xEDB88320）。逐字节 ldrb。 */
crc:
    mov    w2, #-1
    movz   w13, #0x8320
    movk   w13, #0xEDB8, lsl #16   /* 多项式放 w13——x3 是 HCI 基址，禁写 */
18: cbz    x1, 21f
    ldrb   w15, [x0], #1
    eor    w2, w2, w15
    mov    w16, #8
19: and    w17, w2, #1
    lsr    w2, w2, #1
    cbz    w17, 20f
    eor    w2, w2, w13
20: subs   w16, w16, #1
    b.ne   19b
    sub    x1, x1, #1
    b      18b
21: mvn    w2, w2
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
