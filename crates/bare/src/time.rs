//! 通用计时器读数（CNTVCT/CNTFRQ，EL1 可读未陷——u3g 起实测）。
//! S0-5：虚拟定时器（CNTV_*——内核同源默认面）武装/读数。
//! 中断线本体在 gic 模块（INTID 是机器事实，设备侧传入）。

use core::arch::asm;

#[inline]
pub fn cntvct() -> u64 {
    let v: u64;
    unsafe { asm!("isb", "mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack)) };
    v
}

#[inline]
pub fn cntfrq() -> u32 {
    let v: u64;
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) v, options(nomem, nostack)) };
    v as u32
}

/// 秒数兜底读法：CNTFRQ 为 0（理论不该）时用 fallback Hz。
pub fn freq_or(fallback_hz: u32) -> u32 {
    let f = cntfrq();
    if f == 0 { fallback_hz } else { f }
}

/// 忙等 secs 秒（CNTVCT 对拍，u4j 同款）。
pub fn spin_secs(secs: u32, freq_hz: u32) {
    let deadline = cntvct() + secs as u64 * freq_hz as u64;
    while cntvct() < deadline {
        core::hint::spin_loop();
    }
}

// ---- 虚拟定时器（CNTV_CTL/CNTV_CVAL；EL1 下即虚拟面，无需 HCR 改写）----

/// CNTV_CTL 裸读（bit0=ENABLE，bit1=IMASK，bit2=ISTATUS）。
#[inline]
pub fn vtimer_ctl() -> u32 {
    let v: u64;
    unsafe { asm!("isb", "mrs {}, cntv_ctl_el0", out(reg) v, options(nomem, nostack)) };
    v as u32
}

/// 撤防（ENABLE|IMASK 清零——防再响；先撤后改 CVAL 是惯例序）。
pub fn vtimer_disarm() {
    unsafe { asm!("msr cntv_ctl_el0, {:x}", "isb", in(reg) 0u64, options(nostack)) };
}

/// 绝对时刻武装（cval 处触发；返回武装瞬的 CNTVCT 供 dt 对拍）。
pub fn vtimer_arm(cval: u64) -> u64 {
    vtimer_disarm();
    unsafe { asm!("msr cntv_cval_el0, {:x}", in(reg) cval, options(nostack)) };
    unsafe { asm!("msr cntv_ctl_el0, {:x}", "isb", in(reg) 1u64, options(nostack)) };
    cntvct()
}
