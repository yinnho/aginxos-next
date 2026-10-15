//! S0-1 — Rust 入口复刻 u4j（NOLINUX-PLAN S0 首刀）。
//!
//! 验收口径：工具链/链接/打包全链出件，机上行为与 u4j 等价——
//! adb 回窗 84–109s = 码 2（GPT 双 CRC 过）。码表真源
//! crates/bare/src/instrument.rs 头注（= u4j §8）。
//!
//! 与 u4j 的映射：超时分命令码（SSU 182 / 读① 302 / 读② 332）、
//! OCS 212 与 byte0 242 是 issue 层后备码、CHECK 分阶段（SSU 152 /
//! READ 122）、UA 重试 SSU 呼叫点 1 次 + read 内部 ≤2 次（platform 库）。

#![no_std]
#![no_main]

use bare::ufs::{Fail, Ufs};
use bare::{gpt, instrument, time};

include!(concat!(env!("OUT_DIR"), "/facts.rs"));

extern "C" {
    static __bss_start: u64;
    static __bss_end: u64;
}

core::arch::global_asm!(
    ".section .text.boot",
    ".globl _start",
    "_start:",
    "    msr daifset, #0xf",
    "    ldr x0, =__bss_start",
    "    ldr x1, =__bss_end",
    "1:  cmp x0, x1",
    "    b.hs 2f",
    "    str xzr, [x0], #8",
    "    b 1b",
    "2:  ldr x2, ={stack_top}",
    "    mov sp, x2",
    "    b rust_main",
    stack_top = const STACK_TOP,
);

/// 读失败 → 仪器码（分命令超时；OCS/byte0/READ CHECK 恒码）。
fn read_code(r: Fail, timeout_code: u32) -> u32 {
    match r {
        Fail::Timeout => timeout_code,
        Fail::Ocs(_) => 212,
        Fail::BadResp => 242,
        Fail::Check(_) => 122,
    }
}

#[no_mangle]
extern "C" fn rust_main() -> ! {
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    let mut ufs = Ufs::new(HCI, UTRD, UCD, freq);
    ufs.init();

    // ① SSU ACTIVE → W-LUN 0xD0（bootloader 留 PowerDown 的解药；呼叫点
    //    UA 重试 1 次，u4j §20 同款）
    let mut ua = false;
    loop {
        match ufs.ssu_active() {
            Ok(()) => break,
            Err(Fail::Check(key)) if key == 6 && !ua => ua = true,
            Err(Fail::Timeout) => instrument::report_reset(PS_HOLD, 182, freq),
            Err(Fail::Ocs(_)) => instrument::report_reset(PS_HOLD, 212, freq),
            Err(Fail::BadResp) => instrument::report_reset(PS_HOLD, 242, freq),
            Err(Fail::Check(_)) => instrument::report_reset(PS_HOLD, 152, freq),
        }
    }

    // ② GPT 头：LBA1 1 块 → DATA；健全 + 头 CRC（u4j §30 同款）
    let code = match ufs.read10(1, 1, DATA) {
        Err(e) => read_code(e, 302),
        Ok(()) => {
            let hdr = unsafe { core::slice::from_raw_parts_mut(DATA as *mut u8, 4096) };
            match gpt::check(hdr) {
                Err(_) => 92,
                Ok(h) if !gpt::header_crc_ok(hdr, &h) => 32,
                Ok(h) => {
                    // ④ 条目数组：elba、ceil(len/4096) 块 → ARRAY（u4j §111 同款）
                    let blocks = ((h.array_bytes + 0xFFF) >> 12) as u16;
                    match ufs.read10(h.entry_lba, blocks, ARRAY) {
                        Err(e) => read_code(e, 332),
                        Ok(()) => {
                            let n = (blocks as usize) << 12;
                            let array = unsafe { core::slice::from_raw_parts(ARRAY as *const u8, n) };
                            if gpt::array_crc_ok(hdr, array, &h) { 2 } else { 62 }
                        }
                    }
                }
            }
        }
    };
    instrument::report_reset(PS_HOLD, code, freq)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // 999 = Rust panic 面（u4j 无此码——出现即新病，非 u4j 等价行为）
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    instrument::report_reset(PS_HOLD, 999, freq)
}
