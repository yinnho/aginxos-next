/* own-probe.S —— B14 cnfg/owner 窗读探针（2026-10-14）
 *
 * 新读空间：cnfg 0x0c40a000 的 owner 表（+0x700+4N，val&7=拥有 EE）。
 * 真 DT 定谳 cnfg 基址独立于 core（TRUTH.md §2）；B7/B12 的扫描取首个
 * ppid 匹配副本，而 v5 同 PPID 可挂多 APID、owner 表指定谁可写——
 * owner 窗从未探过。本桩零事务零写，扫表凡命中 0x210/0x208 即读其 owner。
 *   闪灭 → cnfg 窗可读（B15=owner-gated 单写探针的地基）
 *   冻死 → cnfg 窗被 fence（owner 门只能绕，不能查）
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x0c44, lsl #16      /* SPMI_CORE 0x0c440000 */
    movz   x10, #0xa000
    movk   x10, #0x0c40, lsl #16     /* CNFG 0x0c40a000 */

    add    x9, x3, #0x900            /* APID 表 */
    movz   x14, #0
1:  cmp    x14, #512
    b.hs   5f
    ldr    w11, [x9, x14, lsl #2]
    cbz    w11, 2f
    lsr    w12, w11, #8
    and    x12, x12, #0xfff
    cmp    x12, #0x210               /* CHGR */
    b.eq   4f
    cmp    x12, #0x208               /* PON */
    b.eq   4f
2:  add    x14, x14, #1
    b      1b
4:  mov    x13, #0x700
    add    x13, x13, x14, lsl #2
    ldr    w15, [x10, x13]           /* 唯一新变量：owner 表读 */
    b      2b

5:  movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100               /* 1e8 校准延时 */
6:  subs   x1, x1, #1
    b.ne   6b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]                 /* PS_HOLD 拉闸 */
7:  b      7b
