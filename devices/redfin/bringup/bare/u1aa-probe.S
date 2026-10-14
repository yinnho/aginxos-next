/* U1AA — mrs CurrentEL 单件（阶梯 3 刀 1-aa，2026-10-14）
 *
 * 刀 0（calib 对照重闪）裁定前提：机器健康、六连冻=sysreg 增量真凶。
 * 本刀 = calib 骨架 + 唯一增量 `mrs CurrentEL`（含 lsr/sub 消费），
 * 无 VBAR / 无 isb / 无 mul / 无异常网 / 无 DRAM 数据访问。
 *
 * 六连冻件的最小公共 sysreg 前缀就是 mrs CurrentEL——u1y 增量集 =
 * {mrs, lsr, sub, adr, cbz/cmp/b.eq, msr VBAR, isb, add, mul}，
 * 其中架构上唯一非纯 ALU 的只有 mrs 与 msr。先审 mrs。
 *
 * 判读（闪长 = EL 编码，每轮数秒数；闪灭×3 后落 b 无头黑屏）：
 *   1s×3 = EL1：mrs 清白 → 下一刀加 msr VBAR_EL1（u1ab）
 *   2s×3 = EL2：同上且 MMU 寄存器面用 EL2 套
 *   3s×3 = EL3：同上且 EL3 套
 *   冻死 = mrs CurrentEL 本身死（硅级怪癖）→ 后续刀禁读 CurrentEL，
 *          盲按 EL1 装网（错则异常兜底=冻死，可接受的单点赌）
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf

    /* 唯一增量：读 CurrentEL 并折进闪长 */
    mrs    x0, CurrentEL
    lsr    x19, x0, #2
    sub    x19, x19, #1             /* 0/1/2 = EL1/2/3 */
    add    x20, x19, #1             /* 1/2/3 */

    /* calib 同款延时骨架，乘数换 x20（无 mul：用移位+加法链装 1e8×n 不值，
     * 直接把循环计数放大 n 倍更朴素：外层 ×x20 内层 1e8——但多层循环
     * 也是新指令（b/cbnz 已在 calib 词汇表内，subs/b.ne 同）。
     * 取最简：单层 1e8 循环 × 外层 x20 轮，全部用已证指令。 */
    movz   x13, #0xE100
    movk   x13, #0x05F5, lsl #16    /* 1e8 */
1:  subs   x12, x13, #0
2:  subs   x12, x12, #1
    b.ne   2b
    subs   x20, x20, #1
    b.ne   1b

    movz   x9, #0x0c26, lsl #16
    movk   x9, #0x4000             /* x9 = 0x0c264000 PS_HOLD（u1aa 首版
                                      movz #0xc264,lsl#16 = 0xC2640000 写歪
                                      →外部中止→冻死；全场 19 件同雷已翻案） */
    str    wzr, [x9]
3:  b      3b
