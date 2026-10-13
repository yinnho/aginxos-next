/* B3 C 入口证（阶梯 bisect 第 3 级）——不碰任何未证外设。
 *
 * B1/B2（含 SPMI 的 kmain）双双冻死无闪 ⇒ 冻点在 kmain 首条 SPMI 读
 * 或更早。B2 的 VBAR 网都没兜住 ⇒ 头号嫌疑=总线 stall（未上钟外设的
 * load 永不返回，非异常、向量网够不着）。
 *
 * B3 只做：C 入口 → 延时 ~2-8s（可见冻结窗）→ 写 PS_HOLD（L13 已证
 * 裸可达）。判读：
 *   闪灭×3+回滚   ⇒ start.S/迁移/C 世界全通，墙=SPMI 裸读 stall；
 *   冻死          ⇒ 死在 start.S/入口本身 —— 必须上 UART（SuzyQ）。
 */

typedef unsigned int u32;
typedef unsigned long long u64;

#define PS_HOLD 0x0c264000UL

static inline void wr32(u64 a, u32 v) { *(volatile u32 *)a = v; }

static void delay_iters(u64 n)
{
    volatile u64 i = n;
    while (i-- > 0)
        ;
}

void kmain(void)
{
    delay_iters(2000000000ULL); /* ~2-8s 可见冻结窗（与 L13 即闪可辨） */
    wr32(PS_HOLD, 0);
    delay_iters(1000000);
    wr32(PS_HOLD, 0); /* 万一没死透，再补一刀 */
    for (;;)
        ;
}
