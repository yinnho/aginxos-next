/* spmi-probe.S —— B8 SPMI_CORE 读探针（2026-10-13 深夜）
 *
 * B7（位无关+纯寄存器全量 substrate）冻死；与 B5 的唯一差类 = SPMI 区域
 * 访问。本桩 = B5 已证壳 + 唯一一条新指令：读 SPMI_CORE+0x900（apid 表
 * 首项）。闪灭 = 表可读，冻点在更深处（CHNLS/OBSRVR 写）；冻死 =
 * 0x0c440000 窗口此阶段被 fence（XPU 挂死区，读=永不应答）。
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    movz   x9, #0x0c44, lsl #16
    ldr    w0, [x9, #0x900]         /* 唯一探针：SPMI_CORE apid 表首项 */
    movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100              /* 1e8 校准过的延时 */
1:  subs   x1, x1, #1
    b.ne   1b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000
    str    wzr, [x0]
2:  b      2b
