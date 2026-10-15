//! MMU 恒等映射与 cache 使能（S0-4，NOLINUX-PLAN）。
//!
//! 参照=u1x-probe.S 机器已验形状（2026-10-13）：T0SZ=25 使 TTBR0_EL1
//! 直指单张 L1 表（1GB 块描述符、512 项×8B=4KB）；MAIR 三属性=
//! idx0 Device-nGnRE / idx1 Normal WB / idx2 Normal NC（DMA 区）。
//! 表本体由设备侧建（VA=PA 恒等，表址 4KB 对齐——本模块不藏机器
//! 布局）。

use core::arch::asm;

/// MAIR_EL1：byte0=0x04（nGnRE）byte1=0xFF（WB）byte2=0x44（NC）。
pub const MAIR: u64 = 0x00_44_FF_04;

/// TCR_EL1（u1x 同值）：T0SZ=25 | SH0=inner | TG0=4KB | IPS=40bit。
pub const TCR: u64 = 0x2_0000_3019;

/// SCTLR_EL1 开闸位：M（MMU）| C（D-cache）| I（I-cache）。
const SCTLR_ON: u64 = 0x1005;

/// 1GB 块描述符（L1）：attr0 Device-nGnRE。
pub fn dev_block(pa: u64) -> u64 {
    pa | 0x401
}

/// 1GB/2MB 块描述符：attr1 Normal WB。
pub fn wb_block(pa: u64) -> u64 {
    pa | 0x405
}

/// 块描述符：attr2 Normal 非缓存（DMA 区）。
pub fn nc_block(pa: u64) -> u64 {
    pa | 0x409
}

/// 表描述符：指向下一级表（valid+table=0b11）。
pub fn table(next: usize) -> u64 {
    next as u64 | 0x3
}

/// 装订并开闸（恒等映射下取指不断流——u1x 实证）。序列=清遗留
/// 翻译 → 配 TTBR/TCR/MAIR → 再清 → 开 M/C/I。
pub fn enable(ttbr: usize) {
    unsafe {
        asm!("tlbi vmalle1", "dsb sy", "isb", options(nostack));
        asm!(
            "msr ttbr0_el1, {}",
            "msr tcr_el1, {}",
            "msr mair_el1, {}",
            "isb",
            in(reg) ttbr as u64,
            in(reg) TCR,
            in(reg) MAIR,
            options(nostack)
        );
        asm!("tlbi vmalle1", "dsb sy", "isb", options(nostack));
        let s: u64;
        asm!("mrs {}, sctlr_el1", out(reg) s, options(nomem, nostack));
        asm!(
            "msr sctlr_el1, {}",
            "isb",
            in(reg) s | SCTLR_ON,
            options(nostack)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_shapes() {
        // 块位形：bit0=1（block）、AF=bit10、attrindx=bits[4:2]。
        assert_eq!(dev_block(0) & 0x7FF, 0x401);
        assert_eq!(wb_block(0) & 0x7FF, 0x405);
        assert_eq!(nc_block(0) & 0x7FF, 0x409);
        // 输出地址落在高位（1GB 对齐基址照搬）。
        assert_eq!(wb_block(0x8000_0000), 0x8000_0405);
        // 表描述符=valid+table。
        assert_eq!(table(0x1000) & 0x3, 0x3);
        assert_eq!(table(0x1000) >> 12 << 12, 0x1000);
    }

    #[test]
    fn mair_three_attrs() {
        assert_eq!(MAIR & 0xFF, 0x04); // idx0 nGnRE
        assert_eq!((MAIR >> 8) & 0xFF, 0xFF); // idx1 WB
        assert_eq!((MAIR >> 16) & 0xFF, 0x44); // idx2 NC
    }
}
