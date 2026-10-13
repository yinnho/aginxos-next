/* vr-probe.S —— B13 合法读事务探针（2026-10-14 凌晨）
 *
 * B12（掩码修复后的全量 substrate）仍冻死。修正认知：B10/B11 的违例命令
 * 多半被 arbiter DENIED——只证了命令机制+窗口映射，未证真 PMIC 事务。
 * 本桩 = B9 扫描 + 唯一一个合法读事务（0x1042 掩码 &0xff）+ 一次轮询 +
 * 复位。零后续事务（第二次事务访问挂在忙死 arbiter 上的路径被封死）。
 *   闪灭 → 首个真读事务活，凶手在写侧（0x1042/0x1643/0x85x）
 *   冻死 → 合法命令一发即挂 arbiter（PMIC 链路此阶段不通）
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
    movz   x13, #0x210
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
    cmp    x7, #0
    b.lt   5f
    lsl    x10, x7, #7              /* 0x80*apid */
    add    x9, x5, x10
    mov    x10, #15
    lsl    x10, x10, #27
    movz   x11, #0x1042
    and    x11, x11, #0xff          /* 合法地址域：0x42 */
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]                /* CMD=READ(15) 0x42 len 1（合法）*/
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16
6:  ldr    w11, [x9, #8]            /* 唯一轮询（冻点若在此即挂）*/
    tbnz   w11, #0, 7f
    subs   x12, x12, #1
    b.ne   6b
    b      5f                        /* 超时=走 */
7:  ldr    w16, [x9, #0x18]         /* RDATA0 */

5:  movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100
8:  subs   x1, x1, #1
    b.ne   8b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]
9:  b      9b
