/* U1w — 零 DRAM 写网标定件（阶梯 3 刀 1w，2026-10-14）
 *
 * 判据链（u1s–u1v 六连冻二分终局 + TRUTH 通读对账）：
 *   u1v 砍光系统寄存器块仍冻死，与 calib（blink 已证）唯一被执行差分 =
 *   镜像内 str/ldr 一对（运行时 0xA7E82000，dis 实核）。TRUTH 全表查证：
 *   sp.S「DRAM 写活」无收据（B6=calib）；已证活集（L13/calib/pi/ka）
 *   全部零 DRAM 数据访问，冻死集全部含 DRAM store。
 *   统一理论：MMU 关 ⇒ 数据访问=Device-nGnRnE ⇒ 设备型 store 打进
 *   DDR 窗在本 SoC 总线挂死（真设备目标 PS_HOLD/SPMI 则活）。
 *
 * 本刀 = 仪器标定：全程零 DRAM 数据访问（含 handler——寄存器旗标）：
 *   CurrentEL 三分支装 VBAR → udf #0 触发 → handler 置 w23=1 +
 *   ELR+4 + eret → 尾巴读 w23 定闪长。异常进入不换 GPR 文件，
 *   w23 跨 eret 存活（ARMv8 架构保证）。向量表仅取指（已证活类）。
 *
 * 判读：
 *   长闪×3（~10s） = 网全链证毕（进入+handler+eret+寄存器旗标）→
 *                    u1s–u1v 冻死定谳为 DRAM store；u1x 上位；
 *   快闪×3（~1s）  = 流程走通但旗标没置（异常没进/没回）——网不可信；
 *   冻死           = CurrentEL 读/VBAR 写本身死（与 u1u/u1v 链矛盾，
 *                    重审布局论）。
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

    movz   w23, #0                  /* 寄存器旗标清零（零 DRAM 写） */
    udf    #0                       /* 触发：必同步异常 */

    /* 旗标定闪长：1=网收（1e9，长）0=漏（1e8，快） */
    cbnz   w23, 4f
    movz   x12, #0xE100
    movk   x12, #0x05F5, lsl #16
    b      5f
4:  movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16
5:  subs   x12, x12, #1
    b.ne   5b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]                /* PS_HOLD ← 0（AOSS 设备写，L13 已证活） */
6:  b      6b

/* 网 handler：置寄存器旗标 + ELR+4 + eret。只碰 w23/x6——
 * x19（EL 路由）不动，零 DRAM 访问。 */
net_handler:
    movz   w23, #1
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
 * 装载基+0x800 ⇒ 2KB 对齐律满足。段内禁 .align（会把内容顶位违律）。 */
.section .vectors, "ax"
.globl vectors
vectors:
.rept 16
    b      net_handler
    .space 124
.endr
