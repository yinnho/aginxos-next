/* U3C — 响应 UPIU 审（阶梯 3 刀 3-c，2026-10-14）
 *
 * u3b 冻（头+PRDT 对账后数据仍不落地）→ 问题上移一层：设备到底接没
 * 接受这条 READ CAPACITY。本刀 = u3b 配置不动（头 0x01004001、
 * PRDT@+0x100），唯换门：读 DW6 所指 UCD+0x80 的响应 UPIU——
 *   byte0 == 0x21（RESPONSE 真落地）且 byte5 == 0x00（SCSI GOOD）
 *   → blink；否则冻。
 *
 * 判读：blink×3 = 指令被接受且 GOOD —— u3b 的冻纯因数据没落 DRAM，
 *       主审转 PRDT 语义（size 单位/段长/PRL 含义）；
 *       冻死 = 响应没落 +0x80 或状态非 GOOD → u3d（单门 resp[0]==0x21
 *       分辨两况；sense 数据线备查：byte16 起 sense key 定死因）。
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

    /* UTRD（DW6=resp@+0x80、DW7=prd@+0x100） */
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

    /* 命令 UPIU：头 0x01004001（flags40·LUN0·tag1） */
    movz   x10, #0x0100, lsl #16
    movk   x10, #0x4001
    str    w10, [x9]
    str    wzr, [x9, #4]
    str    wzr, [x9, #8]
    movz   x10, #0x0800, lsl #16   /* exp_len BE32=8 → LE 落 0x08000000 */
    str    x10, [x9, #12]
    mov    w10, #0x25              /* CDB[0] = READ CAPACITY(10) */
    strb   w10, [x9, #16]

    /* PRDT @UCD+0x100：base 0xB0002000、size=8−1 */
    str    x25, [x9, #0x100]
    str    wzr, [x9, #0x108]
    mov    w10, #7
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

    /* 唯一新门：响应 UPIU 真落地（byte0==0x21）且 SCSI 状态 GOOD（byte5==0） */
    ldrb   w27, [x9, #0x80]
    cmp    w27, #0x21
    b.ne   9f
    ldrb   w27, [x9, #0x85]
    cbnz   w27, 9f

    /* pass：delay 1e9 → PS_HOLD ← 0 */
    movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16
4:  subs   x12, x12, #1
    b.ne   4b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]
9:  b      9b
