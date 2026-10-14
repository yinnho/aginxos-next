/* U3D — UFS READ CAPACITY(10) 分诊刀（阶梯 3，2026-10-14）
 *
 * 源码对账新增三修（/tmp/redfin-kernel ufshcd.c:3780 / ufshci.h:449-464）：
 *   ① DW0 = type<<28 | DD<<26 | INT<<24；UFSHCI≥2.0（redfin sm7250）type=
 *      UTP_CMD_TYPE_UFS_STORAGE(1) → SCSI 读 DW0=0x14000000。旧探针
 *      0x05000000 = type0(SCSI 代际)+INT 位，代际错。
 *   ② exp_len BE32 落盘通式：mov 原生值 + rev w10,w10 再 str。u4d/u4e
 *      的 movz #0x1000/#0x0200,lsl#16 落成 BE32=16/2（非 4096/512）——
 *      告诉设备只读 16/2 字节，两冻判词作废，试验不干净。
 *   ③ task tag = 门铃槽位号：DBR bit0 → UPIU 头 tag 字节=0（旧=1 不符
 *      内核 tag==slot）。
 *   ka.S/pi2.S 零 sctlr 写 → ABL 交接 MMU/D-cache off 非缓存世界，
 *   DMA 写即所见，无需 cache 维护。
 *
 * READ CAPACITY(10)：CDB[0]=0x25（10 字节其余零），回 8 字节 =
 * last_lba(BE32)+block_size(BE32)，真 LU 必双双非零。
 *
 * 判读：blink×3 = 命令被收且数据到门——数据路全通，转 READ(10) 首证；
 *       冻 = 指令被拒或 DMA 未达 → u3e 响应门（+0x200 transcode 0x21
 *       +status GOOD）分层。
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
    mov    w10, #8                 /* DW1 = 8 字节 */
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
    movk   x10, #0x0100            /* DW7 = PRL 1 | PRO 0x100（0x400>>2） */
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
