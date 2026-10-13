/* pi2.S —— B16 全量 substrate v2：owner-gated 选择（2026-10-14）
 *
 * B13/B14/B15 三连闪灭定谳：合法读写事务、cnfg owner 窗全清白；正解姿势
 * （TRUTH.md §2 铁律）= 扫描收集 PPID 全部 APID 副本、读 owner 表选
 * owner==0(APPS) 的副本再发写。B7/B12 冻死根因锁定候选=取首个匹配副本。
 * 本桩 = boot-pi.S 全量逻辑（充电 RMW + 5 轮喂狗 + PON 两段装弹 + PS_HOLD）
 * 唯一改动：apid 选择换 owner-gated（CHGR 与 PON 都查）。
 *   闪灭×3 → substrate 完工，无 Linux 内核阶梯 2（整机刷）解锁
 *   冻死   → 冻点在事务序列/PON 序列本身，逐段再拆
 *
 * 寄存器法（免栈免内存，承 boot-pi.S）：
 *   x3=CORE x4=CHNLS x5=OBSRVR x6=PS_HOLD x19=CNFG（helper 不碰）
 *   x7=apid_chgr x8=apid_pon x20=轮计数
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
    movz   x19, #0xa000
    movk   x19, #0x0c40, lsl #16     /* CNFG       0x0c40a000 */

    movz   x13, #0x210               /* PPID_CHGR */
    bl     apid_own
    mov    x7, x14                   /* owner==0 的副本（-1=无）*/
    movz   x13, #0x208               /* PPID_PON */
    bl     apid_own
    mov    x8, x14

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
2:  movz   x20, #5
1:  cmp    x20, #0
    b.le   3f
    cmp    x7, #0
    b.lt   4f
    mov    x14, x7
    movz   x15, #0x1643
    mov    x16, #1
    bl     spmi_wr1
    movz   x12, #0x3b9a
    movk   x12, #0xca00              /* 1e9 */
    bl     delay
    subs   x20, x20, #1
    b      1b

    /* —— PON 两段装弹 warm 复位 —— */
3:  cmp    x8, #0
    b.lt   5f
    mov    x14, x8
    movz   x15, #0x85b
    bl     spmi_rd1                  /* 清 RESET_EN bit7 */
    cbnz   x17, 4f
    bic    x16, x16, #0x80
    mov    x14, x8
    movz   x15, #0x85b
    bl     spmi_wr1
4:  movz   x12, #0xd40
    movk   x12, #0x3, lsl #16        /* 500000 ≥500µs 装弹窗 */
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

/* apid_own: 入 x13=ppid → 出 x14=owner==0 的 apid（-1=无）
 * 扫 core+0x900+4N 全 512 项，命中即读 owner（cnfg+0x700+4N），owner==0 选中 */
apid_own:
    mov    x14, #0
    add    x9, x3, #0x900
1:  cmp    x14, #512
    b.hs   2f
    ldr    w10, [x9, x14, lsl #2]
    cbz    w10, 3f
    lsr    w11, w10, #8
    and    x11, x11, #0xfff
    cmp    x11, x13
    b.ne   3f
    mov    x12, #0x700
    add    x12, x12, x14, lsl #2
    ldr    w15, [x19, x12]
    ands   w15, w15, #7
    b.eq   4f                        /* owner==0 → 选中 */
3:  add    x14, x14, #1
    b      1b
2:  mov    x14, #-1                  /* 无 APPS 副本 */
4:  ret

/* spmi_wr1: 入 x14=apid x15=reg x16=字节 → 出 x17(0=成)
 * ch=CHNLS+0x10000*apid，WDATA0@0x10，CMD@0x00=(14<<27)|((reg&0xff)<<4)，DONE@0x08 */
spmi_wr1:
    lsl    x10, x14, #16
    add    x9, x4, x10
    str    w16, [x9, #0x10]
    mov    x10, #14
    lsl    x10, x10, #27
    and    x11, x15, #0xff
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16        /* 200000 */
1:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 3f
    subs   x12, x12, #1
    b.ne   1b
    mov    x17, #-2
    ret
3:  and    x17, x11, #0xe            /* 0=成 FAILURE/DENIED/DROPPED≠0 */
    ret

/* spmi_rd1: 入 x14=apid x15=reg → 出 x17(0=成) x16=字节
 * ch=OBSRVR+0x80*apid，CMD@0x00=(15<<27)|((reg&0xff)<<4)，RDATA0@0x18 */
spmi_rd1:
    lsl    x10, x14, #7
    add    x9, x5, x10
    mov    x10, #15
    lsl    x10, x10, #27
    and    x11, x15, #0xff
    lsl    x11, x11, #4
    orr    x10, x10, x11
    str    w10, [x9]
    movz   x12, #0xd40
    movk   x12, #0x3, lsl #16        /* 200000 */
1:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 3f
    subs   x12, x12, #1
    b.ne   1b
    mov    x17, #-2
    ret
3:  ands   x17, x11, #0xe
    b.ne   4f
    ldr    x16, [x9, #0x18]
4:  ret

/* delay: 入 x12=迭代数 */
delay:
    cbz    x12, 2f
1:  subs   x12, x12, #1
    b.ne   1b
2:  ret
