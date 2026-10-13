/* pon-probe.S —— B17 PON 所有权判别器（2026-10-14）
 *
 * B16 全量 substrate v2 闪灭=完工，但 blink 分不清 PON 序列真跑（owner==0
 * 副本存在）还是被跳过（apid=-1 直落 PS_HOLD）。本桩把「没找到」变成
 * 信号：找 PON(0x208) 的 owner==0 副本——
 *   找到 → 延时+PS_HOLD → 闪灭×3（PON 可用，整机刷复位动词可上 PON warm）
 *   没找到 → 死循环冻 logo（PON 全归 TZ，复位动词=纯 PS_HOLD）
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x0c44, lsl #16      /* SPMI_CORE  0x0c440000 */
    movz   x19, #0xa000
    movk   x19, #0x0c40, lsl #16     /* CNFG       0x0c40a000 */

    add    x9, x3, #0x900
    movz   x14, #0
1:  cmp    x14, #512
    b.hs   2f
    ldr    w10, [x9, x14, lsl #2]
    cbz    w10, 3f
    lsr    w11, w10, #8
    and    x12, x11, #0xfff
    cmp    x12, #0x208               /* PON */
    b.ne   3f
    mov    x13, #0x700
    add    x13, x13, x14, lsl #2
    ldr    w15, [x19, x13]
    ands   w15, w15, #7
    b.eq   4f                        /* owner==0 → 命中 */
3:  add    x14, x14, #1
    b      1b
2:  b      2b                        /* 没找到 → 冻死=信号 */
4:
    movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100               /* 1e8 校准延时 */
5:  subs   x1, x1, #1
    b.ne   5b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]                 /* PS_HOLD 拉闸 */
6:  b      6b
