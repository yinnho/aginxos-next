/* sp.S —— B6 栈写入单变量实验（2026-10-13 夜）
 *
 * L13/B5（零内存写）全活；B1–B4（全写栈）全冻死。纸面排除法后唯一
 * 站着的假设：sp=image_end+0x10000 落在被 ABL 时代 XPU 配成「挂死区」
 * 的早期 DRAM——写它=load/store 永不应答（stall 非异常，VBAR 兜不住）。
 *
 * 本桩=L13 壳+唯一一条写：str 到 0x80018ff8（B4 探针循环变量的确切
 * 地址）。冻死=实锤；闪灭=栈洗清，转 UART 硬件仪器。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x9, #0x8001, lsl #16
    movk   x9, #0x9000             /* x9 = 0x80019000（与 B4 完全同源） */
    mov    sp, x9
    str    wzr, [sp, #-8]!         /* 唯一的写：0x80018ff8 */
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]
1:  b      1b
