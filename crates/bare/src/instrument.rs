//! 时延仪器（S0-1 收官件；S0-2 日志环在位后整机退役）。
//!
//! 无屏裸机的唯一外显通道：结果码 × 秒数忙等，然后 PS_HOLD ← 0
//! 冷重启（u4j §8 同款）。adb 侧按回窗读数——2 → 84–109s 过。
//! 码表（真源 TRUTH.md §7 / u4j 头注）：2 过 / 32 头CRC / 62 数组CRC /
//! 92 头不健全 / 122 READ CHECK / 152 SSU 被拒 / 182·302·332 SSU·读①·读②
//! 超时 / 212 OCS / 242 byte0 / 999 panic（Rust 面，u4j 无此码）。

pub fn report_reset(ps_hold: usize, code_secs: u32, freq_hz: u32) -> ! {
    let deadline = crate::time::cntvct() + code_secs as u64 * freq_hz as u64;
    while crate::time::cntvct() < deadline {
        core::hint::spin_loop();
    }
    unsafe { (ps_hold as *mut u32).write_volatile(0) };
    loop {
        core::hint::spin_loop();
    }
}
