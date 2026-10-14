/* U1v — 物理层判别件（阶梯 3 刀 1v，2026-10-14）
 *
 * 判据链：u1u（无触发）冻死 → 杀手 ∈ {镜像内 scratch 写、带段布局、
 * CurrentEL/VBAR/SCTLR 系统寄存器写}。daifset 已在 L13/calib 已证活
 * 基线内（读源码核对），排除。
 *
 * 本刀 = u1u 砍掉全部三个系统寄存器块；.vectors/.scratch 段布局与
 * scratch 写原样保留（向量表成死代码占位，保布局等价）：
 *   快闪×3（~1s） = 布局+镜像内写无毒 → 杀手是某个系统寄存器写
 *                    （CurrentEL 读 / VBAR 写 / SCTLR 写 三选一）；
 *   冻死           = 物理层（段布局或镜像内写）→ 下刀拆段布局 vs 写址。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    /* 清旗标（镜像内 scratch，adr 相对=落点无关） */
    adr    x10, scratch_flag
    str    wzr, [x10]

    /* ---- 无系统寄存器块：直落尾巴 ---- */

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

/* 死代码占位（保 .vectors 段布局与 u1u 等价；无 VBAR 安装不会被进） */
net_handler:
    adr    x0, scratch_flag
    mov    w2, #1
    str    w2, [x0]
    b      8f
8:  b      8b

/* 向量表：flat 0x7C0（布局占位）��� */
.section .vectors, "ax"
.globl vectors
vectors:
.rept 16
    b      net_handler
    .space 124
.endr

/* 镜像内 scratch：flat 0x1FC0。 */
.section .scratch, "aw"
.globl scratch_flag
scratch_flag:
    .space 4
