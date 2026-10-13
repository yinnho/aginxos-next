/* redfin 裸 substrate 第一级（Stage 2）。
 *
 * 【落点真相】ABL 按 arm64 协议装载：base=0x80008000（头里 text_offset
 * =0x80000 是我们自己写的），入口=镜像偏移0 的 code_0 支到 +0x40 ⇒
 * 代码实际从 0x80008040 起跑。linker.ld 直接链 0x80008040（=装载基
 * +64B 头），零迁移——L10–L15 六代「自迁移」件全冻死的根因：actual
 * (0x80008040) > linked(0x80008000) 走前向拷贝，把镜像拷到自己身上、
 * 边执行边自毁。（旧记载「load base=0x80000000」是推想非测量；L13
 * 位置无关小桩两种落点都活，证明不了落点。）
 *
 * start.S 只剩：掩中断 → 栈 → 清 bss → 装 VBAR 自诊断网 → bl kmain。
 * vectors 表在 .vectors 段（2KB 对齐，含在 _image_end 内）。
 */

.section .text.boot
.global _start
_start:
    /* 掩中断（防 ABL 残留定时器打进来，VBAR 不归我们管） */
    msr    daifset, #0xf

    /* 栈：镜像尾上方 64KB（避开 ABL 高区 ramdisk/dtb） */
    ldr    x10, =_image_end
    add    sp, x10, #0x10000

    /* 清 bss */
    ldr    x11, =__bss_start
    ldr    x12, =__bss_end
1:  cmp    x11, x12
    b.hs   2f
    stp    xzr, xzr, [x11], #16
    b      1b

2:  /* B2 自诊断向量网（16 槽全=PS_HOLD 复位）：异常→闪灭可判，
       不再冻死无证。总线 stall（load 永不返回）仍兜不住——那不是异常 */
    ldr    x10, =vectors
    msr    vbar_el1, x10
    isb

    bl     kmain
    /* kmain 不归；归了也只停尸 */
3:  wfe
    b      3b

/* 自诊断向量表（linker.ld 里 2KB 对齐，VBAR 硬要求）。
 * 16 槽 ×128B，槽槽=PS_HOLD 温复位；位置无关构造地址，无字面池。
 * 判读：闪灭+回滚后 dmesg 记 Hard/cold=异常真发生（看冻点在哪层）；
 * 记 Warm Reset=PON 全链通（异常从未发生）。 */
.section .vectors, "ax"
.align 11
.global vectors
vectors:
.rept 16
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]
4:  b      4b
    .space 128 - 16
.endr
