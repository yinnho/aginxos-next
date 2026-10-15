//! S1-3 — ka v3 狗面刀：BARK_BITE 看门狗武装判定+武装/喂/停喂实验。
//!
//! S1-2b 撤「不充电所以狗未武装」解释后，ka 线 130s 无咬疑**狗从未被
//! 武装**（武装者=L0 qpnp-smb5 探针 WD_CFG(0x1651)←bit1|bit6；XBL/
//! fastboot 链无人武装）。本刀：基线读 WD_CFG/SNARL_BARK_BITE(0x1653)/
//! MISC RT_STS(0x1610 bit1=bark)/USBIN RT——判裸世界武装态 → 自行武装
//! RMW → 喂 0x1643←1 ×5@4s → **停喂观察 12×5s**：bark 位升起+环截断
//! =咬死判决（量出 bark→bite 时延）；幸存 60s 则解除武装（护回滚 L0）
//! 走 ⑪。全程数据说话：武装边沿（arm 即计时 vs 须 plugin 事件）也
//! 由幸存/咬死二值自证。
//!
//! 副核入口 = secondary_entry（裸 asm）：掩 DAIF、装 VBAR（向量表
//! per-core！副核不复位态默认）、立独立栈，跳 Rust secondary_main
//! ——纯忙等心跳写 NC mailbox（32B 槽×7 @0xB0010000：magic/mpidr/
//! seq/cnt），**不碰 UFS/GIC**（单控制器/未初始化面）。主核武装
//! 3 秒 WFI，IRQ 醒来收割 7 槽落环后复位。
//!
//! 一致性：主核 NC 映射 ↔ 副核 MMU-off 物理访问（Device 直达），
//! 两侧皆非缓存免维护；副核栈 4KB×7 @0xB0011000 同窗。副核 PSCI
//! 入口态=EL1、MMU off、x0=context_id（=槽号）。
//!
//! 验收（规格原文）：7 个副核各写心跳进日志——每槽 magic 对、
//! mpidr 对号、seq>0，三轮一致，零 EXC。

#![no_std]
#![no_main]
// 环游标/页表/武装时刻是 static mut（exc_entry/irq_entry 与主流程共用）
// ——单核裸件、DAIF 全掩起步，无竞争；引用式访问只为省 addr_of 样板。
#![allow(static_mut_refs)]

use bare::ufs::{Fail, Ufs};
use bare::{exc, gic, gpt, instrument, logring, mmu, psci, spmi, time};

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

// 异常向量表（16 槽×128B，2KB 对齐进 .vectors；桩把槽号装 x0 跳目标
// 入口——不返回）。槽号 = 象限×4 + kind（sync0/irq1/fiq2/serror3）；
// 槽 5 = current EL SPx 的 IRQ——S0-5 本刀的目标槽。
core::arch::global_asm!(
    ".section .vectors, \"ax\"",
    ".align 11",
    ".globl __vectors",
    "__vectors:",
    ".macro VEC idx, tgt",
    "    mov x0, \\idx",
    "    b \\tgt",
    "    .space 0x78",
    ".endm",
    "    VEC 0, exc_entry",
    "    VEC 1, exc_entry",
    "    VEC 2, exc_entry",
    "    VEC 3, exc_entry",
    "    VEC 4, exc_entry",
    "    VEC 5, irq_entry",
    "    VEC 6, exc_entry",
    "    VEC 7, exc_entry",
    "    VEC 8, exc_entry",
    "    VEC 9, exc_entry",
    "    VEC 10, exc_entry",
    "    VEC 11, exc_entry",
    "    VEC 12, exc_entry",
    "    VEC 13, exc_entry",
    "    VEC 14, exc_entry",
    "    VEC 15, exc_entry",
    ".previous",
);

extern "C" {
    static __vectors: u8;
}

/// mailbox 心跳魔数（"SEC1" LE）。
const SEC_MAGIC: u64 = 0x3143_4553;
/// 副核数（cpu@0 主核 + 7 副核，活 DT cpus 节点表定谳）。
const SEC_COUNT: usize = 7;

