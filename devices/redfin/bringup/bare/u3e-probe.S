/* U3E — u3d 单变量：UTRLBA 64 位写 → 32 位写（阶梯 3，2026-10-14 夜）
 *
 * UFSDxe 逆向（factory xbl FV → UFSDxe.efi → Ghidra）：Qualcomm 驱动对
 * UFSHCI 全部 32 位访问。u3 是唯一用 `str w10,[x3,#0x50]` 的探针也是唯一
 * 过门者；u3b/u4/u4b/u4c/u4d/u4e/u3d 七件全用 `str x10`（64 位写 32 位外设
 * 寄存器，疑外部中止→无向量表=冻）全冻。本刀只改这一条，其余同 u3d：
 * DW0=0x14000000、DW1=0、DW7=0x01000001、头 01 40 00 00、exp_len BE 8、
 * CDB 0x25、数据缓冲清零+非零门。
 *
 * 判读：blink×3 = 64 位写即真凶 + 0x14000000 可用 + READ CAPACITY 数据真落
 *       → 转 READ(10) 首证；冻 = u3f（DW0 换 Qualcomm 形 0x05000000）。
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

    /* UCD 清零 0x600 + UTRD 清零 32B + 数据缓冲 8B 清零（非零门防假阳） */
    mov    x11, #0xC0
1:  str    xzr, [x9], #8
    subs   x11, x11, #1
    b.ne   1b
    sub    x9, x9, #0x600
    stp    xzr, xzr, [x8]
    stp    xzr, xzr, [x8, #16]
    str    xzr, [x25]

    /* UTRD：DW0=0x14000000（type1|DD读）；DW1=8B；DW2 预填 OCS=0x0F */
    movz   x10, #0x1400, lsl #16   /* DW0 = 0x14000000 修① */
    str    x10, [x8]
    str    wzr, [x8, #4]           /* DW1 = DUNL（crypto 关=保留 0，ufshcd.c:3631；非数据长度） */
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
    movz   x10, #0x0100, lsl #16   /* DW7 高半 = PRDT offset 0x100（0x400>>2） */
    movk   x10, #0x0001            /* DW7 低半 = PRDT length 1（ufshci.h:560 length 在 +28） */
    str    w10, [x8, #28]

    /* 命令 UPIU：头=01 40 00 00（trans/读flags/LUN0/tag0 修③）*/
    movz   x10, #0x4001            /* LE32 0x00004001 → 内存 01 40 00 00 */
    str    w10, [x9]
    str    wzr, [x9, #4]           /* dword1 = cmd_set_type SCSI=0 + 保留 */
    str    wzr, [x9, #8]           /* dword2 = EHS/data seg 长度 = 0 */
    mov    w10, #8                 /* exp_len BE32=8 → rev 通式 修② */
    rev    w10, w10                /* 0x00000008 → 0x08000000 */
    str    w10, [x9, #12]
    mov    w10, #0x25              /* CDB[0] = READ CAPACITY(10)，其余零 */
    strb   w10, [x9, #16]

    /* PRDT：base 0xB0002000、size=8−1=7 */
    str    x25, [x9, #0x400]
    str    wzr, [x9, #0x408]
    mov    w10, #7
    str    w10, [x9, #0x40c]

    /* 发单：UTRLBA → RSR → DBR */
    movz   x10, #0xB000, lsl #16
    str    w10, [x3, #0x50]        /* UTRLBA 32 位写（u3e 唯一变量） */
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

    /* 非零门：last_lba | block_size ≠ 0（缓冲已清零，非零必是 DMA 落的） */
    ldr    w27, [x25]
    ldr    w28, [x25, #4]
    orr    w27, w27, w28
    cbz    w27, 9f

    /* pass：delay 1e9 → PS_HOLD ← 0 */
    movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16
4:  subs   x12, x12, #1
    b.ne   4b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]
9:  b      9b
