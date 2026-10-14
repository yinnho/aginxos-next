/* U1x — 烤表 MMU 刀（阶梯 3 刀 1x，2026-10-14）
 *
 * 前提：u1w 长闪证网活。理论（TRUTH §1+u1s–u1v 链）：MMU 关 ⇒
 * 数据访问=Device-nGnRnE ⇒ DDR 段 store 挂总线。本刀 = 开 MMU
 * （恒等映射，页表=镜像内常量，**全程零运行时写**）后在 DRAM 做
 * store/load 往返——地址与 u1v 冻死件完全相同（运行时 0xA7E82000，
 * flat .scratch 0x1FC0），唯一变量 = 内存属性：
 *   u1v：MMU 关（Device）  store 0xA7E82000 → 冻死（已收）；
 *   u1x：MMU 开（Normal）  store 0xA7E82000 → ？
 *
 * 页表（flat 0xFC0，运行时=装载基+0x40+0xFC0=装载基+0x1000，
 * 4KB 天然对齐——装载基 2MB 对齐）：
 *   L1 单级表，T0SZ=25 ⇒ 39bit VA ⇒ 硬件从 L1 起走，1GB 块：
 *   e0/e1（0–2GB，全部 MMIO 含 PS_HOLD/UFS/GIC/SPMI）= Device-nGnRE
 *     （attridx0，MAIR byte0=0x04）；
 *   e2–e9（2–10GB，DRAM 8GB）= Normal WB（attridx1，MAIR byte1=0xFF）；
 *   e10–511 = 0（fault → 网收）。
 *   描述符=PA|0x401/0x405（block=0b01、attrindx=bits[4:2]、AF=bit10）。
 *   表 512×8B=4KB，恰好铺满 [0xFC0,0x1FC0)。
 *
 * 判读：
 *   长闪×3 = MMU-on + 恒等映射 + DRAM 往返 + 网全链证毕
 *            → 存储线解锁（描述符可写，u2/u3/u4 骨架定形）；
 *   快闪×3 = 网收了但往返不符（翻译错被网收住 w22 留 1，或 EL≠1 兜底）；
 *   冻死  = MMU 开本身死 / Normal store 同挂 / 网在 MMU-on 下死
 *            → 拆刀：M|C 变体 / 无 store 纯 MMU 件。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    /* CurrentEL；非 EL1 → 全跳，快闪兜底（顺手报 EL≠1） */
    mrs    x0, CurrentEL
    lsr    x19, x0, #2
    sub    x19, x19, #1
    movz   w22, #1                  /* 往返结果预置=失败 */
    movz   w23, #0                  /* 网旗标清零 */
    cbnz   x19, 90f

    adr    x1, vectors
    msr    vbar_el1, x1
    isb

    /* ---- MMU 烤表启用（全 sysreg 写，零 DRAM 写）---- */
    movz   x2, #0xFF04              /* MAIR: idx0=nGnRE(0x04) idx1=WB(0xFF) */
    msr    mair_el1, x2
    movz   x3, #0x3019              /* TCR: T0SZ=25 | SH0=inner(0b11<<12) */
    movk   x3, #0x2, lsl #32        /*     | IPS=0b010(40bit PA, bits[34:32]) */
    msr    tcr_el1, x3
    adr    x4, l1_table
    msr    ttbr0_el1, x4            /* 运行时表址（4KB 对齐，头注） */
    isb
    mrs    x5, sctlr_el1
    orr    x5, x5, #1               /* M=1（C/I 保持关：纯属性对照） */
    msr    sctlr_el1, x5
    isb

    /* ---- DRAM 往返（此刻 VA=PA 恒等；u1v 同址）---- */
    adr    x10, scratch_flag
    movz   w11, #0xA5A5
    str    w11, [x10]               /* 若翻译错 → 网收 → w22 留 1 → 快闪 */
    ldr    w14, [x10]
    sub    w22, w14, w11            /* 0 = 往返一致 */

    udf    #0                       /* MMU-on 下的网触发 */

    /* ---- 判读：网收且往返一致 → 长；否则快 ---- */
90: cbz    w23, 91f
    cbnz   w22, 91f
    movz   x12, #0xCA00
    movk   x12, #0x3B9A, lsl #16    /* 1e9 ≈ 10s（长） */
    b      5f
91: movz   x12, #0xE100
    movk   x12, #0x05F5, lsl #16    /* 1e8 ≈ 1s（快） */
5:  subs   x12, x12, #1
    b.ne   5b
    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000
    str    wzr, [x9]                /* PS_HOLD ← 0（entry0 Device，同前证活） */
6:  b      6b

/* 网 handler：置寄存器旗标 + ELR+4 + eret（仅 EL1 路径——
 * 本件只在 EL1 装网并触发）。只碰 w23/x6，零 DRAM 访问。 */
net_handler:
    movz   w23, #1
    mrs    x6, elr_el1
    add    x6, x6, #4
    msr    elr_el1, x6
    eret

/* 向量表：flat 0x7C0 ⇒ 运行时装载基+0x800（2KB 对齐律）。 */
.section .vectors, "ax"
.globl vectors
vectors:
.rept 16
    b      net_handler
    .space 124
.endr

/* L1 页表：flat 0xFC0 ⇒ 运行时装载基+0x1000（4KB 对齐）。 */
.section .pgtable, "a"
.globl l1_table
l1_table:
    .quad 0x0000000000000401        /* e0: 0–1GB   Device（MMIO 全在窗内） */
    .quad 0x0000000040000401        /* e1: 1–2GB   Device */
    .quad 0x0000000080000405        /* e2: 2–3GB   Normal WB ← DRAM 起 */
    .quad 0x00000000C0000405        /* e3: 3–4GB   Normal WB */
    .quad 0x0000000100000405        /* e4: 4–5GB   Normal WB */
    .quad 0x0000000140000405        /* e5: 5–6GB   Normal WB */
    .quad 0x0000000180000405        /* e6: 6–7GB   Normal WB */
    .quad 0x00000001C0000405        /* e7: 7–8GB   Normal WB */
    .quad 0x0000000200000405        /* e8: 8–9GB   Normal WB */
    .quad 0x0000000240000405        /* e9: 9–10GB  Normal WB */
    .rept 502
    .quad 0                          /* e10–511: fault → 网收 */
    .endr

/* 往返单元：flat 0x1FC0 ⇒ 运行时装载基+0x2000（=u1v 冻死同址）。 */
.section .scratch, "aw"
.globl scratch_flag
scratch_flag:
    .space 4