// 副核入口（PSCI CPU_ON 跳入；x0=context_id=槽号 1..7）：掩 DAIF →
// 装 VBAR（向量表是 per-core 寄存器，副核不复位态默认）→ 立独立栈
// （SEC_STACK + 槽号×4KB，向下生长）→ 跳 Rust。PSCI 入口态 MMU off
// ——字面量池按物理地址取（主核恒等映射下同值）。
core::arch::global_asm!(
    ".section .text.sec_entry",
    ".globl secondary_entry",
    "secondary_entry:",
    "    msr daifset, #0xf",
    "    mov x20, x0",
    "    ldr x0, =__vectors",
    "    msr vbar_el1, x0",
    "    isb",
    "    ldr x1, ={sec_stack}",
    "    lsl x2, x20, #12",
    "    add sp, x1, x2",
    "    mov x0, x20",
    "    b secondary_main",
    sec_stack = const SEC_STACK,
);

extern "C" {
    static secondary_entry: u8;
}

/// 副心搏正身：纯忙等写 NC mailbox 槽（**不碰 UFS/GIC**——单控制器/
/// 未初始化面）。槽 32B：[0]=magic [1]=mpidr [2]=seq [3]=cnt。
#[no_mangle]
extern "C" fn secondary_main(i: usize) -> ! {
    let slot = (SEC_MAILBOX + i * 32) as *mut u64;
    unsafe {
        slot.add(0).write_volatile(SEC_MAGIC);
        slot.add(1).write_volatile(gic::mpidr());
        let mut seq: u64 = 0;
        loop {
            seq = seq.wrapping_add(1);
            slot.add(2).write_volatile(seq);
            slot.add(3).write_volatile(time::cntvct());
            // 节流 ~52ms/拍（19.2MHz 百万拍）——NC 写直达 DRAM，
            // 7 核全速写徒耗总线。
            let t0 = time::cntvct();
            while time::cntvct() - t0 < 1_000_000 {
                core::hint::spin_loop();
            }
        }
    }
}

/// 裸件线约定（非机器事实）：boot_a 内 scratch（u5a 实证位）。
const RING_OFF: u32 = 16384;
const RING_BLOCKS: u32 = 256;

/// L1（1GB 块）+ L2（e2 拆 2MB）双表，4KB 对齐（TTBR 硬律）。
#[repr(align(4096))]
struct Table([u64; 512]);
static mut L1: Table = Table([0; 512]);
static mut L2: Table = Table([0; 512]);

/// L2 里 DMA 非缓存窗：项 384..391（=PA 0xB0000000–0xB0200000）。
const DMA_L2_FIRST: usize = 384;
const DMA_L2_COUNT: usize = 8;

/// 环游标（主流程推进；exc_entry/irq_entry 读——单核裸件无竞争）。
struct Ring {
    base: u32,
    seq: u64,
    slot: u32,
}
static mut RING: Ring = Ring { base: 0, seq: 0, slot: 0 };

/// 武装时刻（CNTVCT）——irq_entry 对拍 dt 用；0 = 尚未武装。
static mut ARMED_AT: u64 = 0;

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

