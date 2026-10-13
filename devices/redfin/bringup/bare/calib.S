/* calib.S —— B5 时钟标定桩（2026-10-13 夜）
 *
 * B1–B4 全部「冻死」，但全部兼容另一解释：ABL 交接的钟是慢钟，延时环
 * 分钟级一轮，肉眼即「永久冻死」。本桩 = L13 已证壳（零栈/零字面/零
 * bl，纯寄存器位置无关）+ 立即数延时 1e8 次迭代再 PS_HOLD。
 *
 * 测法：秒表量闪灭周期 N 秒 ⇒ 钟速 ≈ (2~3 × 1e8) / N Hz。
 *   N≈0.1s ⇒ ~2-3GHz（B4 另有死因）
 *   N≈3s   ⇒ ~100MHz
 *   N≈15s  ⇒ ~20MHz（B4 的 2e9 延时=每轮十几分钟——它可能一直活着）
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100             /* x1 = 0x05F5E100 = 100,000,000 */
1:  subs   x1, x1, #1
    b.ne   1b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000             /* x0 = 0x0c264000 PS_HOLD */
    str    wzr, [x0]
2:  b      2b
