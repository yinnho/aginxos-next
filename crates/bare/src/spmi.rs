//! SPMI PMIC arbiter v5 运输层（S1-1，NOLINUX-PLAN）。
//!
//! 参照 = 本机内核 drivers/spmi/spmi-pmic-arb.c v5 路径逐函数对账 +
//! RUNG2 asm substrate B13–B17 上机实测（TRUTH.md §2）：
//! - APID 表 core+0x900+4N（表项 0=空位，ppid=(v>>8)&0xFFF，bit24=IRQ
//!   owner 标）；owner 表 cnfg+0x700+4N（val&7=write_ee）。
//! - 读 = obsrvr+0x80*apid 通道窗（CMD@0x00 STATUS@0x08 RDATA0@0x18
//!   RDATA1@0x1C）；写 = chnls+0x10000*apid（先落 WDATA 再发 CMD，
//!   须 owner&7==ee——B7/B12 冻死案铁律）。
//! - cmd=(opc<<27)|((addr&0xff)<<4)|(len-1)&7；≤8B。
//! - 【S1-3b/S1-3c 寻址律第三律：PMIC 侧寄存器寻址三档】短命令（READ=15
//!   /WRITE=14）总线只带 **5 位**地址（≥0x20 截断，spmi_register_write
//!   文档明写）；EXT（EXT_READ=13/EXT_WRITE=2）带 8 位——但 PM7250B
//!   实测该空间为**死区**（S1-3b：写 ACK-全弃、读疑似低页别名，三寄存器
//!   两外设同病）；内核 charger regmap 地址是 flat 16 位（如 WD_CFG
//!   0x1651），regmap_spmi_ext 按 addr>0xFF 全走 **EXT-LONG**
//!   （spmi_ext_register_writel/readl → arb EXT_WRITEL=0/EXT_READL=1，
//!   16 位地址=periph 字节由 APID 路由承载+偏移 8 位，spmi-pmic-arb.c
//!   翻译表 0x30-0x37→op0/0x38-0x3F→op1 实锤）。本运输层因此**一律
//!   EXT-LONG**——这正是内核对此页的唯一走法。
//! - STATUS：轮询 DONE=bit0，DONE 后查 FAILURE=bit1/DENIED=bit2/
//!   DROPPED=bit3（内核 wait_for_done 同序）。
//!
//! 无状态设计（零缓存零 static）：每次 find 扫表 ≤512 项——探针与
//! 64s 喂狗周期下开销可忽略，免 BSS 摆表。

/// core+0x000：arbiter 版本（v5 ∈ [0x50000000, 0x70000000)）。
pub const CORE_VERSION: u32 = 0x000;
/// core+0x004：特性位图（bit[10:0]=APID 数真源，不拍 512）。
pub const CORE_FEATURES: u32 = 0x004;
/// core+0x900+4N：v5 APID→PPID 映射表（内核 apid_map_offset_v5）。
pub const APID_MAP_V5: u32 = 0x900;
/// cnfg+0x700+4N：所有权表（val&7=该 APID 的 write EE）。
pub const OWNERSHIP_TABLE: u32 = 0x700;

/// 通道内寄存器（读写窗同布局，内核 PMIC_ARB_* 同值）。
pub const CH_CMD: u32 = 0x00;
pub const CH_STATUS: u32 = 0x08;
pub const CH_WDATA0: u32 = 0x10;
pub const CH_RDATA0: u32 = 0x18;

/// arbiter 内部 opcode（非 SPMI 总线码；内核 PMIC_ARB_OP_*）。一律
/// EXT-LONG：短命令 5 位地址截断、8 位 EXT 空间 PM7250B 实测死区
/// （S1-3b），内核 charger regmap（flat 16 位地址>0xFF）全走 LONG
/// （S1-3c）。periph 字节由 APID 路由承载，cmd 只带 8 位偏移。
pub const OP_EXT_READL: u32 = 1;
pub const OP_EXT_WRITEL: u32 = 0;

const ST_DONE: u32 = 1 << 0;
const ST_FAILURE: u32 = 1 << 1;
const ST_DENIED: u32 = 1 << 2;
const ST_DROPPED: u32 = 1 << 3;

/// 事务结局（status=终读 STATUS 原值，定位用）。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TErr {
    /// DONE 未置位超时（总线/映射病）。
    Timeout,
    /// DENIED——owner 门拒（选错 APID 副本）。
    Denied(u32),
    /// FAILURE——外设拒收。
    Failed(u32),
    /// DROPPED——总线丢弃。
    Dropped(u32),
}