fn push_hex(buf: &mut [u8], n: &mut usize, mut v: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut tmp = [0u8; 16];
    let mut k = 0;
    loop {
        tmp[k] = HEX[(v & 0xF) as usize];
        k += 1;
        v >>= 4;
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
/// part 后跟 nums 对应位（无则裸标签——S0-3 首刷 trigger 标签即栽在
/// zip 长度失配上：nums 只剩 seq 时循环体整段不跑）。
fn text<'a>(buf: &'a mut [u8; 96], parts: &[&str], nums: &[u64]) -> &'a str {
    let mut n = 0;
    push_str(buf, &mut n, "#");
    push_u64(buf, &mut n, nums[0]);
    for (i, s) in parts.iter().enumerate() {
        push_str(buf, &mut n, s);
        if let Some(v) = nums.get(i + 1) {
            push_u64(buf, &mut n, *v);
        }
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// 异常现场记录文本（十/十六进制混排，专用组装）。
fn exc_text<'a>(buf: &'a mut [u8; 96], seq: u64, slot: usize, esr: u64, elr: u64, far: u64, el: u8) -> &'a str {
    let mut n = 0;
    push_str(buf, &mut n, "#");
    push_u64(buf, &mut n, seq);
    push_str(buf, &mut n, " EXC s=");
    push_u64(buf, &mut n, slot as u64);
    push_str(buf, &mut n, " esr=0x");
    push_hex(buf, &mut n, esr);
    push_str(buf, &mut n, " elr=0x");
    push_hex(buf, &mut n, elr);
    push_str(buf, &mut n, " far=0x");
    push_hex(buf, &mut n, far);
    push_str(buf, &mut n, " el=");
    push_u64(buf, &mut n, el as u64);
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// 单值十六进制记录（GIC 步进遗言）：#seq label0x<hex>。
fn hex_text<'a>(buf: &'a mut [u8; 96], seq: u64, label: &str, v: u64) -> &'a str {
    let mut n = 0;
    push_str(buf, &mut n, "#");
    push_u64(buf, &mut n, seq);
    push_str(buf, &mut n, label);
    push_hex(buf, &mut n, v);
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// 写一条环记录（组包 → WRITE(10) FUA → 回读验 seq），游标自增。
fn ring_put(ufs: &mut Ufs, freq: u32, level: u32, txt: &str) {
    let ring = unsafe { &mut RING };
    let buf = unsafe { core::slice::from_raw_parts_mut(STAGING as *mut u8, 4096) };
    logring::write_record(buf, ring.seq, time::cntvct(), level, txt);
    let lba = ring.base + ring.slot;
    if let Err(e) = ufs.write10(lba, 1, STAGING) {
        instrument::report_reset(PS_HOLD, write_code(e), freq);
    }
    match ufs.read10(lba, 1, DATA) {
        Err(e) => instrument::report_reset(PS_HOLD, read_code(e, 472), freq),
        Ok(()) => {
            let blk = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
            match logring::parse(blk) {
                Some((s, _, _)) if s == ring.seq => {}
                _ => instrument::report_reset(PS_HOLD, 392, freq),
            }
        }
    }
    ring.seq += 1;
    ring.slot = (ring.slot + 1) % RING_BLOCKS;
}

/// 异常入口（向量桩跳入，不返回）：现场记进环 → 复位。
/// UFS 事务重铺（prep 幂等）；SSU 不重发——主流程已 ACTIVE，若 trap
/// 早于 SSU（设备 PowerDown）写会失败，走仪器码复位保 tries 轮回。
#[no_mangle]
extern "C" fn exc_entry(slot: usize) -> ! {
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    let (esr, elr, far, el) = (exc::esr_el1(), exc::elr_el1(), exc::far_el1(), exc::current_el());
    let mut ufs = Ufs::new(HCI, UTRD, UCD, freq);
    ufs.init();
    let seq = unsafe { RING.seq };
    let mut t = [0u8; 96];
    let txt = exc_text(&mut t, seq, slot, esr, elr, far, el);
    ring_put(&mut ufs, freq, logring::LVL_ERR, txt);
    // 遗言已落盘（FUA）→ 立即复位
    unsafe { (PS_HOLD as *mut u32).write_volatile(0) };
    loop {
        core::hint::spin_loop();
    }
}

