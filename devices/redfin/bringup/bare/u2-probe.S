/* U2 — UFS 控制器活态探针（阶梯 3 刀 2，2026-10-14）
 *
 * U1 前置基础上判 ABL 留态：HCE(0x34) bit0==1 且 UTRLRSR(0x60) bit0==1
 * → blink（控制器活着、传输列表在跑=快路存在）；否则冻死（需全 init：
 * HCE 装弹+UIC LINKSTARTUP，备用大刀）。
 * 契约见 TRUTH.md §7。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x01d8, lsl #16
    movk   x3, #0x4000             /* UFSHCI 0x1d84000 */

    ldr    w20, [x3]               /* CAP */
    /* VER 闸门已撤（2026-10-14 翻案刀）：qcom hw major≥2 的 VER 是坏寄存器
     * （UFSHCD_QUIRK_BROKEN_UFS_HCI_VERSION，内核自编 0x0200）——活控制器
     * 也会踩空冻死。CAP 零值门保留。 */
    cbz    w20, 9f

    ldr    w23, [x3, #0x34]        /* HCE */
    tbz    w23, #0, 9f
    ldr    w24, [x3, #0x60]        /* UTRLRSR */
    tbz    w24, #0, 9f

    /* pass：delay 1e9 → PS_HOLD ← 0 */
    movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16
1:  subs   x12, x12, #1
    b.ne   1b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]
9:  b      9b
