//! GICv3 最小中断面（S0-5，NOLINUX-PLAN）。
//!
//! 参照 = 本机内核 drivers/irqchip/irq-gic-v3.c（redbull 树）序列蒸馏：
//! dist 0→RWP→ARE_NS|G1A|G1；RD 唤醒 = WAKER 清 ProcessorSleep 轮询
//! ChildrenAsleep 清零；SGI/PPI 配置面在 RD+0x10000 第二帧
//! （IGROUPR0/ISENABLER0/IPRIORITYR）；CPU 接口走系统寄存器
//! （SRE→PMR→IGRPEN1）。基址/帧数/stride/INTID 全由设备侧传入（D14）。
//!
//! RD 匹配式照抄 __gic_populate_rdist：`(typer >> 32) == aff`，其中
//! aff = Aff3<<24|Aff2<<16|Aff1<<8|Aff0（MPIDR 分字段提取——不能裸
//! `mpidr & 0xFFFFFFFF`，会把 MT 位 24 混进、Aff3 错位）。

/// GICD_CTLR： distributor 控制（RWP=bit31，ARE_NS=bit4，G1A=bit1，G1=bit0）。
pub const DIST_CTLR: u32 = 0x000;
/// GICD_TYPER： 中断容量/特性（内核 gic_init_bases 的第一次 GIC MMIO）。
pub const DIST_TYPER: u32 = 0x008;
/// GICR_TYPER： RD 身份（[63:32]=affinity，Last/ProcessorNumber 在低位）。
pub const RD_TYPER: u32 = 0x008;
/// GICR_WAKER： ProcessorSleep=bit1，ChildrenAsleep=bit2。
pub const RD_WAKER: u32 = 0x014;
/// SGI/PPI 配置帧相对 RD 的偏移（内核 GICR_SGI_BASE 的 SGI_OFFSET）。
pub const SGI_BASE: u32 = 0x1_0000;
/// IGROUPR0（相对 SGI 帧）： INTID 0–31 的组选择（1=Group1 NS）。
pub const SGI_IGROUPR0: u32 = 0x080;
/// ISENABLER0（相对 SGI 帧）： 写 1 set（读改写不必，写 0 无副作用）。
pub const SGI_ISENABLER0: u32 = 0x100;
/// IPRIORITYR 基（相对 SGI 帧）： 一寄存器 4 个 INTID、一字节一个。
pub const SGI_IPRIORITYR: u32 = 0x400;

/// dist 使能字 = ARE_NS|G1A|G1（内核 gic_dist_init 写 0x13）。
pub const DIST_CTLR_ARE_G1: u32 = 0x13;
/// CTLR.RWP（写后待清零位）。
pub const CTLR_RWP: u32 = 1 << 31;

/// Device 内存 32 位读。
#[inline]
pub fn rd32(base: usize, off: u32) -> u32 {
    unsafe { ((base + off as usize) as *const u32).read_volatile() }
}

/// Device 内存 32 位写。
#[inline]
pub fn wr32(base: usize, off: u32, v: u32) {
    unsafe { ((base + off as usize) as *mut u32).write_volatile(v) }
}

/// Device 内存 64 位读（TYPER）。
#[inline]
pub fn rd64(base: usize, off: u32) -> u64 {
    unsafe { ((base + off as usize) as *const u64).read_volatile() }
}

/// IPRIORITYR 寄存器偏移（一寄存器 4 个 INTID）。纯函数供测试。
pub fn prio_off(intid: u32) -> u32 {
    SGI_IPRIORITYR + 4 * (intid >> 2)
}

/// IPRIORITYR 内字节移位（intid&3 号字节）。纯函数供测试。
pub fn prio_shift(intid: u32) -> u32 {
    8 * (intid & 3)
}

/// MPIDR → RD 匹用 affinity（Aff3<<24|Aff2<<16|Aff1<<8|Aff0）。纯函数供测试。
pub fn mpidr_aff(mpidr: u64) -> u32 {
    (((mpidr >> 32) & 0xFF) << 24
        | ((mpidr >> 16) & 0xFF) << 16
        | ((mpidr >> 8) & 0xFF) << 8
        | (mpidr & 0xFF)) as u32
}

/// 扫 RD 帧找本核（内核 __gic_populate_rdist 同式；typer==0 的空帧
/// 提前止损——未实现帧读回零，继续扫只会白走）。
pub fn find_rd(gicr: usize, rd_count: u32, stride: u32, mpidr: u64) -> Option<usize> {
    let want = mpidr_aff(mpidr) as u64;
    for i in 0..rd_count {
        let rd = gicr + i as usize * stride as usize;
        let typer = rd64(rd, RD_TYPER);
        if typer == 0 {
            break;
        }
        if (typer >> 32) == want {
            return Some(rd);
        }
    }
    None
}

/// 唤醒本核 RD：清 ProcessorSleep，轮询 ChildrenAsleep 清零（内核
/// gic_enable_redist 前半；超时返回——终值里的残位就是证据）。
pub fn wake_rd(rd: usize, spin: u32) -> u32 {
    let v = rd32(rd, RD_WAKER) & !0x2;
    wr32(rd, RD_WAKER, v);
    let mut n = spin;
    loop {
        let v = rd32(rd, RD_WAKER);
        if v & 0x4 == 0 {
            return v;
        }
        if n == 0 {
            return v;
        }
        n -= 1;
        core::hint::spin_loop();
    }
}