/// IRQ 入口（槽 5 桩跳入，不返回）：先掩中断（EOI 降优先级后杂响会
/// 重入本入口——首步断根）→ IAR 认领 → EOI 结案 → 撤定时器 → 环记录
/// （id/dt/ctl）→ **收割副核 mailbox**（S0-6：7 槽各记 aff/seq，
/// seq>0 即该核跳过心跳过）→ 复位。
#[no_mangle]
extern "C" fn irq_entry(_slot: usize) -> ! {
    gic::irq_mask();
    let id = gic::icc_iar1();
    let now = time::cntvct();
    let ctl = time::vtimer_ctl();
    if id != 0x3FF {
        gic::icc_eoir1(id);
    }
    time::vtimer_disarm();
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    let mut ufs = Ufs::new(HCI, UTRD, UCD, freq);
    ufs.init();
    let dt = now.wrapping_sub(unsafe { ARMED_AT });
    let mut t = [0u8; 96];
    let txt = text(&mut t, &[" IRQ id=", " dt=", " ctl="], &[unsafe { RING.seq }, id as u64, dt, ctl as u64]);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // 收割副核心跳（S0-6）：槽未动（主核 CPU_ON 前清零）= aff/seq 全
    // 0=该核未起；活着=aff 对号（0x100×i）且 seq 持续增长。
    for i in 1..=SEC_COUNT {
        let slot = (SEC_MAILBOX + i * 32) as *const u64;
        let (aff, seq) = unsafe { (slot.add(1).read_volatile(), slot.add(2).read_volatile()) };
        let txt = text(&mut t, &[" sec aff=", " seq="], &[unsafe { RING.seq }, aff, seq]);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    }
    unsafe { (PS_HOLD as *mut u32).write_volatile(0) };
    loop {
        core::hint::spin_loop();
    }
}

/// GPT 头+条目数组双 CRC 校验链（S0-2 自检正身），返回 (耗时 cycles,
/// entries)。条目数组留 ARRAY 缓冲供名查。
fn gpt_verify(ufs: &mut Ufs, freq: u32) -> (u64, usize) {
    let t0 = time::cntvct();
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
    (time::cntvct() - t0, entries)
}

/// 建双表：L1 项 0/1/3=Device 1GB 块、项 2→L2（2MB 全 WB，DMA 窗 NC）。
fn fill_tables() {
    let l1 = unsafe { &mut L1 };
    let l2 = unsafe { &mut L2 };
    l1.0[0] = mmu::dev_block(0x0);
    l1.0[1] = mmu::dev_block(0x4000_0000);
    l1.0[2] = mmu::table(l2 as *mut Table as usize);
    l1.0[3] = mmu::dev_block(0xC000_0000);
    for i in 0..512 {
        // 注意括号：`0x8000_0000 + i << 21` 会解析成 `(0x8000_0000 + i) << 21`
        // （+ 比 << 紧）——反汇编审计曾逮住（描述符=0x0010_0000_0000_0405）。
        l2.0[i] = mmu::wb_block(0x8000_0000 + ((i as u64) << 21));
    }
    for i in DMA_L2_FIRST..DMA_L2_FIRST + DMA_L2_COUNT {
        l2.0[i] = mmu::nc_block(0x8000_0000 + ((i as u64) << 21));
    }
}

