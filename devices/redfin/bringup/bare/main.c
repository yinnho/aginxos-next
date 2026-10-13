/* redfin 裸 substrate 第一级 main：无 Linux 内核的自证心跳。
 *
 * 行为（验收面=手机自己温重启回 Linux）：
 *   1. SPMI 找 CHGR(PPID 0x210)/PON(0x208) 的 APID（battery-truth.md §6）
 *   2. 插电态保命：充电使能 0x1042 bit0 复位 + 周期喂狗 0x1643←1
 *   3. 延时 ~15s（喂狗循环里过）
 *   4. PON 装弹 warm 复位（0x85A/0x85B）→ 写 0 到 PS_HOLD 0x0c264000
 *
 * 全部寄存器协议真源=devices/redfin/bringup/battery-truth.md（源↔二进制↔
 * 活体三对齐）。无 console：唯一的输出通道就是复位本身。
 */

typedef unsigned int u32;
typedef unsigned long long u64;
typedef unsigned char u8;

#define SPMI_CORE   0x0c440000UL
#define SPMI_CHNLS  0x0c600000UL
#define SPMI_OBSRVR 0x0e600000UL
#define PS_HOLD     0x0c264000UL

/* PM7250B（SID 2）外设 PPID —— battery-truth §6 表 */
#define PPID_CHGR 0x210
#define PPID_PON  0x208

static inline void wr32(u64 a, u32 v) { *(volatile u32 *)a = v; }
static inline u32 rd32(u64 a) { return *(volatile u32 *)a; }

/* APID 自发现：扫 core+0x900+4N（v5 表 512 项），ppid=(v>>8)&0xfff */
static int apid_for(u32 ppid)
{
    for (u32 n = 0; n < 512; n++) {
        u32 v = rd32(SPMI_CORE + 0x900 + 4 * n);
        if (v == 0)
            continue;
        if (((v >> 8) & 0xfff) == ppid)
            return (int)n;
    }
    return -1;
}

/* SPMI v5 单事务写（≤8B）：WDATA0@0x10 WDATA1@0x14 CMD@0x00 STATUS@0x08 */
static int spmi_wr(int apid, u32 reg, const u8 *d, int len)
{
    u64 ch = SPMI_CHNLS + 0x10000UL * (u32)apid;
    u32 w0 = 0, w1 = 0;
    for (int i = 0; i < len && i < 8; i++) {
        if (i < 4) w0 |= (u32)d[i] << (8 * i);
        else       w1 |= (u32)d[i] << (8 * (i - 4));
    }
    wr32(ch + 0x10, w0);
    if (len > 4)
        wr32(ch + 0x14, w1);
    wr32(ch + 0x00, (14u << 27) | ((reg & 0xff) << 4) | (u32)((len - 1) & 7));
    for (int t = 0; t < 200000; t++) {
        u32 s = rd32(ch + 0x08);
        if (s & 1) /* DONE */
            return (s & 0xe) ? -1 : 0; /* FAILURE/DENIED/DROPPED */
    }
    return -2;
}

/* SPMI v5 读：obsrvr+0x80*APID，RDATA0@0x18 */
static int spmi_rd(int apid, u32 reg, u8 *d, int len)
{
    u64 ch = SPMI_OBSRVR + 0x80UL * (u32)apid;
    wr32(ch + 0x00, (15u << 27) | ((reg & 0xff) << 4) | (u32)((len - 1) & 7));
    for (int t = 0; t < 200000; t++) {
        u32 s = rd32(ch + 0x08);
        if (s & 1) {
            if (s & 0xe)
                return -1;
            u32 r0 = rd32(ch + 0x18);
            for (int i = 0; i < len && i < 4; i++)
                d[i] = (u8)(r0 >> (8 * i));
            return 0;
        }
    }
    return -2;
}

static void delay_iters(u64 n)
{
    volatile u64 i = n;
    while (i-- > 0)
        ;
}

/* PON 装弹 + SoC 拉闸：warm 复位（battery-truth §7 两段式） */
static void pon_warm_reset(int apid_pon)
{
    if (apid_pon >= 0) {
        u8 b = 0;
        if (spmi_rd(apid_pon, 0x85b, &b, 1) == 0) { /* 清 RESET_EN */
            b &= (u8)~0x80u;
            spmi_wr(apid_pon, 0x85b, &b, 1);
        }
        delay_iters(500000); /* ≥500µs（睡眠钟容差） */
        u8 warm = 0x01;
        spmi_wr(apid_pon, 0x85a, &warm, 1); /* 装弹 warm */
        if (spmi_rd(apid_pon, 0x85b, &b, 1) == 0) { /* 拉保险 */
            b |= 0x80;
            spmi_wr(apid_pon, 0x85b, &b, 1);
        }
    }
    /* 事务均已轮询 DONE——空闲窗拉闸（§7 铁律的免 SCM 变体） */
    wr32(PS_HOLD, 0);
    delay_iters(1000000);
    wr32(PS_HOLD, 0); /* 万一没死透，再补一刀 */
}

void kmain(void)
{
    int apid_chgr = apid_for(PPID_CHGR);
    int apid_pon  = apid_for(PPID_PON);

    if (apid_chgr >= 0) {
        /* 充电使能复位（RMW 0x1042 bit0）——插电态保命其一 */
        u8 b = 0;
        if (spmi_rd(apid_chgr, 0x1042, &b, 1) == 0) {
            b |= 0x01;
            spmi_wr(apid_chgr, 0x1042, &b, 1);
        }
        /* 喂狗=写 1→0x1643；延时期间周期喂，钟速不敏感 */
        for (int round = 0; round < 5; round++) {
            u8 pet = 1;
            spmi_wr(apid_chgr, 0x1643, &pet, 1);
            delay_iters(1000000000ULL); /* ~1-4s/轮，5 轮 */
        }
    } else {
        delay_iters(5000000000ULL);
    }

    pon_warm_reset(apid_pon);

    /* 不该到这：原地停尸，长按电源可离场 */
    for (;;)
        ;
}
