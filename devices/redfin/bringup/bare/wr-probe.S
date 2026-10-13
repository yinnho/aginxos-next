/* wr-probe.S —— B11 写事务探针（2026-10-14 凌晨）
 *
 * B10 证读事务全活。B11 = B10 + 唯一一个 SPMI 写事务（喂狗 0x1643←1，
 * B7 同源：CHNLS+0x10000*apid 的 WDATA0 写 + CMD 写 + DONE 轮询）。
 *   闪灭×3 → 写事务清白 → B7 冻点在 PON 装弹序列或充电使能 RMW（B12）
 *   冻死   → 0x0c6xxxxx 通道窗被 fence（CMD/WDATA 写即 stall）
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x0c44, lsl #16      /* SPMI_CORE */
    movz   x4, #0x0c60, lsl #16      /* SPMI_CHNLS */

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
2:  mov    x7, #-1
4:
    /* 唯一新变量：一个写事务（未找到则跳过）*/
    cmp    x7, #0
    b.lt   5f
    lsl    x10, x7, #16             /* 0x10000*apid */
    add    x9, x4, x10
    mov    x16, #1
    str    w16, [x9, #0x10]         /* WDATA0 = 1 */
    mov    x10, #14
    lsl    x10, x10, #27
    movz   x11, #0x1643
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]                /* CMD=WRITE(14) reg 0x1643 len 1 */
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16       /* 200000 轮询界 */
6:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 5f               /* DONE=成（写无数据回读）*/
    subs   x12, x12, #1
    b.ne   6b
    b      5f                        /* 超时=走 */

5:  movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100
8:  subs   x1, x1, #1
    b.ne   8b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]
9:  b      9b
