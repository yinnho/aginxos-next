/* U1m — MMU-on 恒等映射探针（阶梯 3 刀 1m v2，2026-10-14）
 *
 * 判据链：U1a/U1c/U1d 冻死（UFS@0x1d84000、GCC@0x1077018；AOSS 岛
 * PS_HOLD/SPMI 全活）。U1n v1（同骨架+udf 标定）也冻死 → 骨架自病，
 * U1m v1「MMU-on 仍 stall」作废。v2 修两错后重测。
 *
 * 剩余主假说：MMU-off 制度即病——Linux 碰外域恒在恒等映射+设备属性
 * (nGnRE)下；arm64 启动协议要求 bootloader 交棒时 MMU 关，而内核
 * head.S 开 MMU 前零 MMIO——「MMU-off + 外域读」这个组合在全链路
 * 健康史上从未发生过。事务属性（shareability/ARCACHE 侧带）在
 * MMU-off 下由核心硬定，可能正是 NoC 不认的那种。
 *
 * 本刀 = U1f 全套（VBAR 网+旗标闪长编码）+ 恒等映射：
 *   单级 L1@0x80021000 四个 1GB 块；[0,1)GB=[1,2)GB=Device-nGnRE
 *   （idx0=0x04），[2,4)GB=Normal WB（idx1=0xFF，代码/DRAM 在此）；
 *   MAIR/TCR/TTBR0/SCTLR.M 按现行 EL 写 el1/el2 两套（CurrentEL
 *   判别，x19 记 EL2 位）。T0SZ=25 → 39 位 VA，硬件从 L1 起步走表。
 *
 * 【v1 两错账（冻死根因，2026-10-14 复算；U1n v2 先标定后本刀）】
 *   ① TTBR0=0x80020000(L0)+T0SZ=25 自相矛盾：39 位 VA 从 L1 起步，
 *     TTBR0 必须直指 L1——v1 让硬件把 L0 当 L1、顺 L0[0] 表描述符
 *     再降一级把我们 L1 当 L2 查 → 全零区 → 一切访问（含取向量）
 *     翻译错 → 核挂死无证；
 *   ② attrindx 位[4:2] 算错：0x405=idx1(WB)、0x409=idx2(MAIR 未
 *     定义 0x00)——外域被标 WB、DRAM 被标废档。正解 device=0x401
 *     (idx0)、normal=0x405(idx1)。
 *   另：daifclr #0x8 挪到被测指令前一刻（同步 abort 本就不受 DAIF
 *   掩，迟到无损失；躲 ABL 遗留 SError 在建表期引爆的窗口）。
 *
 * 判读（U1n v2 标定长闪通过后才有效）：
 *   快闪×3（~1s）  = MMU-on 下读通（旗标 0）→ 制度病坐实，u 梯全部
 *                    转入 MMU-on 制度继续爬（U2 起重造在骨架上）；
 *   长闪×3（~10s） = MMU-on 下读 abort 被网住 → 事务有响应（错误），
 *                    转防火墙/时钟态刀；
 *   冻死           = MMU-on 下仍 stall → 块/域电源问题（ABL 交棒时
 *                    已被 RPM 掐电？），转电源域刀。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    /* 清旗标（DRAM，sp.S 已证 MMU-off 写活） */
    movz   x10, #0x8001, lsl #16
    movk   x10, #0x8000             /* 0x80018000 */
    str    wzr, [x10]

    /* EL 记位 + VBAR 网（同 U1f） */
    mrs    x0, CurrentEL
    cmp    x0, #0x8
    cset   x19, eq                  /* x19=1 → EL2 */
    adr    x1, vectors
    cbz    x19, 20f
    msr    vbar_el2, x1
    b      21f
20: msr    vbar_el1, x1
21: isb

    /* ---- 页表：单级 L1@0x80021000（T0SZ=25 → 硬件从 L1 起步）---- */
    movz   x1, #0x8002, lsl #16     /* 0x80020000（清零含 L0 历史区） */
    mov    x2, x1
    add    x3, x1, #0x2000
