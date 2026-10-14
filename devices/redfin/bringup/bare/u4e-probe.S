/* U4E — UFS 首块读·512 世界版（阶梯 3 刀 4-e，2026-10-14）
 *
 * 源码定谳（/tmp/redfin-kernel ufshcd.c:5048 / ufshci.h:514-563）：
 *   ① UTRD DW6/7 = length<<16 | offset，且 offset 单位是 dword（>>2）；
 *   ② ALIGNED_UPIU_SIZE=512 → 命令 UPIU[512]+响应 UPIU[512]+PRDT@+0x400，
 *     响应在字节 +0x200；DW6=RUL 0x80|RUO 0x80=0x00800080（u3 侥幸对）；
 *   ③ PRL=PRDT 条目数、PRO=0x400>>2=0x100 → DW7 应为 0x00010100。
 *   u3 家族全部写成 0x01000001（两半装反：len256/off1）——控制器一直在
 *   字节+4（命令 UPIU 内部）读 PRDT，DMA 从未发生：u3 空心绿、u4/u4b
 *   magic 对零、u3b（PRDT 又被我错搬到 +0x100）冻，一案全解。
 *
 * u4d（4096 世界全契约版）冻——世界成为最后未测变量。u4e = u4d 唯改
 * 尺寸三处：DW1 4096→512、exp_len 0x1000→0x200、PRDT size 0xFFF→0x1FF
 * （头/DW7/PRDT 位/LBA/magic 全保持源码正解）。
 *
 * 判读：blink×3 = 无内核读盘首证（512 世界，GPT 头 LBA1 到手）；
 *       冻死 = 世界也排除 → u3d（READ CAPACITY 同契约+非零门）分层。
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

    /* UTRD（DW1=4096；DW6/7 = len<<16|off，off 单位 dword） */
    movz   x10, #0x0500, lsl #16   /* DW0 = DD|INT */
    str    x10, [x8]
    movz   x10, #0x0200            /* DW1 = 512B 改① */
    str    w10, [x8, #4]
    mov    w10, #0x0F              /* DW2 = invalid OCS 预填 */
    str    w10, [x8, #8]
    str    wzr, [x8, #12]
    movz   x10, #0xB000, lsl #16
    movk   x10, #0x1000            /* DW4/5 = UCD 地址 */
    str    x10, [x8, #16]
    str    wzr, [x8, #20]
    movz   x10, #0x0080, lsl #16
    movk   x10, #0x0080            /* DW6 = RUL 0x80 | RUO 0x80（0x200>>2） */
    str    x10, [x8, #24]
    movz   x10, #0x0001, lsl #16
    movk   x10, #0x0100            /* DW7 = PRL 1 | PRO 0x100（0x400>>2）改① */
    str    w10, [x8, #28]

    /* 命令 UPIU：头 0x01004001、exp_len BE32=4096、CDB=READ(10) LBA1 ×1 */
    movz   x10, #0x0100, lsl #16
    movk   x10, #0x4001            /* 内存 01 40 00 01 = flags40·LUN0·tag1 */
    str    w10, [x9]
    str    wzr, [x9, #4]
    str    wzr, [x9, #8]
    movz   x10, #0x0200, lsl #16   /* exp_len BE32=0x200 → LE 0x02000000 改② */
    str    x10, [x9, #12]
    mov    w10, #0x28              /* CDB[0] = READ(10) */
    strb   w10, [x9, #16]
    movz   x10, #0x0100, lsl #16   /* CDB[2..5] BE 00000001 → LE 0x01000000 */
    str    x10, [x9, #0x12]
    movz   w10, #0x0100            /* CDB[7..8] BE 0001 → LE 0x0100 */
    strh   w10, [x9, #0x17]

    /* PRDT 改②：归位 UCD+0x400（PRO 所指）、base 0xB0002000、size=4096−1 */
    str    x25, [x9, #0x400]
    str    wzr, [x9, #0x408]
    movz   x10, #0x01FF
    str    w10, [x9, #0x40c]       /* size = 512−1 改③ */

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
    b      9f
3:  ldr    w10, [x8, #8]
    and    w10, w10, #0x0F
    cbnz   w10, 9f                 /* OCS≠0 → 冻死 */

    /* magic：bytes "EFI PART" → LE dwords 0x20494645 / 0x54524150 */
    ldr    w27, [x25]
    movz   x28, #0x4645
    movk   x28, #0x2049, lsl #16
    cmp    w27, w28
    b.ne   9f
    ldr    w27, [x25, #4]
    movz   x28, #0x4150
    movk   x28, #0x5452, lsl #16
    cmp    w27, w28
    b.ne   9f

    /* pass：delay 1e9 → PS_HOLD ← 0 */
    movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16
4:  subs   x12, x12, #1
    b.ne   4b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]
9:  b      9b
