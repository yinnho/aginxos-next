/* U1a — UFS MMIO 读可行性判别器（阶梯 3 刀 1b，2026-10-14）
 *
 * U1 冻死后拆变量：本刀只做一次 ldr CAP @0x1d84000，**不看值**，
 * 读到指令流就走 delay+PS_HOLD（blink）。
 *   blink×3 → 读事务执行且返回（时钟/总线活），U1 冻=值不符 → 走 U1v2；
 *   冻死    → 读本身挂死（时钟/NoC 门没开）→ 走 GCC 时钟刀。
 * 版本寄存器在 qcom hw major≥2 是坏的（UFSHCD_QUIRK_BROKEN_UFS_HCI_
 * VERSION，驱动自编 0x0200）——U1 拿 VER 当闸门踩空，此刀起弃用 VER。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x01d8, lsl #16
    movk   x3, #0x4000             /* UFSHCI 0x1d84000 */

    ldr    w20, [x3]               /* CAP——读不挂即过 */

    movz   x12, #0xCA00            /* delay 1e9 */
    movk   x12, #0x3B9A, lsl #16
1:  subs   x12, x12, #1
    b.ne   1b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]               /* PS_HOLD ← 0 */
2:  b      2b
