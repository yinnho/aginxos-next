/* U3B — READ CAPACITY 送达审（阶梯 3 刀 3-b，2026-10-14）
 *
 * 审的问题：u3 的绿（门铃清+OCS==0）是空心还是实心——8 字节答案到底
 * 落没落 0xB0002000。空心的两条机理候选（u3b 一并对账修正）：
 *   ① UPIU 头字节序错：0x01400001 LE 落内存是 01 00 40 01（flags=0、
 *      LUN=0x40 不在设备 LUN 表）→ SCSI 层 CHECK CONDITION，但事务层
 *      照样门铃清+OCS=0。改 0x01004001（内存字节 01 40 00 01）。
 *   ② PRDT 布阵与 DW7 自相矛盾：DW7=0x01000001 指 prd_off=0x100
 *      （Linux ufshcd 同布局：响应 UPIU@UCD+0x80、PRDT@UCD+0x100），
 *      而 u3 把 PRDT 写在 +0x400 —— 控制器在 +0x100 读到全零 PRDT
 *      → 零长完成 → DMA 从未发生。改：PRDT 搬 +0x100。
 * 新增送达门：buffer 首 8B 按位或非零（真盘 READ CAPACITY 答案
 * last_lba/block_size 必非零）。
 *
 * 判读：blink×3 = READ CAPACITY 真送达，传输层全绿（含 DMA）→
 *       u4 同修 PRDT 即为读盘首证冲刺；
 *       冻死 = 数据仍未落地 → u3c 读响应 UPIU 状态字节定 SCSI 层真相。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x01d8, lsl #16
    movk   x3, #0x4000             /* x3 = UFSHCI 0x1d84000 */
    movz   x8, #0xB000, lsl #16    /* x8 = UTRDL 0xB0000000 */
    movz   x9, #0xB000, lsl #16
    movk   x9, #0x1000             /* x9 = UCD 0xB0001000 */
    movz   x25, #0xB000, lsl #16
    movk   x25, #0x2000            /* x25 = 数据 0xB0002000 */

    /* UCD 清零 0x600 + UTRD 清零 32B */
    mov    x11, #0xC0
1:  str    xzr, [x9], #8
    subs   x11, x11, #1
    b.ne   1b
    sub    x9, x9, #0x600
    stp    xzr, xzr, [x8]
    stp    xzr, xzr, [x8, #16]

    /* UTRD（DW6/7 不动：resp@+0x80、prd@+0x100 正是 Linux 布局） */
    movz   x10, #0x0500, lsl #16   /* DW0 = DD|INT */
    str    x10, [x8]
    mov    w10, #8                 /* DW1 = 数据 8B */
    str    w10, [x8, #4]
    mov    w10, #0x0F              /* DW2 = invalid OCS 预填 */
    str    w10, [x8, #8]
    str    wzr, [x8, #12]
    movz   x10, #0xB000, lsl #16
    movk   x10, #0x1000            /* DW4 = UCD 地址 */
    str    x10, [x8, #16]
    str    wzr, [x8, #20]
    movz   x10, #0x0080, lsl #16
    movk   x10, #0x0080            /* DW6 = resp_len 0x80 | resp_off 0x80 */
    str    x10, [x8, #24]
    movz   x10, #0x0100, lsl #16
    movk   x10, #0x0001            /* DW7 = prd_len 1 | prd_off 0x100 */
    str    w10, [x8, #28]

    /* 命令 UPIU：改① 头 0x01004001（内存 01 40 00 01 = flags40·LUN0·tag1） */
    movz   x10, #0x0100, lsl #16
    movk   x10, #0x4001
    str    w10, [x9]
    str    wzr, [x9, #4]
    str    wzr, [x9, #8]
    movz   x10, #0x0800, lsl #16   /* exp_len BE32=8 → LE 落 0x08000000 */
    str    x10, [x9, #12]
    mov    w10, #0x25              /* CDB[0] = READ CAPACITY(10) */
    strb   w10, [x9, #16]

    /* PRDT 改②：搬到 UCD+0x100（DW7 所指处，Linux 同布局） */
    str    x25, [x9, #0x100]       /* base = 0xB0002000 */
    str    wzr, [x9, #0x108]       /* reserved */
    mov    w10, #7                 /* size = 8−1 */
    str    w10, [x9, #0x10c]

    /* 发单：UTRLBA → RSR → DBR */
    movz   x10, #0xB000, lsl #16
    str    x10, [x3, #0x50]
    str    wzr, [x3, #0x54]
    mov    w10, #1
    str    w10, [x3, #0x60]
    str    w10, [x3, #0x58]

    /* 轮询门铃自清（超时 1e8） */
    movz   x12, #0xE100
    movk   x12, #0x05F5, lsl #16
2:  ldr    w10, [x3, #0x58]
    tbz    w10, #0, 3f
    subs   x12, x12, #1
    b.ne   2b
    b      9f                      /* 超时 → 冻死 */
3:  ldr    w10, [x8, #8]
    and    w10, w10, #0x0F
    cbnz   w10, 9f                 /* OCS≠0 → 冻死 */

    /* 改③ 送达门：答案首 8B 按位或非零 */
    ldr    w27, [x25]
    ldr    w28, [x25, #4]
    orr    w27, w27, w28
    cbz    w27, 9f                 /* 全零 = 没送达 → 冻死 */

    /* pass：delay 1e9 → PS_HOLD ← 0 */
    movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16
4:  subs    x12, x12, #1
    b.ne   4b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]
9:  b      9b
