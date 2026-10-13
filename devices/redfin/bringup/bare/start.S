/* redfin 裸 substrate 第一级（Stage 2 rung 1）。
 *
 * ABL 事实（stock 内核 blob 0 偏移即 A64 指令、无 arm64 Image 头）：
 * kernel blob 被当裸入口码跳，x0=dtb，MMU/中断已关。
 * 加载地址不确定（0x80000000 或 0x80008000 候选）——本头自迁移吸收：
 * 算 delta，整段搬到链接基址（0x80008000），再跳真身。
 * 链接点全在本文件：.text.boot 段必须在最前（见 linker.ld KEEP）。
 *
 * 2026-10-13 实测（L13 无迁移件首证）：load base=0x80000000、入口=镜像
 * 偏移 0（经 code_0 支到 +0x40）⇒ delta=0x80000 远大于小镜像。旧后向
 * 拷贝下界误用链接地址 x10：循环一字节即退，br 落未拷贝区=L10/L11/
 * L12a 冻 logo 根因。已改存 actual 起点（x18）为下界。
 */

.section .text.boot
.global _start
_start:
    /* 掩中断（防 ABL 残留定时器打进来，VBAR 不归我们管） */
    msr    daifset, #0xf

1:  adr    x9, 1b                  /* 实际运行地址 */
    ldr    x10, =1b                /* 链接地址 */
    cmp    x9, x10
    b.eq   3f                      /* 落点即链接点，免搬 */

    /* delta = x12 = linked - actual；按方向选前/后向字节拷贝 */
    ldr    x11, =_image_end        /* 链接尾 */
    ldr    x13, =_start
    sub    x14, x11, x13           /* size */
    cmp    x9, x10
    b.lt   2f                      /* actual < linked：dst>src → 后向 */

    /* 前向（actual > linked，dst < src） */
    add    x15, x9, x14            /* src end */
    mov    x16, x13                /* dst = linked base */
4:  ldrb   w17, [x9], #1
    strb   w17, [x16], #1
    cmp    x9, x15
    b.lt   4b
    b      3f

2:  /* 后向（actual < linked，dst > src，重叠区安全） */
    mov    x18, x9                 /* 下界=actual 起点（2026-10-13 修：
                                       原误用链接地址 x10，delta>镜像时一字节即退） */
    add    x9, x9, x14             /* src end */
    add    x16, x11, #0            /* dst end = linked end */
5:  subs   x9, x9, #1
    ldrb   w17, [x9]
    subs   x16, x16, #1
    strb   w17, [x16]
    cmp    x9, x18
    b.ge   5b

3:  /* 已在链接世界，跳链接地址的真身 */
    ldr    x10, =_start2
    br     x10

.section .text
.type _start2, %function
_start2:
    /* 栈：镜像尾上方 64KB（避开 ABL 高区 ramdisk/dtb） */
    ldr    x10, =_image_end
    add    sp, x10, #0x10000

    /* 清 bss */
    ldr    x11, =__bss_start
    ldr    x12, =__bss_end
6:  cmp    x11, x12
    b.hs   7f
    stp    xzr, xzr, [x11], #16
    b      6b

7:  mov    x0, xzr
    bl     kmain
    /* kmain 不归；归了也只停尸 */
8:  wfe
    b      8b