/// dist：0→RWP→ARE|G1A|G1→RWP（内核 gic_dist_init 头两步；使能后
/// RWP 等待内核略过，此处保留——无害且稳）。返回 CTLR 终读。
pub fn dist_init(gicd: usize, spin: u32) -> u32 {
    wr32(gicd, DIST_CTLR, 0);
    wait_rwp(gicd, spin);
    wr32(gicd, DIST_CTLR, DIST_CTLR_ARE_G1);
    wait_rwp(gicd, spin);
    rd32(gicd, DIST_CTLR)
}

fn wait_rwp(gicd: usize, spin: u32) {
    let mut n = spin;
    while rd32(gicd, DIST_CTLR) & CTLR_RWP != 0 {
        if n == 0 {
            return;
        }
        n -= 1;
        core::hint::spin_loop();
    }
}

/// INTID<32（SGI/PPI）配 Group1-NS + 使能 + 优先级（prio 须 < PMR 才
/// 能过关——内核 DEFAULT_PMR_VALUE=0xf0，我们给 0xa0）。
pub fn enable_ppi(rd: usize, intid: u32, prio: u8) {
    let sgi = rd + SGI_BASE as usize;
    let b = intid & 31;
    wr32(sgi, SGI_IGROUPR0, rd32(sgi, SGI_IGROUPR0) | 1 << b);
    wr32(sgi, SGI_ISENABLER0, 1 << b);
    let off = prio_off(intid);
    let sh = prio_shift(intid);
    let p = rd32(sgi, off);
    wr32(sgi, off, (p & !(0xFF << sh)) | (prio as u32) << sh);
}

// ---- CPU 接口（系统寄存器；编码核自 arch/arm64/include/asm/sysreg.h）----

/// MPIDR_EL1 裸读。
pub fn mpidr() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) v, options(nomem, nostack)) };
    v
}

/// ICC_SRE_EL1 置 SRE（EL1 系统寄存器面使能；回读 bit0 为准——内核
/// gic_enable_sre() 同式，失败即内存映射面，本线不支持）。返回回读 bit0。
pub fn icc_sre_enable() -> bool {
    unsafe {
        core::arch::asm!("msr icc_sre_el1, {:x}", "isb", in(reg) 7u64, options(nostack));
    }
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, icc_sre_el1", out(reg) v, options(nomem, nostack)) };
    v & 1 != 0
}

/// ICC_PMR_EL1 = 0xf0（内核 DEFAULT_PMR_VALUE——PPI 优先级 0xa0 在其下）。
pub fn icc_pmr_0xf0() {
    unsafe { core::arch::asm!("msr icc_pmr_el1, {:x}", "isb", in(reg) 0xf0u64, options(nostack)) };
}

/// ICC_IGRPEN1_EL1 开 Group1。
pub fn icc_igrpen1(on: bool) {
    let v = if on { 1u64 } else { 0 };
    unsafe { core::arch::asm!("msr icc_igrpen1_el1, {:x}", "isb", in(reg) v, options(nostack)) };
}

/// ICC_IAR1_EL1 读（中断确认；0x3FF=spurious）。
pub fn icc_iar1() -> u32 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, icc_iar1_el1", out(reg) v, options(nomem, nostack)) };
    v as u32
}

/// ICC_EOIR1_EL1 写（中断结案）。
pub fn icc_eoir1(id: u32) {
    unsafe { core::arch::asm!("msr icc_eoir1_el1, {:x}", "isb", in(reg) id as u64, options(nostack)) };
}

/// DAIF 清 I 位——解中断掩。立即数是 DAIF 4 位域（F|I|A|D，bit0=F
/// bit1=I）的**掩码**（内核 arch_local_irq_enable 原文 `daifclr, #2`），
/// 首刷曾写 `#1`——只清了 F 位，I 恒掩、IRQ 永不进槽。
pub fn irq_unmask() {
    unsafe { core::arch::asm!("msr daifclr, #2", "isb") };
}

/// DAIF 置 I 位——掩中断（IRQ 入口首步：EOI 降优先级后杂响会重入，
/// 先断根再动状态）。立即数=域掩码，I=bit1 → `#2`。
pub fn irq_mask() {
    unsafe { core::arch::asm!("msr daifset, #2", "isb") };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内核 __gic_populate_rdist 匹配式：typer 高 32 == Aff 提炼（非裸掩）。
    #[test]
    fn mpidr_aff_extracts_fields_without_mt() {
        // Aff0=0x05, Aff1=0x06, Aff2=0x07, Aff3=0x08（bits 39:32）；
        // U(bit31)+MT(bit24) 置位——两位都不得混进提炼值
        let mpidr = 0x0000_0008_8107_0605;
        assert_eq!(mpidr_aff(mpidr), 0x0807_0605);
    }

    #[test]
    fn prio_layout_follows_intid() {
        assert_eq!(prio_off(19), SGI_IPRIORITYR + 16); // 19>>2=4 → 第 5 个寄存器
        assert_eq!(prio_shift(19), 24); // 19&3=3 → 字节 3
        assert_eq!(prio_off(31), SGI_IPRIORITYR + 28);
        assert_eq!(prio_shift(31), 24);
    }

    #[test]
    fn dist_ctlr_bits_match_kernel() {
        assert_eq!(DIST_CTLR_ARE_G1, 0x13); // ARE_NS(4)|G1A(1)|G1(0)
        assert_eq!(CTLR_RWP, 1 << 31);
    }
}