/// v5 判定（内核同式：[0x50000000, 0x70000000)）。
pub fn is_v5(version: u32) -> bool {
    (0x5000_0000..0x7000_0000).contains(&version)
}

/// APID 数真源 = FEATURES & 0x7FF。
pub fn apid_count(features: u32) -> u32 {
    features & 0x7FF
}

/// 表项解包：None=空位（v==0 跳过）；Some((ppid, irq_owner_bit24))。
pub fn map_entry(v: u32) -> Option<(u32, bool)> {
    if v == 0 {
        None
    } else {
        Some(((v >> 8) & 0xFFF, v & (1 << 24) != 0))
    }
}

/// 命令字 fmt_cmd_v2（内核逐位同式；地址域只有 8 位，高位由 PPID 承载）。
pub fn fmt_cmd(opc: u32, addr: u8, len: usize) -> u32 {
    (opc << 27) | ((addr as u32 & 0xFF) << 4) | ((len as u32 - 1) & 0x7)
}

/// v5 五区窗句柄（机器基址由调用方从 facts 注入，ee=0/channel=0 写死
/// APPS——多 EE 门由 find_apid_owner 承担）。
pub struct Arb {
    pub core: usize,
    pub chnls: usize,
    pub obsrvr: usize,
    pub cnfg: usize,
}

fn rd32(base: usize, off: u32) -> u32 {
    unsafe { ((base + off as usize) as *const u32).read_volatile() }
}

fn wr32(base: usize, off: u32, v: u32) {
    unsafe { ((base + off as usize) as *mut u32).write_volatile(v) }
}

impl Arb {
    pub fn version(&self) -> u32 {
        rd32(self.core, CORE_VERSION)
    }

    pub fn features(&self) -> u32 {
        rd32(self.core, CORE_FEATURES)
    }

    /// owner 表读：val&7 = 该 APID 的 write EE。
    pub fn owner(&self, apid: u32) -> u32 {
        rd32(self.cnfg, OWNERSHIP_TABLE + 4 * apid) & 7
    }

    /// APID 表项直读（探针全副本扫描用）。
    pub fn map_at(&self, apid: u32) -> Option<(u32, bool)> {
        map_entry(rd32(self.core, APID_MAP_V5 + 4 * apid))
    }

    /// 找 PPID 的**任意** APID 副本（读观察窗无 owner 门）。
    pub fn find_apid_read(&self, ppid: u32, apid_max: u32) -> Option<u32> {
        for apid in 0..apid_max {
            if let Some((p, _)) = map_entry(rd32(self.core, APID_MAP_V5 + 4 * apid)) {
                if p == ppid {
                    return Some(apid);
                }
            }
        }
        None
    }

    /// 找 PPID 的 owner-gated 副本：write_ee==ee（B7/B12 铁律——取首个
    /// 匹配撞非 owner 副本=冻死；内核扫描同式：本 EE 副本覆盖首匹配）。
    pub fn find_apid_owner(&self, ppid: u32, ee: u32, apid_max: u32) -> Option<u32> {
        let mut hit: Option<u32> = None;
        for apid in 0..apid_max {
            if let Some((p, _)) = map_entry(rd32(self.core, APID_MAP_V5 + 4 * apid)) {
                if p == ppid {
                    hit = Some(apid);
                    if self.owner(apid) == ee {
                        return Some(apid);
                    }
                }
            }
        }
        // 无本 EE 副本：回最后一个副本（调用方写会吃 DENIED 落环定位）。
        hit
    }

    fn wait_done(base: usize, off: usize) -> Result<(), TErr> {
        // 内核 PMIC_ARB_TIMEOUT_US=100（µs 级）；裸件无 udelay，1M 次
        // 轮询 ≈ 数 ms，覆盖足够且不冒挂死风险（超时可落环复位）。
        for _ in 0..1_000_000 {
            let st = rd32(base + off, CH_STATUS);
            if st & ST_DONE != 0 {
                if st & ST_DENIED != 0 {
                    return Err(TErr::Denied(st));
                }
                if st & ST_FAILURE != 0 {
                    return Err(TErr::Failed(st));
                }
                if st & ST_DROPPED != 0 {
                    return Err(TErr::Dropped(st));
                }
                return Ok(());
            }
        }
        Err(TErr::Timeout)
    }

