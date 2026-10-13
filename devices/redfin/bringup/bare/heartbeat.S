/* heartbeat.S —— redfin 裸 substrate 阶梯 1 正身（2026-10-13 L13 首证）
 *
 * 36 字节位置无关 stub：无迁移、无栈、无 C runtime、无绝对地址。
 * ABL 发射后第一条指令就写 PS_HOLD=0（0x0c264000）×3 → SoC 即刻温复位。
 * 心跳=logo 一闪即灭；A/B tries 3→0 后 ABL 自动回滚 b 槽进 L0 Linux。
 *
 * 实证链（收据见 docs/HARDWARE.md ledger）：
 *   - L13 发射 ×3 = 温复位 ×3（slot a tries 3→0、unbootable=yes，
 *     36 字节件不可能自己进 Linux；发射前拒绝不烧 tries）；
 *   - 对比 L10/L11/L12a（带 start.S 迁移路径）全部冻 logo ⇒ 崩点在
 *     start.S 的迁移/栈/bss，不在 PS_HOLD 动词。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000          /* x0 = 0x0c264000 PS_HOLD */
    str    wzr, [x0]
    nop
    str    wzr, [x0]
    nop
    str    wzr, [x0]
1:  b      1b
