//! S0-2 — UFS 日志环（NOLINUX-PLAN S0 第二刀）。
//!
//! S0-1 自检链（SSU 唤醒 → GPT 双 CRC）原样保留为前置；通过后：
//! 在校验后 GPT 里按名查 boot_a（u5a §⑤ 契约），scratch 区
//! （start+16384 起 256 块 = 1MB）铺环形日志——boot 记录 + 12 条心跳
//! （每 10s，写盘限速合规）+ 终记录，WRITE(10) FUA 落盘、每条回读验。
//! 写完 PS_HOLD 立即复位（成功路径时延仪器退役；早期失败仍走码表）。
//!
//! 验收：多轮复位后环内序号跨启动连续增长（L0 侧 ring-parse.sh 解析
//! 出文本）；一轮 ≈ boot→14 条记录→自复位，tries 3→0 自动回滚 L0。
//!
//! 仪器码表（真源 crates/bare/src/instrument.rs 头注）沿用 + S0-2 增量：
//! 272 找不到 boot_a / 92 界检 / 472 环扫描读 / 362 写超时 / 422 写保护
//! (DATA PROTECT) / 122 写 CHECK / 392 写后回读不中。

#![no_std]
#![no_main]

use bare::ufs::{Fail, Ufs};
use bare::{gpt, instrument, logring, time};

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

/// 裸件线约定（非机器事实）：boot_a 内 64MB 处 scratch 区（u5a 实证位）。
const RING_OFF: u32 = 16384;
const RING_BLOCKS: u32 = 256;
const HEARTBEATS: u32 = 12;
const HEARTBEAT_SECS: u32 = 10;

/// 读失败 → 仪器码（分命令超时；OCS/byte0/READ CHECK 恒码）。
fn read_code(r: Fail, timeout_code: u32) -> u32 {
    match r {
        Fail::Timeout => timeout_code,
        Fail::Ocs(_) => 212,
        Fail::BadResp => 242,
        Fail::Check(_) => 122,
    }
}

/// 写失败 → 仪器码（u5a 码表：362 超时 / 422 写保护 / 122 其它 CHECK）。
fn write_code(r: Fail) -> u32 {
    match r {
        Fail::Timeout => 362,
        Fail::Ocs(_) => 212,
        Fail::BadResp => 242,
        Fail::Check(7) => 422,
        Fail::Check(_) => 122,
    }
}

fn push_str(buf: &mut [u8], n: &mut usize, s: &str) {
    for &b in s.as_bytes() {
        buf[*n] = b;
        *n += 1;
    }
}

