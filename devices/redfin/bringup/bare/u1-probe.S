/* U1 — UFS MMIO 探针 v2（阶梯 3 刀 1，2026-10-14）
 *
 * v2 改两处（承 U1 冻死复盘）：①弃用 VER——qcom hw major≥2 版本寄存器
 * 坏（UFSHCD_QUIRK_BROKEN_UFS_HCI_VERSION，驱动自编 0x0200）；②先开
 * AHB 门钟（GCC 0x1077018 bit0，U1a 证裸读挂死=时钟关）。
 * 闸门：(CAP & 0x1F)!=0（NUTRS 位权，0-based）→ blink；否则冻死。
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

    ldr    w20, [x3]               /* CAP */
    and    w22, w20, #0x1F         /* NUTRS 位权 */
    cbz    w22, 9f

    movz   x12, #0xCA00            /* delay 1e9 */
    movk   x12, #0x3B9A, lsl #16
1:  subs   x12, x12, #1
    b.ne   1b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]
9:  b      9b
