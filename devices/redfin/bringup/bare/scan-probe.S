/* scan-probe.S —— B9 纯扫描探针（2026-10-14 凌晨）
 *
 * B8 证 SPMI_CORE 表首项可读。B9 = B5 壳 + 完整 512 项扫描（与 B7 的
 * 扫描逐字节同源），但零 SPMI 事务、零写。二分「读空间」vs「事务空间」：
 *   闪灭×3 → 扫描清白 → B7 冻点在 CHNLS/OBSRVR 事务（B10=读事务探针）
 *   冻死   → 表读越页（0x0c441000 页 fence 或某项触发 stall）
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x0c44, lsl #16      /* SPMI_CORE */
    add    x9, x3, #0x900            /* APID 表 */
    movz   x14, #0                   /* n = 0..511 */
1:  cmp    x14, #512
    b.hs   3f
    ldr    w10, [x9, x14, lsl #2]    /* 唯一动作类：逐项读表 */
    add    x14, x14, #1
    b      1b
3:  movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100               /* 1e8 校准延时 */
4:  subs   x1, x1, #1
    b.ne   4b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]
5:  b      5b