fn push_u64(buf: &mut [u8], n: &mut usize, mut v: u64) {
    let mut tmp = [0u8; 20];
    let mut k = 0;
    loop {
        tmp[k] = b'0' + (v % 10) as u8;
        k += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while k > 0 {
        k -= 1;
        buf[*n] = tmp[k];
        *n += 1;
    }
}

/// 组文本（#seq 前缀——L0 解析直读十进制序号，od 按 BE 进位读不了 LE u64）。
fn text<'a>(buf: &'a mut [u8; 96], parts: &[&str], nums: &[u64]) -> &'a str {
    let mut n = 0;
    push_str(buf, &mut n, "#");
    push_u64(buf, &mut n, nums[0]);
    for (s, v) in parts.iter().zip(nums.iter().skip(1)) {
        push_str(buf, &mut n, s);
        push_u64(buf, &mut n, *v);
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// 写一条环记录（组包 → WRITE(10) FUA → 回读验 seq）。
fn ring_put(ufs: &mut Ufs, ring_base: u32, slot: u32, seq: u64, level: u32, txt: &str, freq: u32) {
    let buf = unsafe { core::slice::from_raw_parts_mut(STAGING as *mut u8, 4096) };
    logring::write_record(buf, seq, time::cntvct(), level, txt);
    let lba = ring_base + slot;
    if let Err(e) = ufs.write10(lba, 1, STAGING) {
        instrument::report_reset(PS_HOLD, write_code(e), freq);
    }
    match ufs.read10(lba, 1, DATA) {
        Err(e) => instrument::report_reset(PS_HOLD, read_code(e, 472), freq),
        Ok(()) => {
            let blk = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
            match logring::parse(blk) {
                Some((s, _, _)) if s == seq => {}
                _ => instrument::report_reset(PS_HOLD, 392, freq),
            }
        }
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

    // ② GPT 头 + ④ 条目数组 → 校验链（S0-1 原样；失败码不变）。
    //    check() 保证 array_bytes ≤ 4096 → 数组恒 1 块。
    let entries = match ufs.read10(1, 1, DATA) {
        Err(e) => instrument::report_reset(PS_HOLD, read_code(e, 302), freq),
        Ok(()) => {
            let hdr = unsafe { core::slice::from_raw_parts_mut(DATA as *mut u8, 4096) };
            match gpt::check(hdr) {
                Err(_) => instrument::report_reset(PS_HOLD, 92, freq),
                Ok(h) => {
                    if !gpt::header_crc_ok(hdr, &h) {
                        instrument::report_reset(PS_HOLD, 32, freq);
                    }
                    match ufs.read10(h.entry_lba, 1, ARRAY) {
                        Err(e) => instrument::report_reset(PS_HOLD, read_code(e, 332), freq),
                        Ok(()) => {
                            let array =
                                unsafe { core::slice::from_raw_parts(ARRAY as *const u8, 4096) };
                            if !gpt::array_crc_ok(hdr, array, &h) {
                                instrument::report_reset(PS_HOLD, 62, freq);
                            }
                            h.array_bytes >> 7
                        }
                    }
                }
            }
        }
    };

    // ⑤ 名查 boot_a（u5a §⑤：找不到 272；越 32 位/越界 92）
    let array = unsafe { core::slice::from_raw_parts(ARRAY as *const u8, 4096) };
    let part = match gpt::find(array, entries, "boot_a") {
        None => instrument::report_reset(PS_HOLD, 272, freq),
        Some(p) => p,
    };
    if (part.start | part.end) >> 32 != 0 {
        instrument::report_reset(PS_HOLD, 92, freq);
    }
    let ring_base = part.start as u32 + RING_OFF;
    if ring_base < part.start as u32 || ring_base > part.end as u32 || part.end as u32 - ring_base + 1 < RING_BLOCKS {
        instrument::report_reset(PS_HOLD, 92, freq);
    }

    // ⑥ 扫环找 max seq（逐块——单块已证路径；u5a 图样 AGXWRT01 非法魔数不扰）
    let mut max_seq = 0u64;
    let mut max_slot = RING_BLOCKS - 1;
    for i in 0..RING_BLOCKS {
        match ufs.read10(ring_base + i, 1, DATA) {
            Err(e) => instrument::report_reset(PS_HOLD, read_code(e, 472), freq),
            Ok(()) => {
                let blk = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
                if let Some((s, _, _)) = logring::parse(blk) {
                    if s >= max_seq {
                        max_seq = s;
                        max_slot = i;
                    }
                }
            }
        }
    }

    // ⑦ boot 记录 + 心跳 + 终记录（seq 接续 = max+1，slot = max_slot+1）
    let boot0 = time::cntvct();
    let mut seq = max_seq + 1;
    let mut slot = (max_slot + 1) % RING_BLOCKS;
    let mut t = [0u8; 96];
    let txt = text(&mut t, &[" boot resume=", " freq="], &[seq, max_seq, freq as u64]);
    ring_put(&mut ufs, ring_base, slot, seq, logring::LVL_BOOT, txt, freq);
    for i in 1..=HEARTBEATS {
        time::spin_secs(HEARTBEAT_SECS, freq);
        seq += 1;
        slot = (slot + 1) % RING_BLOCKS;
        let elapsed = (time::cntvct() - boot0) / freq as u64;
        let txt = text(&mut t, &[" heartbeat ", " elapsed="], &[seq, i as u64, elapsed]);
        ring_put(&mut ufs, ring_base, slot, seq, logring::LVL_INFO, txt, freq);
    }
    seq += 1;
    slot = (slot + 1) % RING_BLOCKS;
    let txt = text(&mut t, &[" cycle done reset"], &[seq]);
    ring_put(&mut ufs, ring_base, slot, seq, logring::LVL_INFO, txt, freq);

    // ⑧ 写尽落盘（FUA）→ 立即复位（成功路径仪器退役），三轮烧 tries 回滚 L0
    unsafe { (PS_HOLD as *mut u32).write_volatile(0) };
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // 999 = Rust panic 面（u4j 无此码——出现即新病，非 u4j 等价行为）
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    instrument::report_reset(PS_HOLD, 999, freq)
}
