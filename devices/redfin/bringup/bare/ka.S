/* ka.S —— 阶梯 2 在役裸 substrate v2：+ 阶梯 3 存储自检常驻（2026-10-14 深夜）
 *
 * redfin 整机在役态正身：无 Linux 内核。v2 开机序：
 *   ① UFS 存储自检（u4j 契约，TRUTH §7 末条）：UTRLBA/RSR → SSU ACTIVE →
 *      W-LUN 0xD0 唤醒（bootloader 交接留 PowerDown）→ 读 LUN0 LBA1 GPT 头
 *      （magic+健全+头 CRC32）→ 按头读条目数组（数组 CRC32）。
 *      过 → ka_main；败 → 时延编码复位（等 x21 秒后 PS_HOLD，3 轮落 b 槽 L0
 *      自报；码表同 u4j：头 CRC 32/数组 CRC 62/头不健全 92/READ CHECK
 *      CONDITION 122/SSU 被拒 152/超时 SSU 182·读#1 302·读#2 332/OCS 212/
 *      byte0 242）。
 *   ② ka_main（v1 原样）：owner-gated 选 CHGR(0x210) APPS 副本 → 充电使能
 *      RMW 0x1042 bit0 → forever { 喂狗 0x1643←1；delay 3e9 }，**不复位**。
 * 稳态 = Google logo 静止 = SPMI 充电+喂狗+存储自检全过；早期败=复位落 L0。
 *
 * 寄存器两世界不重叠：① 段 x3=UFSHCI 基、x8 UTRD、x9 UCD、x20 CNTFRQ、
 * x23 tag、x25 缓冲、x29 超时码、x24 返址（子程序禁写，见 TRUTH 全局寄存器
 * 律）；② 段重设 x3=CORE x4=CHNLS x5=OBSRVR x19=CNFG x7=apid_chgr。
 * v2 修：ka_main 先 x7=-1——v1 无 APPS 副本时 x7 未初始化（ABL 残值侥幸无害，
 * ① 段之后必为正数垃圾，会以野 APID 喂狗）。
 * DRAM：① 段用 0xB0000000 起 16KB（uefiplat "Kernel Expanded" SYS_MEM）；
 * ② 段零 DRAM 写（承 v1）。
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
20: bl     ufs_prep
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
    bl     ufs_issue
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
    bl     ufs_read
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
    bl     ufs_crc
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
    bl     ufs_read
    mov    x0, x6
    mov    w1, w18
    bl     ufs_crc
    mov    x21, #62                /* 数组 CRC 不中 */
    cmp    w2, w26
    b.ne   8f
    b      ka_main                 /* 存储自检过 → 常驻 substrate */

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


/* ===== ② 常驻 substrate（v1 原样，+ x7 初始化） ===== */
ka_main:
    mov    x7, #-1                   /* v2 修：apid_chgr 缺省=无 */
    movz   x3, #0x0c44, lsl #16      /* SPMI_CORE  0x0c440000 */
    movz   x4, #0x0c60, lsl #16      /* SPMI_CHNLS 0x0c600000 */
    movz   x5, #0x0e60, lsl #16      /* SPMI_OBSRVR 0x0e600000 */
    movz   x19, #0xa000
    movk   x19, #0x0c40, lsl #16     /* CNFG       0x0c40a000 */

    movz   x13, #0x210               /* PPID_CHGR */
    bl     apid_own
    cmp    x14, #0
    b.lt   2f                        /* 无 APPS 副本 → 直入喂狗环 */

    /* —— 充电使能 RMW 0x1042 bit0（一次）—— */
    mov    x7, x14
    mov    x15, #0x1042
    bl     spmi_rd1
    cbnz   x17, 2f                   /* 读败 → 跳过（环仍活）*/
    orr    x16, x16, #1
    mov    x14, x7
    mov    x15, #0x1042
    bl     spmi_wr1

    /* —— 永续喂狗环 —— */
2:  cmp    x7, #0
    b.lt   3f
    mov    x14, x7
    mov    x15, #0x1643
    mov    x16, #1
    bl     spmi_wr1                  /* 喂狗（败也不出环）*/
3:  movz   x12, #0x5E00
    movk   x12, #0xB2D0, lsl #16     /* 3e9 延时 */
    bl     delay
    b      2b

/* apid_own: 入 x13=ppid → 出 x14=owner==0 的 apid（-1=无）*/
apid_own:
    mov    x14, #0
    add    x9, x3, #0x900
1:  cmp    x14, #512
    b.hs   2f
    ldr    w10, [x9, x14, lsl #2]
    cbz    w10, 3f
    lsr    w11, w10, #8
    and    x11, x11, #0xfff
    cmp    x11, x13
    b.ne   3f
    mov    x12, #0x700
    add    x12, x12, x14, lsl #2
    ldr    w15, [x19, x12]
    ands   w15, w15, #7
    b.eq   4f
3:  add    x14, x14, #1
    b      1b
2:  mov    x14, #-1
4:  ret

/* spmi_wr1: 入 x14=apid x15=reg x16=字节 → 出 x17(0=成) */
spmi_wr1:
    lsl    x10, x14, #16
    add    x9, x4, x10
    str    w16, [x9, #0x10]
    mov    x10, #14
    lsl    x10, x10, #27
    and    x11, x15, #0xff
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16
1:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 3f
    subs   x12, x12, #1
    b.ne   1b
    mov    x17, #-2
    ret
3:  and    x17, x11, #0xe
    ret

/* spmi_rd1: 入 x14=apid x15=reg → 出 x17(0=成) x16=字节 */
spmi_rd1:
    lsl    x10, x14, #7
    add    x9, x5, x10
    mov    x10, #15
    lsl    x10, x10, #27
    and    x11, x15, #0xff
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16
1:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 3f
    subs   x12, x12, #1
    b.ne   1b
    mov    x17, #-2
    ret
3:  ands   x17, x11, #0xe
    b.ne   4f
    ldr    x16, [x9, #0x18]
4:  ret

/* delay: 入 x12=迭代数 */
delay:
    cbz    x12, 2f
1:  subs   x12, x12, #1
    b.ne   1b
2:  ret

/* ===== ① 段子程序（u4j） ===== */
/* ufs_read：w4=LBA、w5=块数（4096B）、x6=缓冲；先清缓冲、READ(10)、UA 重试 ≤2；
 * 只在 GOOD 时返回，其余 x21=122 跳 8。链接寄存器存 x24。 */
ufs_read:
    mov    x24, x30
    mov    x22, #0
15: bl     ufs_prep
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
    bl     ufs_issue
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

/* ufs_crc（叶子）：x0=ptr、x1=len → w2=CRC32（反射 0xEDB88320）。逐字节 ldrb。 */
ufs_crc:
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

/* ufs_prep（叶子）：清 UCD 0x600 + 数据 8B；UTRD 清零后 DW2=0x0F、DW4=UCD、DW6=0x00800080 */
ufs_prep:
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

/* ufs_issue（叶子）：填 tag、敲门铃、等 1s；超时/OCS/byte0 直接跳 8（后备）；
 * 返回 w0=0 GOOD；w0=1 CHECK CONDITION，w1 = sense key（非 0x70 则 0xFF） */
ufs_issue:
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
