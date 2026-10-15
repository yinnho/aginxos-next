//! 异常现场读取与 VBAR 装订（S0-3，NOLINUX-PLAN）。
//!
//! 向量表本体（16 槽×128B、2KB 对齐）由设备侧 global_asm 装订，
//! 本模块只提供 VBAR 写入与现场寄存器裸读（EL1 名，EL1 下可读）。
//! CurrentEL 编码 = EL<<2（EL1 → 0b0100）。

use core::arch::asm;

/// 装订向量表基址（须 2KB 对齐），isb 保生效序。
pub fn set_vbar(table: usize) {
    unsafe { asm!("msr vbar_el1, {}", "isb", in(reg) table, options(nostack)) };
}

/// 当前异常级别（0/1/2/3）。
pub fn current_el() -> u8 {
    let v: u64;
    unsafe { asm!("mrs {}, CurrentEL", out(reg) v, options(nomem, nostack)) };
    ((v >> 2) & 3) as u8
}

pub fn esr_el1() -> u64 {
    let v: u64;
    unsafe { asm!("isb", "mrs {}, esr_el1", out(reg) v, options(nomem, nostack)) };
    v
}

pub fn elr_el1() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {}, elr_el1", out(reg) v, options(nomem, nostack)) };
    v
}

pub fn far_el1() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {}, far_el1", out(reg) v, options(nomem, nostack)) };
    v
}
