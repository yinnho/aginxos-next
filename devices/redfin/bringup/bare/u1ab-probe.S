/* U1AB — mrs CurrentEL 隔离审（阶梯 3 刀 1-ab，2026-10-14）
 *
 * 前账：刀 0 calib 对照活（三重证：blink×3 烧尽 tries:a=0 落 b）；刀 1-aa
 * （calib+{mrs,lsr,sub,add,嵌套循环}）冻死三重证（眼判常亮+adb 12min 全
 * 黑+tries:a=2 待验）。按架构常识该增量集不该挂——分析有洞，刀拆增量。
 *
 * 本刀 = calib.S 原文 + 唯一一处增量：`mrs x0, CurrentEL`（读而不用，
 * x0 随后被 calib 原文的 movz 覆盖）。审头号嫌疑「sysreg 读」本身。
 *
 * 判读（与 calib 同节奏 = 同闪长）：
 *   blink×3 = mrs 清白 → 增量罪在 ALU 链/嵌套结构，下一刀拆结构（u1ac）
 *   常亮冻死 = mrs CurrentEL 死罪（硅级怪癖/接管面有陷阱）→ 后续一切
 *          探针禁读 sysreg，EL 盲按 EL1 装网（错则异常兜底=冻死，可接受）
 */
.section .text.boot
.globl _start
_start:
    msr    daifset, #0xf
    mrs    x0, CurrentEL          /* 唯一增量：读而不用 */
    movz   x1, #0x05F5, lsl #16
    movk   x1, #0xE100            /* x1 = 0x05F5E100 = 100,000,000 */
1:  subs   x1, x1, #1
    b.ne   1b
    movz   x0, #0x0c26, lsl #16
    movk   x0, #0x4000            /* x0 = 0x0c264000 PS_HOLD */
    str    wzr, [x0]
2:  b      2b
