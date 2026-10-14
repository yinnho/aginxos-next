/* U1t — 对齐触发网自检（阶梯 3 刀 1t，2026-10-14）
 *
 * 判据链：u1s（最小网件：MMU-off+镜像内 scratch+三分支 VBAR+udf）
 * 也冻死——断点在异常路径本身。msr VBAR 架构上不可陷阱不可挂
 * （ARMv8.2 无 FGT，VBAR 无 trap 位），主嫌 = 异常进入。
 *
 * 本刀 = u1s 同骨架，触发器换成对齐错：
 *   SCTLR.A=1（按 EL 写 el1/el2/el3 三套）→ 读 scratch+1（1 mod 4
 *   的字读）→ 必同步 alignment abort——零总线访问（不存在 stall
 *   混淆）、非外部错（SCR_EL3.EA 路由碰不到）、与数据 abort 同
 *   向量槽（current-EL SPx sync=+0x200）同进入机构。
 *
 * 判读：
 *   长闪×3（~10s） = 网以真 abort 类证毕（进入+handler+eret+旗标
 *                    全链）→ u1n v3（本骨架+镜像内页表+MMU）上位；
 *   快闪×3（~1s）  = 对齐 abort 没触发（A 位没生效——异常）；
 *   冻死           = 异常进入本身在此环境死（udf/对齐同死 → 类无关
 *                    → 路由/环境问题）→ 下刀 u1u（零触发件）定
 *                    VBAR 安装毒性，网线若死则转无网判读制度。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    /* 清旗标（镜像内 scratch，adr 相对=落点无关） */
    adr    x10, scratch_flag
    str    wzr, [x10]

    /* CurrentEL 三分支装 VBAR；x19 = EL号-1（0/1/2 = EL1/EL2/EL3） */
    mrs    x0, CurrentEL
    lsr    x19, x0, #2
    sub    x19, x19, #1
    adr    x1, vectors
    cbz    x19, 1f                  /* EL1 */
    cmp    x19, #1
    b.eq   2f                       /* EL2 */
    msr    vbar_el3, x1             /* EL3 */
    b      3f
1:  msr    vbar_el1, x1
    b      3f
2:  msr    vbar_el2, x1
3:  isb

    /* ---- SCTLR.A=1（严格对齐检查，按 EL 三套）---- */
    cbz    x19, 11f
    cmp    x19, #1
    b.eq   12f
    mrs    x5, sctlr_el3
    orr    x5, x5, #2
    msr    sctlr_el3, x5
    b      13f
11: mrs    x5, sctlr_el1
    orr    x5, x5, #2
    msr    sctlr_el1, x5
    b      13f
12: mrs    x5, sctlr_el2
    orr    x5, x5, #2
    msr    sctlr_el2, x5
13: isb

    /* ---- 触发：scratch+1 的字读（1 mod 4）→ 必对齐 abort ---- */
    adr    x3, scratch_flag
    add    x3, x3, #1
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
    str    wzr, [x9]                /* PS_HOLD ← 0（AOSS，L13 已证活） */
6:  b      6b

/* 异常网 handler：置旗标 + ELR+4 + eret（三分支 ELR）。
 * clobber x0/x2/x5/x6——主流程后段只用 x9/x10/x12/x19/x20/x22。 */
net_handler:
    adr    x0, scratch_flag
    mov    w2, #1
    str    w2, [x0]
    cbz    x19, 7f
    cmp    x19, #1
    b.eq   71f
    mrs    x6, elr_el3
    add    x6, x6, #4
    msr    elr_el3, x6
    b      8f
71: mrs    x6, elr_el2
    add    x6, x6, #4
    msr    elr_el2, x6
    b      8f
7:  mrs    x6, elr_el1
    add    x6, x6, #4
    msr    elr_el1, x6
8:  eret

/* 向量表：flat 固定 0x7C0（.ld 定址）⇒ 运行时=装载基+0x40+0x7C0=
 * 装载基+0x800 ⇒ 任意 2KB 对齐装载基下满足 VBAR 2KB 对齐律。
 * 段内禁 .align 11（会把内容顶回 0x800 违反对齐律）。 */
.section .vectors, "ax"
.globl vectors
vectors:
.rept 16
    b      net_handler
    .space 124
.endr

/* 镜像内 scratch：flat 0x1FC0（运行时=装载基+0x2000，随双入口约定
 * 平移 0x80000，皆落在本镜像 DRAM 足迹内）。 */
.section .scratch, "aw"
.globl scratch_flag
scratch_flag:
    .space 4