    /// SPMI 读（≤8B）：任意副本皆可。addr 为 PPID 内 8 位寄存器偏移。
    pub fn read(&self, apid: u32, addr: u8, buf: &mut [u8]) -> Result<(), TErr> {
        assert!(buf.len() >= 1 && buf.len() <= 8);
        let off = 0x80 * apid as usize;
        wr32(self.obsrvr + off, CH_CMD, fmt_cmd(OP_EXT_READL, addr, buf.len()));
        Self::wait_done(self.obsrvr, off)?;
        let d0 = rd32(self.obsrvr + off, CH_RDATA0).to_le_bytes();
        let d1 = rd32(self.obsrvr + off, CH_RDATA0 + 4).to_le_bytes();
        let n = buf.len().min(4);
        buf[..n].copy_from_slice(&d0[..n]);
        if buf.len() > 4 {
            let m = buf.len() - 4;
            buf[4..].copy_from_slice(&d1[..m]);
        }
        Ok(())
    }

    /// SPMI 写（≤8B）：**调用方自证 owner**（本方法不重查——内核同式
    /// 在 offset 计算处门禁，选副本的职责在 find_apid_owner）。
    /// 内核同序：WDATA 先落、CMD 后发。
    pub fn write(&self, apid: u32, addr: u8, data: &[u8]) -> Result<(), TErr> {
        assert!(data.len() >= 1 && data.len() <= 8);
        let off = 0x10000 * apid as usize;
        let mut w0 = [0u8; 4];
        w0[..data.len().min(4)].copy_from_slice(&data[..data.len().min(4)]);
        wr32(self.chnls + off, CH_WDATA0, u32::from_le_bytes(w0));
        if data.len() > 4 {
            let mut w1 = [0u8; 4];
            w1.copy_from_slice(&data[4..]);
            wr32(self.chnls + off, CH_WDATA0 + 4, u32::from_le_bytes(w1));
        }
        wr32(self.chnls + off, CH_CMD, fmt_cmd(OP_EXT_WRITEL, addr, data.len()));
        Self::wait_done(self.chnls, off)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_cmd_matches_kernel_v2() {
        // (opc<<27)|((addr&0xff)<<4)|(bc&7)，bc=len-1。
        assert_eq!(fmt_cmd(OP_EXT_READL, 0x06, 1), (1 << 27) | (0x06 << 4) | 0);
        assert_eq!(fmt_cmd(OP_EXT_WRITEL, 0x43, 1), (0 << 27) | (0x43 << 4) | 0);
        // 地址域 8 位全带：0x1043 的 0x10 由 PPID/APID 承载，cmd 收全
        // 0x43——短命令在总线截到 5 位、8 位 EXT 在 PM7250B 是死区，
        // LONG（op0/op1）才达 charger 页（S1-3b/S1-3c 立法）。
        assert_eq!(fmt_cmd(OP_EXT_WRITEL, 0x43, 2) & 0x0000_0FF0, 0x430);
        assert_eq!(fmt_cmd(OP_EXT_READL, 0x51, 1) & 0x0000_0FF0, 0x510);
        // len=8 → bc=7。
        assert_eq!(fmt_cmd(OP_EXT_READL, 0x00, 8) & 7, 7);
    }

    #[test]
    fn extl_opcodes_match_kernel_enum() {
        // spmi-pmic-arb.c PMIC_ARB_OP_*：EXT_WRITEL=0/EXT_READL=1（内核
        // charger regmap flat 16 位地址>0xFF 全走 LONG——S1-3c）。死区档：
        // 短 READ=15/WRITE=14（5 位截断）、8 位 EXT_READ=13/EXT_WRITE=2
        // （PM7250B 写全弃）。
        assert_eq!(OP_EXT_READL, 1);
        assert_eq!(OP_EXT_WRITEL, 0);
    }

    #[test]
    fn v5_version_window() {
        assert!(is_v5(0x5001_0000));
        assert!(is_v5(0x5FFF_FFFF));
        assert!(!is_v5(0x4001_2345));
        assert!(!is_v5(0x7000_0000));
    }

    #[test]
    fn map_entry_decode() {
        assert_eq!(map_entry(0), None);
        // ppid=(v>>8)&0xFFF（12 位窗），bit24=IRQ owner。
        let (ppid, irq) = map_entry(0x0102_1000).unwrap();
        assert_eq!(ppid, 0x210);
        assert!(irq);
        let (ppid, irq) = map_entry(0x0002_1600).unwrap();
        assert_eq!(ppid, 0x216);
        assert!(!irq);
    }

    #[test]
    fn apid_count_from_features() {
        assert_eq!(apid_count(0x0143), 0x143);
        assert_eq!(apid_count(0xFFFF_FFFF) & !0x7FF, 0);
    }
}
