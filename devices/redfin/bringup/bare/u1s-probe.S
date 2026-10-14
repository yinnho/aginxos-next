/* U1s — 网最小自检（阶梯 3 刀 1s，2026-10-14）
 *
 * 判据链：U1n v2（页表修对 TTBR0/attr 后）仍冻死——骨架里还有第三断点，
 * 且 u1n v2 无任何外域访问，断点必在骨架自身。两嫌疑：
 *   ① scratch（旗标 0x80018000/页表 0x80020000）全在 live.dts
 *      hyp_region@80000000（6MB no-map）内——若 QHEE 驻留，清 8KB
 *      页表=踩管理程序内存。Linux head.S 的页表建在自己镜像里
 *      （adr 相对），从不往镜像外 scratch——本刀照抄该正典；
 *   ② EL3 入场未覆盖（CurrentEL 只判 EL1/EL2；真在 EL3 则 udf 进
 *      VBAR_EL3=残留垃圾→冻死）。
 *
 * 本刀 = 最小骨架单变量测网：
 *   MMU-off、零页表、零 MAIR/TCR/TTBR/SCTLR 写、零 daifclr、零外域访问；
 *   scratch（旗标）在镜像内 flat 0x1FC0（adr 定位，落点无关）；
 *   VBAR 按 CurrentEL 三分支（EL1/EL2/EL3，x19=EL号-1）；
 *   udf #0 必同步 abort → 十六槽向量全进 net_handler → 置旗标+ELR+4+eret。
 *
 * 判读：
 *   长闪×3（~10s） = 网根上活：VBAR 安装、向量取指、handler、eret、
 *                    旗标、闪长编码全链证毕（MMU-off 制度）→ 下一刀
 *                    u1n v3（本骨架+镜像内页表+MMU）；
 *   快闪×3（~1s）  = 异常：udf 没炸或没进网；
 *   冻死           = 网根上坏（EL3 之外的环境 surprise / 向量取指死 /
 *                    eret 落错）→ 拆 VBAR-install 与 udf 两件分测。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    /* 清旗标（镜像内 scratch，adr 相对=落点无关；不再碰 hyp_region） */
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

    /* ---- 网自检：必触发同步 abort（MMU-off、零外域）---- */
    udf    #0

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
 * clobber x0/x2/x6——主流程后段只用 x9/x10/x12/x19/x22。 */
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
