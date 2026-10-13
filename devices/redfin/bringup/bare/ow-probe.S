/* ow-probe.S —— B15 owner-gated 单写探针（2026-10-14）
 *
 * TRUTH.md §2 所有权铁律的正解姿势首验：扫描收集 CHGR(0x210) 的全部
 * APID 副本，逐个读 owner（cnfg 0x0c40a000+0x700+4N，val&7），选中
 * owner==0(APPS) 的副本，发唯一一笔合法写（喂狗 0x1643←1，掩码 &0xff）。
 * 全副本无 owner==0 → 跳写直接复位（CHGR 非我可写=重要裁决位）。
 *   闪灭 → owner-gated 写清白 → B16=全量 substrate 换正解选择重跑
 *   冻死 → 写侧即使 owner-gated 也挂（PON/喂狗序列嫌疑上升）
 * 依赖：B13（合法读活）+ B14（owner 窗可读）都已闪灭。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x0c44, lsl #16      /* SPMI_CORE  0x0c440000 */
    movz   x4, #0x0c60, lsl #16      /* SPMI_CHNLS 0x0c600000 */
    movz   x10, #0xa000
    movk   x10, #0x0c40, lsl #16     /* CNFG       0x0c40a000 */

    /* 扫描：x7 = 选中的 owner==0 之 CHGR apid（-1=无 APPS 副本）*/
    add    x9, x3, #0x900
    movz   x7, #0
    movz   x14, #0
1:  cmp    x14, #512
    b.hs   2f
    ldr    w11, [x9, x14, lsl #2]
    cbz    w11, 3f
    lsr    w12, w11, #8
    and    x12, x12, #0xfff
    cmp    x12, #0x210               /* CHGR */
    b.ne   3f
    mov    x13, #0x700               /* owner 读：cnfg+0x700+4*apid */
    add    x13, x13, x14, lsl #2
    ldr    w15, [x10, x13]
    ands   w15, w15, #7
    b.ne   3f                        /* owner≠0 → 非我副本，跳过 */
    mov    x7, x14                   /* owner==0 → 选中，扫毕 */
    b      2f
3:  add    x14, x14, #1
    b      1b
2:  cmp    x7, #0
    b.lt   5f                        /* 无 APPS 副本 → 不写 */

    /* 唯一新变量：owner-gated 合法写 0x1643←1 */
    lsl    x9, x7, #16               /* ch = CHNLS+0x10000*apid */
    add    x9, x4, x9
    movz   x16, #1
    str    w16, [x9, #0x10]          /* WDATA0 = 1 */
    mov    x10, #14
    lsl    x10, x10, #27             /* WRITE(14) */
    movz   x11, #0x1643
    and    x11, x11, #0xff           /* 合法地址域：0x43 */
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]                 /* CMD 发射 */
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16        /* 200000 轮询界 */
6:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 5f               /* DONE → 走（写无回读）*/
    subs   x12, x12, #1
    b.ne   6b
    /* 超时=走（不判死）*/

5:  movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100               /* 1e8 校准延时 */
7:  subs   x1, x1, #1
    b.ne   7b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]                 /* PS_HOLD 拉闸 */
8:  b      8b
