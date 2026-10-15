//! 通用计时器读数（CNTVCT/CNTFRQ，EL1 可读未陷——u3g 起实测）。
//! S0-5 才引入中断；此处只有裸读与忙等。

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
