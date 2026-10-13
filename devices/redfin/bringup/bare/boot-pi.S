/* boot-pi.S —— B7 位置无关全量 substrate（2026-10-13 深夜，逆向闭环后）
 *
 * 【ABL 跳转契约（abl_a.img Ghidra 反编译直读，收据见 ledger）】
 *   内核落点 = 0xA7E80000（.data 常量 0x61950，2MB对齐+0x80000 校验），
 *   入口 = 镜像+0（x0=DTB），MMU/cache 已关、TLBI 已刷、DAIF 全掩。
 *   B1–B4 六代冻死根因：按 0x80008040 绝对链接，字面量全偏 ~0x278 万，
 *   BSS 清零首写 0x80008xxx = 低 DRAM TZ 挂死区，总线永不应答。
 *
 * 本桩 = main.c 全量逻辑的纯寄存器翻译：零 `ldr =` 字面量、零 DRAM 访问，
 * 对落点零假设——位置无关，落哪跑哪。
 *
 *   1. SPMI 找 CHGR(0x210)/PON(0x208) 的 APID（battery-truth.md §6）
 *   2. 充电使能 0x1042 bit0 RMW + 5 轮喂狗 0x1643←1（延时 1e9/轮）
 *   3. PON 两段装弹 warm 复位（0x85B/0x85A）+ PS_HOLD ×2
 *
 * 寄存器法（免栈免内存）：
 *   x3=SPMI_CORE  x4=CHNLS  x5=OBSRVR  x6=PS_HOLD   （全程恒定）
 *   x7=apid_chgr  x8=apid_pon  x20=轮计数           （跨调用保持）
 *   x13=ppid入    x14=apid  x15=reg  x16=字节       （子程序参数）
 *   x9-x12,x17=子程序草稿（每调用即弃）
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x0c44, lsl #16      /* SPMI_CORE  0x0c440000 */
    movz   x4, #0x0c60, lsl #16      /* SPMI_CHNLS 0x0c600000 */
    movz   x5, #0x0e60, lsl #16      /* SPMI_OBSRVR 0x0e600000 */
    movz   x6, #0x0c26, lsl #16
    movk   x6, #0x4000               /* PS_HOLD   0x0c264000 */

    movz   x13, #0x210               /* PPID_CHGR */
    bl     apid_for
    mov    x7, x14                   /* apid_chgr（-1=没找到）*/
    movz   x13, #0x208               /* PPID_PON */
    bl     apid_for
    mov    x8, x14                   /* apid_pon */

    /* —— 充电使能 RMW 0x1042 bit0 —— */
    cmp    x7, #0
    b.lt   2f
    mov    x14, x7
    movz   x15, #0x1042
    bl     spmi_rd1
    cbnz   x17, 2f                   /* 读败 → 跳过 */
    orr    x16, x16, #1
    mov    x14, x7
    movz   x15, #0x1042
    bl     spmi_wr1

    /* —— 5 轮喂狗 —— */
    movz   x20, #5
1:  cmp    x20, #0
    b.le   2f
    mov    x14, x7
    movz   x15, #0x1643
    mov    x16, #1
    bl     spmi_wr1
    movz   x12, #0x3b9a
    movk   x12, #0xca00              /* 1e9 = 0x3B9ACA00 */
    bl     delay
    subs   x20, x20, #1
    b      1b

    /* —— PON 两段装弹 warm 复位 —— */
2:  cmp    x8, #0
    b.lt   5f
    mov    x14, x8
    movz   x15, #0x85b
    bl     spmi_rd1                  /* 清 RESET_EN bit7 */
    cbnz   x17, 3f
    bic    x16, x16, #0x80
    mov    x14, x8
    movz   x15, #0x85b
    bl     spmi_wr1
3:  movz   x12, #0xd40
    movk   x12, #0x3, lsl #16       /* 500000=0x30D40 ≥500µs 装弹窗 */
    bl     delay
    mov    x14, x8
    movz   x15, #0x85a
    mov    x16, #1
    bl     spmi_wr1                  /* 装弹 warm */
    mov    x14, x8
    movz   x15, #0x85b
    bl     spmi_rd1                  /* 拉保险 bit7 */
    cbnz   x17, 5f
    orr    x16, x16, #0x80
    mov    x14, x8
    movz   x15, #0x85b
    bl     spmi_wr1

5:  str    wzr, [x6]                 /* PS_HOLD=0 拉闸 */
    movz   x12, #0xf424              /* 1e6 */
    bl     delay
    str    wzr, [x6]                 /* 补一刀 */
6:  b      6b

/* apid_for: 入 x13=ppid → 出 x14=apid（-1=没找到）
 * 扫 core+0x900+4N 共 512 项，ppid=(v>>8)&0xfff，v==0 跳过 */
apid_for:
    mov    x14, #0
    add    x9, x3, #0x900
1:  cmp    x14, #512
    b.hs   2f
    ldr    w10, [x9, x14, lsl #2]
    cbz    w10, 3f
    lsr    x11, x10, #8
    and    x11, x11, #0xfff
    cmp    x11, x13
    b.eq   4f
3:  add    x14, x14, #1
    b      1b
2:  mov    x14, #-1                  /* 扫尽未中 */
4:  ret

/* spmi_wr1: 入 x14=apid x15=reg x16=字节 → 出 x17(0=成)
 * ch=CHNLS+0x10000*apid，WDATA0@0x10，CMD@0x00=(14<<27)|(reg<<4)|0，DONE@0x08 */
spmi_wr1:
    lsl    x10, x14, #16
    add    x9, x4, x10
    str    w16, [x9, #0x10]
    mov    x10, #14
    lsl    x10, x10, #27
    and    x11, x15, #0xff        /* 地址域只装低 8 位（PPID 承载高位）*/
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16       /* 200000 */
1:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 3f               /* DONE 置位 → 查失败位 */
    subs   x12, x12, #1
    b.ne   1b
    mov    x17, #-2                  /* 轮询超时 */
    ret
3:  and    x17, x11, #0xe            /* 0=成 FAILURE/DENIED/DROPPED≠0 */
    ret

/* spmi_rd1: 入 x14=apid x15=reg → 出 x17(0=成) x16=字节
 * ch=OBSRVR+0x80*apid，CMD@0x00=(15<<27)|(reg<<4)|0，RDATA0@0x18 */
spmi_rd1:
    lsl    x10, x14, #7
    add    x9, x5, x10
    mov    x10, #15
    lsl    x10, x10, #27
    and    x11, x15, #0xff        /* 同上——违例命令字曾锁死通道窗（B7 冻案）*/
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16       /* 200000 */
1:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 3f
    subs   x12, x12, #1
    b.ne   1b
    mov    x17, #-2
    ret
3:  ands   x17, x11, #0xe
    b.ne   4f
    ldr    w16, [x9, #0x18]          /* 低字节即所读 */
4:  ret

/* delay: 入 x12=迭代数（纯消耗，钟速不敏感）*/
delay:
    cbz    x12, 2f
1:  subs   x12, x12, #1
    b.ne   1b
2:  ret