#[no_mangle]
extern "C" fn rust_main() -> ! {
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    let mut ufs = Ufs::new(HCI, UTRD, UCD, freq);
    ufs.init();

    // ⓪ 向量装订（任何后续异常/中断都有桩——先于一切外设动作）
    exc::set_vbar(unsafe { &__vectors as *const u8 as usize });

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

    // ② cache-off 基线：GPT 双 CRC 计时（对照读）
    let (t_pre, _) = gpt_verify(&mut ufs, freq);

    // ③ 建表 + 开闸（M|C|I）。表址 4KB 对齐由 repr 保证——运行时兜底
    //    断言（对齐错即 fault，EXC 遗言自报）
    fill_tables();
    let l1_addr = unsafe { &L1 as *const Table as usize };
    if l1_addr & 0xFFF != 0 {
        instrument::report_reset(PS_HOLD, 92, freq);
    }
    mmu::enable(l1_addr);

    // ④ 判据①：cache-on 世界 GPT 双 CRC 再验（不过→仪器码早退）
    let (t_post, entries) = gpt_verify(&mut ufs, freq);

    // ⑤ 名查 boot_a + 界检（u5a §⑤ 契约；array=cache-on 读）
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

    // ⑥ 扫环找 max seq（cache-on 计时=S0-4 对照读，本刀顺带留痕）
    let t0 = time::cntvct();
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
    let t_scan = time::cntvct() - t0;

    // ⑦ 游标落位 + 四连记录：boot / pre / mmu / scan
    unsafe {
        RING.base = ring_base;
        RING.seq = max_seq + 1;
        RING.slot = (max_slot + 1) % RING_BLOCKS;
    }
    let el = exc::current_el() as u64;
    let mut t = [0u8; 96];
    let txt = text(&mut t, &[" boot el=", " resume=", " freq="], &[unsafe { RING.seq }, el, max_seq, freq as u64]);
    ring_put(&mut ufs, freq, logring::LVL_BOOT, txt);
    let txt = text(&mut t, &[" pre t="], &[unsafe { RING.seq }, t_pre]);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let txt = text(&mut t, &[" mmu=1 crc=1 t="], &[unsafe { RING.seq }, t_post]);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let txt = text(&mut t, &[" scan t="], &[unsafe { RING.seq }, t_scan]);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);

    // ⑧ GIC 面——严格内核次序（gic_init_bases：GICD TYPER 读 → gic_dist_init
    //    → gic_cpu_init=populate+wake），每步落环。首刷反着写（先碰 RD），
    //    首条 GIC MMIO（RD0 TYPER 读）即总线停摆——遗言止于 scan、烧了
    //    一整轮才定位；次序+步进记录都是本刷的学费。
    let typer = gic::rd32(GICD, gic::DIST_TYPER);
    let txt = hex_text(&mut t, unsafe { RING.seq }, " gicd typer=0x", typer as u64);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let ctlr = gic::dist_init(GICD, 100_000);
    let txt = hex_text(&mut t, unsafe { RING.seq }, " dist ctlr=0x", ctlr as u64);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let mpidr = gic::mpidr();
    // ⑧b S0-6b 探针①：主核裸 MPIDR 落环（此前只喂 find_rd 从未落环
    //    ——三角的主核角一直缺席）。
    let txt = hex_text(&mut t, unsafe { RING.seq }, " mpidr=0x", mpidr);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // ⑧b 探针②：RD_TYPER 全表——8 帧按帧号序逐帧落环（find_rd 的
    //    逐帧读已证安全），记录在 mpidr 记录之后连排，帧号=顺序号。
    for i in 0..RD_COUNT {
        let rd = GICR + i * RD_STRIDE;
        let typer = gic::rd64(rd, gic::RD_TYPER);
        let txt = hex_text(&mut t, unsafe { RING.seq }, " rdt=0x", typer);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    }
    let rd = match gic::find_rd(GICR, RD_COUNT as u32, RD_STRIDE as u32, mpidr) {
        None => instrument::report_reset(PS_HOLD, 612, freq),
        Some(rd) => rd,
    };
    let rd_idx = ((rd - GICR) / RD_STRIDE) as u64;
    let txt = text(&mut t, &[" rd="], &[unsafe { RING.seq }, rd_idx]);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let waker = gic::wake_rd(rd, 100_000);
    let txt = hex_text(&mut t, unsafe { RING.seq }, " waker=0x", waker as u64);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);

    // ⑨ CPU 接口三连（SRE 失败=内存映射面，本线不支持，证据码 642）
    if !gic::icc_sre_enable() {
        instrument::report_reset(PS_HOLD, 642, freq);
    }
    gic::icc_pmr_0xf0();
    gic::icc_igrpen1(true);
    gic::enable_ppi(rd, TIMER_INTID as u32, 0xa0);

    // ⑩ SPMI 面（S1-3c 狗面刀·EXT-LONG 重制）：S1-3b 定谳——8 位 EXT
    //    空间（op13/op2）在 PM7250B 是死区（写 ACK 全弃/读疑似低页
    //    别名），内核 charger regmap flat 16 位地址>0xFF 全走 EXT-LONG
    //    （WRITEL=op0/READL=op1，periph 由 APID 承载）。运输层已换 LONG，
    //    梯子不变复跑：真基线 w0（WD_CFG 0x1651 | SBB 0x1653<<8 |
    //    MISC_RT 0x1610<<16 | USBIN_RT 0x1310<<24）→ 写驻双试验 c42
    //    （CHGR 0x1042：cur→0→回读→还原→终读）/m53（SBB：cur→cur^0x10
    //    →回读→还原，狗未武装改值惰性）→ 武装梯 aw2（0x1651←|0x42，
    //    +0ms/+1s/+5s 三次回读）→ 喂 0x1643←1 ×2@4s → 停喂观察 12×5s
    //    （w3=wd_cfg|misc_rt<<8|usbin_rt<<16，bark=0x1610 bit1 升起→
    //    bite 截环）→ 幸存则补喂+解除武装。
    //    证据码：632=非 v5、633=PPID 无副本、634=读事务失败、636=写事务失败。
    let arb = spmi::Arb {
        core: CORE,
        chnls: CHNLS,
        obsrvr: OBSRVR,
        cnfg: CNFG,
    };
    let ver = arb.version();
    let txt = hex_text(&mut t, unsafe { RING.seq }, " spmi v=0x", ver as u64);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let feat = arb.features();
    let txt = hex_text(&mut t, unsafe { RING.seq }, " feat=0x", feat as u64);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    if !spmi::is_v5(ver) {
        instrument::report_reset(PS_HOLD, 632, freq);
    }
    let n_apid = spmi::apid_count(feat);
    // 三 PPID 各找 owner==0 副本：oXXX=ppid<<16|apid<<8|owner。
    let picks: [(u32, &str); 3] = [
        (PPID_CHGR as u32, " o210=0x"),
        (PPID_USBIN as u32, " o213=0x"),
        (PPID_MISC as u32, " o216=0x"),
    ];
    let mut apids = [0u32; 3];
    for (i, (ppid, label)) in picks.iter().enumerate() {
        let a = match arb.find_apid_owner(*ppid, 0, n_apid) {
            Some(a) => a,
            None => instrument::report_reset(PS_HOLD, 633, freq),
        };
        apids[i] = a;
        let v = ((*ppid as u64) << 16) | ((a as u64) << 8) | arb.owner(a) as u64;
        let txt = hex_text(&mut t, unsafe { RING.seq }, label, v);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    }
    let (chgr, usbin, misc) = (apids[0], apids[1], apids[2]);
    // 跨外设单字节读助手：失败即 634 落环复位。
    let rd = |ap: u32, off: u8| -> u8 {
        let mut b = [0u8; 1];
        match arb.read(ap, off, &mut b) {
            Ok(()) => b[0],
            Err(_) => instrument::report_reset(PS_HOLD, 634, freq),
        }
    };
    let wr = |ap: u32, off: u8, val: u8| {
        if arb.write(ap, off, &[val]).is_err() {
            instrument::report_reset(PS_HOLD, 636, freq);
        }
    };
    // 真基线：wd_cfg|sbb_cfg<<8|misc_rt<<16|usbin_rt<<24（EXT 后才是真值）。
    let v0 = rd(misc, 0x51) as u64
        | (rd(misc, 0x53) as u64) << 8
        | (rd(misc, 0x10) as u64) << 16
        | (rd(usbin, 0x10) as u64) << 24;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " w0=0x", v0);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // 写驻试验一 c42：CHGR 0x1042 充电使能（内核同写的 RW 寄存器），
    // 满电 TERMINATE 态下瞬时关断无碍——cur→0→回读→还原→终读。
    let cur = rd(chgr, 0x42);
    wr(chgr, 0x42, 0);
    let back = rd(chgr, 0x42);
    wr(chgr, 0x42, cur);
    let fin = rd(chgr, 0x42);
    let v = (cur as u64) | (back as u64) << 8 | (fin as u64) << 16;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " c42=0x", v);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // 写驻试验二 m53：MISC 0x1653 SBB 配置（狗未武装，SNARL 字段改值
    // 惰性）——cur→cur^0x10→回读→还原→终读。
    let cur = rd(misc, 0x53);
    wr(misc, 0x53, cur ^ 0x10);
    let back = rd(misc, 0x53);
    wr(misc, 0x53, cur);
    let fin = rd(misc, 0x53);
    let v = (cur as u64) | (back as u64) << 8 | (fin as u64) << 16;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " m53=0x", v);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // 武装梯 aw2：WD_CFG←cur|0x42（bit6 bark IRQ 使能|bit1 PLUGIN 启动
    // 计时；PLUGIN 在位=计时即启），+0ms/+1s/+5s 三次回读——S1-3 的
    // aw=0x0 悬案在此重审。
    let cur = rd(misc, 0x51);
    wr(misc, 0x51, cur | 0x42);
    let a0 = rd(misc, 0x51);
    let end = time::cntvct() + freq as u64;
    while time::cntvct() < end {}
    let a1 = rd(misc, 0x51);
    let end = time::cntvct() + freq as u64 * 4;
    while time::cntvct() < end {}
    let a5 = rd(misc, 0x51);
    let v = (cur as u64)
        | (a0 as u64) << 8
        | (a1 as u64) << 16
        | (a5 as u64) << 24;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " aw2=0x", v);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // 喂狗 ×2@4s：PET 位写 1。
    for i in 1..=2u64 {
        wr(misc, 0x43, 1);
        let txt = hex_text(&mut t, unsafe { RING.seq }, " pf=0x", i);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
        if i < 2 {
            let end = time::cntvct() + freq as u64 * 4;
            while time::cntvct() < end {}
        }
    }
    // 停喂观察 12×5s：w3=wd_cfg|misc_rt<<8|usbin_rt<<16（bark=bit1 升起
    // →bite 截环即答案；全程平=狗没咬；12 样本盖 64s bark 档）。
    for s in 0..12u64 {
        let v3 = rd(misc, 0x51) as u64
            | (rd(misc, 0x10) as u64) << 8
            | (rd(usbin, 0x10) as u64) << 16;
        let txt = hex_text(&mut t, unsafe { RING.seq }, " w3=0x", v3);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
        if s < 11 {
            let end = time::cntvct() + freq as u64 * 5;
            while time::cntvct() < end {}
        }
    }
    // 幸存 60s：补喂一记再解除武装（bit1←0，护回滚后的 XBL/L0 链），
    // 读回落环。
    wr(misc, 0x43, 1);
    let cur = rd(misc, 0x51);
    wr(misc, 0x51, cur & !0x02);
    let back = rd(misc, 0x51);
    let v = (cur as u64) | (back as u64) << 8;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " dw=0x", v);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);

    // ⑪ PSCI 面（S0-6）：version 落环 → 清 mailbox → 7 副核 CPU_ON
    //    （rc 各落环；SMC 从 EL1 陷 EL3 由 TZ 受理——L0 内核拉副核
    //    同路径）→ 武装 3 秒 → 解掩 → WFI（IRQ 入口收割落环）。
    let pv = psci::version();
    let txt = hex_text(&mut t, unsafe { RING.seq }, " psci v=0x", pv as u64);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    for i in 1..=SEC_COUNT {
        let slot = (SEC_MAILBOX + i * 32) as *mut u64;
        for w in 0..4 {
            unsafe { slot.add(w).write_volatile(0) };
        }
    }
    let entry = core::ptr::addr_of!(secondary_entry) as u64;
    for i in 1..=SEC_COUNT {
        let target = 0x100 * i as u64;
        let rc = psci::cpu_on(target, entry, i as u64);
        let txt = text(&mut t, &[" on aff=", " rc="], &[unsafe { RING.seq }, target, rc as u64]);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    }
    let delay = freq as u64 * 3;
    let armed_at = time::vtimer_arm(time::cntvct() + delay);
    unsafe { ARMED_AT = armed_at; }
    let txt = text(&mut t, &[" arm id=", " dt="], &[unsafe { RING.seq }, TIMER_INTID as u64, delay]);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    gic::irq_unmask();
    unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };

    // WFI 却无 trap 返回（杂醒）——证据码 652 复位
    instrument::report_reset(PS_HOLD, 652, freq)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // 999 = Rust panic 面（u4j 无此码——出现即新病，非 u4j 等价行为）
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    instrument::report_reset(PS_HOLD, 999, freq)
}
