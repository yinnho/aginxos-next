/* rd-probe.S —— B10 读事务探针（2026-10-14 凌晨）
 *
 * B9 证 512 项扫描全活。B10 = B9 + 唯一一个 SPMI 读事务（CHGR 0x1042，
 * 与 B7 首事务同源：OBSRVR+0x80*apid 的 CMD 写 + DONE 轮询 + RDATA 读）。
 *   闪灭×3 → 读事务清白 → 冻点在 CHNLS 写事务（B11=写探针）
 *   冻死   → 0x0e6xxxxx 观察窗被 fence（CMD 写即 stall）
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x0c44, lsl #16      /* SPMI_CORE */
    movz   x5, #0x0e60, lsl #16      /* SPMI_OBSRVR */

    /* 扫描找 CHGR apid（x7）——B9 已证安全 */
    add    x9, x3, #0x900
    movz   x7, #0
    movz   x13, #0x210               /* PPID_CHGR */
1:  cmp    x7, #512
    b.hs   2f
    ldr    w10, [x9, x7, lsl #2]
    cbz    w10, 3f
    lsr    x11, x10, #8
    and    x11, x11, #0xfff
    cmp    x11, x13
    b.eq   4f
3:  add    x7, x7, #1
    b      1b
2:  mov    x7, #-1                  /* 没找到（理论上不会）*/
4:
    /* 唯一新变量：一个读事务。未找到则跳过。 */
    cmp    x7, #0
    b.lt   5f
    lsl    x10, x7, #7              /* 0x80*apid */
    add    x9, x5, x10
    mov    x10, #15
    lsl    x10, x10, #27
    movz   x11, #0x1042
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]                /* CMD=READ(15) reg 0x1042 len 1 */
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16       /* 200000 轮询界 */
6:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 7f
    subs   x12, x12, #1
    b.ne   6b
    b      5f                        /* 超时=走（不判死）*/
7:  ldr    w16, [x9, #0x18]         /* RDATA0（读了就扔）*/

5:  movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100
8:  subs   x1, x1, #1
    b.ne   8b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]
9:  b      9b