1:  stp    xzr, xzr, [x2], #16      /* 清 8KB */
    cmp    x2, x3
    b.lo   1b
    mov    w5, #0x401               /* L1[0] = device 块（idx0=nGnRE） */
    str    w5, [x1, #0x1000]
    movz   w6, #0x0401
    movk   w6, #0x4000, lsl #16     /* L1[1] = 0x40000401 device */
    str    w6, [x1, #0x1004]
    movz   w7, #0x0405
    movk   w7, #0x8000, lsl #16     /* L1[2] = 0x80000405 normal WB */
    str    w7, [x1, #0x1008]
    movz   w8, #0x0405
    movk   w8, #0xC000, lsl #16     /* L1[3] = 0xC0000405 normal WB */
    str    w8, [x1, #0x100C]
    dsb    sy

    /* ---- 按 EL 装 MAIR/TCR/TTBR0 ---- */
    movz   x5, #0xFF04              /* MAIR idx0=0x04(nGnRE) idx1=0xFF(WB) */
    cbz    x19, 22f
    msr    mair_el2, x5
    movz   x5, #0x0019              /* TCR_EL2: PS=40b|T0SZ=25 */
    movk   x5, #0x0002, lsl #16
    msr    tcr_el2, x5
    movz   x5, #0x8002, lsl #16
    movk   x5, #0x1000              /* TTBR0=0x80021000 直指 L1（起步层） */
    msr    ttbr0_el2, x5
    b      23f
22: msr    mair_el1, x5
    movz   x5, #0x0519              /* TCR_EL1: IPS=40b|WB|T0SZ=25 */
    movk   x5, #0x0002, lsl #32
    msr    tcr_el1, x5
    movz   x5, #0x8002, lsl #16
    movk   x5, #0x1000              /* TTBR0=0x80021000 直指 L1（起步层） */
    msr    ttbr0_el1, x5
23: isb

    /* ---- 开 MMU：清 I(12)/C(2)/A(1) 置 M(0)（掩码经寄存器） ---- */
    movz   x28, #0x1006
    cbz    x19, 24f
    mrs    x5, sctlr_el2
    bic    x5, x5, x28
    orr    x5, x5, #1
    msr    sctlr_el2, x5
    b      25f
24: mrs    x5, sctlr_el1
    bic    x5, x5, x28
    orr    x5, x5, #1
    msr    sctlr_el1, x5
25: isb

    /* ---- U1a/U1f 同地址：UFSHCI CAP（现于 nGnRE 属性下）---- */
    msr    daifclr, #0x8            /* SError 迟解掩（躲遗留错建表期引爆） */
    isb
    movz   x3, #0x01d8, lsl #16
    movk   x3, #0x4000
    ldr    w20, [x3]

    /* 旗标定闪长：0=快(1e8) 1=长(1e9) */
    ldr    w22, [x10]
    cbnz   w22, 4f
    movz   x12, #0xE100
    movk   x12, #0x05F5, lsl #16
    b      5f
4:  movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16
5:  subs   x12, x12, #1
    b.ne   5b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]                /* PS_HOLD ← 0（AOSS，映射为 device） */
6:  b      6b

/* 异常网 handler（同 U1f）：置旗标 + ELR+4 + eret。
 * clobber x0/x2/x5/x6——主流程后段只用 x9/x10/x12/x19/x22。 */
net_handler:
    movz   x0, #0x8001, lsl #16
    movk   x0, #0x8000
    mov    w2, #1
    str    w2, [x0]
    cbz    x19, 7f
    mrs    x6, elr_el2
    add    x6, x6, #4
    msr    elr_el2, x6
    b      8f
7:  mrs    x6, elr_el1
    add    x6, x6, #4
    msr    elr_el1, x6
8:  eret

/* 向量表：flat 固定 0x7C0（.ld 定址）⇒ 运行时=装载基+0x40+0x7C0=
 * 装载基+0x800 ⇒ 任意 2KB 对齐装载基下满足 VBAR 2KB 对齐律。
 * 【对齐律教训】v1 链 ALIGN(0x800)=flat 0x800 → 运行时 0x80008840，
 * VBAR 硬抹低 11 位 → 向量基 0x80008000 → 异常跳进 flat 0x1C0 随机
 * 指令流冻死，网失效无证。
 * 段内禁 .align 11（会把内容顶回 0x800 重蹈覆辙）。 */
.section .vectors, "ax"
.globl vectors
vectors:
.rept 16
    b      net_handler
    .space 124
.endr
