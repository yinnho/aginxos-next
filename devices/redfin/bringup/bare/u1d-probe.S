/* U1d — GCC 自检探针（阶梯 3 刀 1d，2026-10-14）
 *
 * U1c 冻死（AHB 门开了 UFS 读还挂）。先证乘法器本身：GCC@0x100000
 * 属常开域。读 0x1077018 → RMW bit0=1 → 回读 bit0==1 → blink（不看
 * UFS）。冻死=连 GCC 都读挂（config-NoC 域死，升刀方向完全不同）。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x4, #0x0107, lsl #16
    movk   x4, #0x7018             /* GCC 0x1077018 = ufs_ahb CBCR */

    ldr    w5, [x4]                /* 读GCC——挂则冻死，即判据 */
    orr    w5, w5, #1
    str    w5, [x4]
    ldr    w6, [x4]
    tbz    w6, #0, 9f              /* 写没粘住=GCC 写死 → 冻死 */

    movz   x12, #0xCA00            /* delay 1e9 */
    movk   x12, #0x3B9A, lsl #16
1:  subs   x12, x12, #1
    b.ne   1b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]               /* PS_HOLD ← 0 */
9:  b      9b
