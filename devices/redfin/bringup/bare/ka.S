/* ka.S —��� 阶梯 2 在役裸 substrate（RUNG2，2026-10-14）
 *
 * redfin 整机在役态正身：无 Linux 内核，充电使能 + 永续喂狗环，**不复位**
 * （复位=tries 烧命，是阶梯 1 的动词；本刀稳态=Google logo 静止，早期崩
 * =blink×3 落黑屏 L0——二值判据见 RUNG2.md §2）。
 *
 * 主循环：owner-gated 选 CHGR(0x210) 的 APPS 副本 → 充电使能 RMW
 * 0x1042 bit0（一次）→ forever { 喂狗 0x1643←1；delay 3e9（数秒级，
 * ≪64s 咬窗） }。
 *
 * 寄存器法承 pi2.S（TRUTH.md §1/§2 契约）：位置无关、零字面量、零 DRAM
 * 写。x3=CORE x4=CHNLS x5=OBSRVR x19=CNFG x7=apid_chgr。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x3, #0x0c44, lsl #16      /* SPMI_CORE  0x0c440000 */
    movz   x4, #0x0c60, lsl #16      /* SPMI_CHNLS 0x0c600000 */
    movz   x5, #0x0e60, lsl #16      /* SPMI_OBSRVR 0x0e600000 */
    movz   x19, #0xa000
    movk   x19, #0x0c40, lsl #16     /* CNFG       0x0c40a000 */

    movz   x13, #0x210               /* PPID_CHGR */
    bl     apid_own
    cmp    x14, #0
    b.lt   2f                        /* 无 APPS 副本 → 直入喂狗环 */

    /* —— 充电使能 RMW 0x1042 bit0（一次）—— */
    mov    x7, x14
    mov    x15, #0x1042
    bl     spmi_rd1
    cbnz   x17, 2f                   /* 读败 → 跳过（环仍活）*/
    orr    x16, x16, #1
    mov    x14, x7
    mov    x15, #0x1042
    bl     spmi_wr1

    /* —— 永续喂狗环 —— */
2:  cmp    x7, #0
    b.lt   3f
    mov    x14, x7
    mov    x15, #0x1643
    mov    x16, #1
    bl     spmi_wr1                  /* 喂狗（败也不出环）*/
3:  movz   x12, #0x5E00
    movk   x12, #0xB2D0, lsl #16     /* 3e9 延时 */
    bl     delay
    b      2b

/* apid_own: 入 x13=ppid → 出 x14=owner==0 的 apid（-1=无）*/
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
    b.eq   4f
3:  add    x14, x14, #1
    b      1b
2:  mov    x14, #-1
4:  ret

/* spmi_wr1: 入 x14=apid x15=reg x16=字节 → 出 x17(0=成) */
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
    movk   x12, #0x3, lsl #16
1:  ldr    w11, [x9, #8]
    tbnz   w11, #0, 3f
    subs   x12, x12, #1
    b.ne   1b
    mov    x17, #-2
    ret
3:  and    x17, x11, #0xe
    ret

/* spmi_rd1: 入 x14=apid x15=reg → 出 x17(0=成) x16=字节 */
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
    movk   x12, #0x3, lsl #16
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
