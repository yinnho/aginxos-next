/* U1y — sysreg 块判别件（阶梯 3 刀 1y，2026-10-14）
 *
 * 矩阵缺格：{CurrentEL 读+三分支 VBAR 写+isb, 无触发, 无 DRAM 写}。
 * u1w（本块+udf）冻死 ⇒ 网没回来；死因二选一：本块本身死，或
 * udf 异常进入被路由到垃圾（EL1 被路由此 EL2 的陈旧 VBAR → 围栏）。
 * u1v（无本块, 仅 str）也冻死 ⇒ DRAM store 独立成 killer。
 * 本刀 = u1w 砍触发：块活则快闪且顺手读出 EL。
 *
 * 判读（闪长 = EL 编码，每轮数秒数）：
 *   blink ~1s×3 = EL1：sysreg 块清白 → u1w 冻死=异常进入/路由死
 *                    （T3），网线按「不可用」设计后续刀；
 *   blink ~2s×3 = EL2：同上且 EL=EL2（MMU 寄存器面用 EL2 套！）；
 *   blink ~3s×3 = EL3：同上且 EL=EL3；
 *   冻死        = CurrentEL 读或 VBAR 写本身死 → 下刀 calib+mrs 单件。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    /* CurrentEL 三分支装 VBAR；x19 = EL号-1（0/1/2 = EL1/2/3） */
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

    /* 闪长 = EL 编码：1e8×EL（calib 已定标 ~1s/1e8） */
    movz   x13, #0xE100
    movk   x13, #0x05F5, lsl #16    /* 1e8 */
    add    x20, x19, #1
    mul    x12, x13, x20
4:  subs   x12, x12, #1
    b.ne   4b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]                /* PS_HOLD ← 0（AOSS，L13 已证活） */
5:  b      5b

/* 死代码占位（无触发不进；保 .text 布局与 u1w 大体等价） */
net_handler:
    movz   w23, #1
6:  b      6b

/* 向量表：flat 0x7C0 ⇒ 运行时基+0x800（2KB 对齐律；u1w 同布局）。 */
.section .vectors, "ax"
.globl vectors
vectors:
.rept 16
    b      net_handler
    .space 124
.endr
