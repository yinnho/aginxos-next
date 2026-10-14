/* U1c — UFS AHB 门钟开+读探针（阶梯 3 刀 1c，2026-10-14）
 *
 * U1a 冻死定谳：裸读 0x1d84000 挂死=时钟关。本刀先 RMW 开
 * GCC_UFS_PHY_AHB_CLK（iface 门，无父时钟纯 gate：
 * GCC@0x100000+0x77018 bit0），再 ldr CAP，**不看值**就 blink。
 *   blink×3 → 门钟开+读返回（寄存器面活）→ 接 U1v2 验值；
 *   冻死    → 光 AHB 门不够（config-NoC/RPMh 带宽门，升刀）。
 * GCC 时钟图全账见 TRUTH.md §7。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x01d8, lsl #16
    movk   x3, #0x4000             /* UFSHCI 0x1d84000 */
    movz   x4, #0x0107, lsl #16
    movk   x4, #0x7018             /* GCC 0x1077018 = ufs_ahb CBCR */

    ldr    w5, [x4]
    orr    w5, w5, #1
    str    w5, [x4]                /* 开 AHB 门钟 */

    movz   x12, #0x96              /* ~1e6 稳定延时 */
1:  subs   x12, x12, #1
    b.ne   1b

    ldr    w20, [x3]               /* CAP——读不挂即过 */

    movz   x12, #0xCA00            /* delay 1e9 */
    movk   x12, #0x3B9A, lsl #16
2:  subs   x12, x12, #1
    b.ne   2b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]               /* PS_HOLD ← 0 */
3:  b      3b
