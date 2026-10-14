/* U1f — VBAR 网 + SError 解掩探针（阶梯 3 刀 1f，2026-10-14）
 *
 * U1a（读 0x1d84000 无网）冻死；U1c/U1d（GCC@0x100000）同。u 系至今
 * 全部裸奔：VBAR 未装、daifset #0xf 把 SError（A 位）也掩死。假说：
 * 外域读不是「无响应 stall」而是 DECERR/SLVERR 错误响应 → External
 * Abort（同步或 SError）→ 裸件异常无处去 → 冻死无证。
 *
 * 本刀 = U1a 同一地址（UFS CAP@0x1d84000）+ 三件仪表：
 *   1) 装 VBAR（按 CurrentEL 选 vbar_el1/vbar_el2；表址用 adr 取，
 *      落点无关——两种 ABL 装载约定下都对；装载基页对齐+flat 0x800
 *      偏移 ⇒ 运行时表自动 2KB 对齐）；
 *   2) 解掩 SError（daifclr #0x8）——掩着的话错误响应挂起在 load 上
 *      即冻（DAIF.A 位）；
 *   3) handler=置旗标 0x80018000=1 + ELR+4 跳过 + eret（IRQ/FIQ 槽
 *      防御性同款；十六槽全走一个 handler）。
 *
 * 判读（闪长编码，肉眼分辨）：
 *   快闪×3（~1s/次） = 读干净完成（旗标 0）→ U1a 冻死另有其因；
 *   长闪×3（~10s/次）= 异常被网住（旗标 1）→ 块有错误响应：防火墙/
 *                       时钟态问题，非总线死——u 系可带着网继续爬；
 *   冻死             = 真 stall（连错误响应都没有）→ MMU-off 事务
 *                       属性刀（恒等映射）上位。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    /* 清旗标（DRAM 落点，sp.S 已证 MMU-off 写活） */
    movz   x10, #0x8001, lsl #16
    movk   x10, #0x8000             /* 0x80018000 */
    str    wzr, [x10]

    /* CurrentEL → 选 VBAR_ELx（EL1=0b0100 EL2=0b1000，位[3:2]） */
    mrs    x0, CurrentEL
    adr    x1, vectors
    cmp    x0, #0x8
    b.eq   2f
    msr    vbar_el1, x1
    b      3f
2:  msr    vbar_el2, x1
3:  isb
    /* 解掩 SError（A 位）；IRQ/FIQ 保持掩 */
    msr    daifclr, #0x8
    isb

    /* U1a 同地址：UFSHCI CAP */
    movz   x3, #0x01d8, lsl #16
    movk   x3, #0x4000
    ldr    w20, [x3]                /* 异常→网住跳过；干净→直落 */

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
    str    wzr, [x9]                /* PS_HOLD ← 0 */
6:  b      6b

/* 异常网 handler：置旗标 + 跳过错发指令 + eret。
 * clobber x0/x2/x5/x6——读后主流程只用 x9/x10/x12/x22，无冲突。 */
net_handler:
    movz   x0, #0x8001, lsl #16
    movk   x0, #0x8000
    mov    w2, #1
    str    w2, [x0]
    mrs    x5, CurrentEL
    cmp    x5, #0x8
    b.eq   7f
    mrs    x6, elr_el1
    add    x6, x6, #4
    msr    elr_el1, x6
    b      8f
7:  mrs    x6, elr_el2
    add    x6, x6, #4
    msr    elr_el2, x6
8:  eret

/* 向量表：flat 固定 0x7C0 ⇒ 运行时=装载基+0x800（2KB 对齐律，
 * 见 u1m-probe.S 对齐律教训；段内禁 .align 11）。 */
.section .vectors, "ax"
.globl vectors
vectors:
.rept 16
    b      net_handler
    .space 124
.endr
