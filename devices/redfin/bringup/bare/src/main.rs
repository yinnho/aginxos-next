//! S1-7 — 常驻 soak 刀：S1 验收「常驻≥1 天电量不降」的真考体。
//!
//! S1 四刀已收（喂狗/充电/电量计/温控，见 TRUTH §12）；本刀不再三轮
//! 自毁，而是**无限常驻**：ka.S 同法喂狗（MISC 0x1643←1，每 30s 一喂，
//! bark 窗 64s 的一半），每 20 喂（10 分钟）采一包两字落环——
//! f = QG V(0xC0)<<48|I(0xC2)<<32|MSOC(0xBF)<<24|CHGR 0x1006<<16|
//! 0x1042<<8|0x17 标记；t = VADC die(0x06)<<48|chg(0x09)<<32|
//! bat_therm(0x4a)<<16|vbat(0x84)。环 256 块 → 10min/包 ≈ 1.7 天滚动
//! 历史。开机段沿用 S1-6 面（SPMI init+VADC 八通道+w4 活性窗+基线
//! 还原），随后进环不复位。
//!
//! 生存性：裸件永不标记 boot successful——常驻期间 tries 不烧（无复
//! 位即无烧命）；病发（PMIC 硬复位）则烧一命，3 发后自动回 b 槽
//! L0（自愈），环里留着死前最后样本=病发取证副产。验收线=次日电
//! 量不降（MSOC 恒 100/V 不落）+ 温度平稳；0x1006=TAPER/FULLON 证
//! 词需半电充电日（本刀满电=TERMINATE 恒态也是有效材料）。
//!
//! S2 正题同刀（s18 段，⑩-2）：裸件 ext4 只读——GPT 名查 userdata
//! → UfsDev（read10 一块弹 DATA NC 窗按需切片）→ ext4::Fs::open →
//! 三靶（/etc/hostname、/etc/inittab、/bin/busybox）resolve+64KB 分
//! 段读+增量 crc32 落环；同晚 L0 侧 ssh md5/crc 对账=验收「读出与
//! L0 一致」。借用律：Fs 持 &mut Ufs 期间不能落环——先算后记。
//! ext4 解析器与增量 crc32 全在 crates/bare（19+1 测绿，真镜像
//! md5 对账含多 extent 大件）。
//!
//! 副核入口 = secondary_entry（裸 asm）：掩 DAIF、装 VBAR（向量表
//! per-core！副核不复位态默认）、立独立栈，跳 Rust secondary_main
//! ——纯忙等心跳写 NC mailbox（32B 槽×7 @0xB0010000：magic/mpidr/
//! seq/cnt），**不碰 UFS/GIC**（单控制器/未初始化面）。
//!
//! 验收（规格原文）：采样包连续 ≥1 天，f 字 MSOC 不降、V 漂 <50mV，
//! t 字 die 温不越 60°C，环零 634/636 事务败证。

#![no_std]
#![no_main]
// 环游标/页表/武装时刻是 static mut（exc_entry/irq_entry 与主流程共用）
// ——单核裸件、DAIF 全掩起步，无竞争；引用式访问只为省 addr_of 样板。
#![allow(static_mut_refs)]

use bare::ufs::{Fail, Ufs};
use bare::crc32;
use bare::ext4::{self, BlockDev};
use bare::{exc, gic, gpt, instrument, logring, mmu, spmi, time};

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

/// S2 ext4 分段读缓冲（64KB——busybox 级全文件滚 crc）。
static mut E2BUF: [u8; 65536] = [0; 65536];

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

/// S2：ext4::BlockDev 的 UFS 腿——byte_addr（userdata 分区内字节偏移）
/// → LBA（4K）read10 一块弹 DATA（NC DMA 窗），按需切片拷出。跨块/
/// 跨块边界由外层 while 分段；DATA 弹跳保证 DMA 只落 NC 窗。
struct UfsDev<'a> {
    ufs: &'a mut Ufs,
    /// userdata 分区起始 LBA（GPT 名查，32 位已验）
    base: u32,
}

impl BlockDev for UfsDev<'_> {
    fn read(&mut self, byte_addr: u64, out: &mut [u8]) -> Result<(), ()> {
        let mut done = 0usize;
        while done < out.len() {
            let a = byte_addr + done as u64;
            let lba = self.base + (a >> 12) as u32;
            let bo = (a & 0xFFF) as usize;
            let n = (4096 - bo).min(out.len() - done);
            self.ufs.read10(lba, 1, DATA).map_err(|_| ())?;
            let blk = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
            out[done..done + n].copy_from_slice(&blk[bo..bo + n]);
            done += n;
        }
        Ok(())
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

    // ⑩ SPMI 面（S1-6 温控读刀）：PM7250B VADC（PPID 0x231，外设
    //     0x3100，qcom,spmi-adc5）六通道一发读——die_temp 0x02/vph_pwr
    //     0x83/vbat_sns 0x84/chg_temp 0x09/bat_therm 0x4a（ratiometric）/
    //     smb1390_therm 0x0e。协议（adc_configure 同序）：DIG_PARAM 0x42
    //     （absolute=0x28 TIMER_CAL+ABS+dec2/ratiometric=0x18）→FAST_AVG
    //     0x43=0→CH_SEL 0x44=ch→DELAY 0x45（bat/smb=2 即 200µs，余 0）→
    //     EN_CTL1 0x46|=0x80→CONV_REQ 0x47=0x80→轮询 STATUS1 0x08（bit1
    //     REQ_STS 清+bit0 EOC 置）→DATA0/1 0x50/0x51=raw 15bit。
    //     换算（data_pmic5 full_scale=0x70e4）：VµV=raw×1875000/28900×den
    //     （vph/vbat 预分压 ÷1/3 即 den=3）；die m°C=raw×1875000/28900/2
    //     −273150；chg m°C=377500−VµV/4；smb m°C=419400−VµV×100/356；
    //     bat_therm V→R=100k·V/(1.875V−V) 查 NTC。终态还原基线六寄存器
    //     并验回读；4×2s 活性窗连发 die_temp 看温漂。vbat_sns 与 QG
    //     0xC0 机内对账。
    //    证据码：632=非 v5、635=VADC 无副本、634=读事务失败、636=写事务失败。
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
    // VADC 主角（fatal）：o231=ppid<<16|apid<<8|owner。
    let vad = match arb.find_apid_owner(PPID_VADC as u32, 0, n_apid) {
        Some(a) => a,
        None => instrument::report_reset(PS_HOLD, 635, freq),
    };
    let v = ((PPID_VADC as u64) << 16) | ((vad as u64) << 8) | arb.owner(vad) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " o231=0x", v);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // 跨外设单字节读写助手：失败即 634/636 落环复位。
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
    // 单通道转换（adc_configure/adc_wait_eoc 同序）：配五寄存器→REQ→
    // EOC 轮询（deadline 300ms）→DATA0/1。返回 raw|status<<16|npolls<<24。
    let conv = |ch: u8, dp: u8, dl: u8| -> u64 {
        wr(vad, 0x42, dp);
        wr(vad, 0x43, 0x00);
        wr(vad, 0x44, ch);
        wr(vad, 0x45, dl);
        let en = rd(vad, 0x46) | 0x80;
        wr(vad, 0x46, en);
        wr(vad, 0x47, 0x80);
        let deadline = time::cntvct() + freq as u64 * 300 / 1000;
        let mut status = 0u8;
        let mut npolls = 0u64;
        while time::cntvct() < deadline {
            status = rd(vad, 0x08);
            npolls += 1;
            if status & 0x03 == 0x01 {
                break;
            }
        }
        let raw = rd(vad, 0x50) as u64 | (rd(vad, 0x51) as u64) << 8;
        raw | (status as u64) << 16 | npolls.min(255) << 24
    };
    // 基线：STATUS1/2 + 0x42-0x47 六连（还原底本）。
    let bs = rd(vad, 0x08) as u64 | (rd(vad, 0x09) as u64) << 8;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " bs=0x", bs);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let base = [
        rd(vad, 0x42),
        rd(vad, 0x43),
        rd(vad, 0x44),
        rd(vad, 0x45),
        rd(vad, 0x46),
        rd(vad, 0x47),
    ];
    let bb = base.iter().enumerate().fold(0u64, |a, (i, &b)| a | (b as u64) << (8 * i));
    let txt = hex_text(&mut t, unsafe { RING.seq }, " bb=0x", bb);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // 八通道一发读：(ch, DIG_PARAM, DELAY)。rXX=raw|status<<16|npolls<<24。
    // 通道号真源=qcom,spmi-vadc.h ADC_* 枚举（dtsi 节名 @2 是旧 VADC
    // 编号残影——s16 首发射 die 用 0x02 读成浮脚，s16b 定谳 0x06）；
    // r00/r01=ref_gnd/vref_1p25 锚点（应 0V/1.25V，免费刻度校准）。
    let chans: [(u8, u8, u8); 8] = [
        (0x00, 0x28, 0x00),
        (0x01, 0x28, 0x00),
        (0x06, 0x28, 0x00),
        (0x83, 0x28, 0x00),
        (0x84, 0x28, 0x00),
        (0x09, 0x28, 0x00),
        (0x4a, 0x18, 0x02),
        (0x0e, 0x28, 0x02),
    ];
    for &(ch, dp, dl) in &chans {
        let v = conv(ch, dp, dl);
        let label = match ch {
            0x00 => " r00=0x",
            0x01 => " r01=0x",
            0x06 => " r06=0x",
            0x83 => " r83=0x",
            0x84 => " r84=0x",
            0x09 => " r09=0x",
            0x4a => " r4a=0x",
            _ => " r0e=0x",
        };
        let txt = hex_text(&mut t, unsafe { RING.seq }, label, v);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    }
    // 活性窗 4×2s：die_temp 连发（w4=raw|status<<16）——裸世温漂活性。
    for s in 0..4u64 {
        let v = conv(0x06, 0x28, 0x00);
        let txt = hex_text(&mut t, unsafe { RING.seq }, " w4=0x", v & 0xffff_ffff);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
        if s < 3 {
            let end = time::cntvct() + freq as u64 * 2;
            while time::cntvct() < end {}
        }
    }
    // 还原基线六寄存器，验回读：rs=现 0x42|0x44<<8|0x46<<16|原值同序<<24。
    wr(vad, 0x42, base[0]);
    wr(vad, 0x43, base[1]);
    wr(vad, 0x44, base[2]);
    wr(vad, 0x45, base[3]);
    wr(vad, 0x46, base[4]);
    wr(vad, 0x47, base[5]);
    let cur = [rd(vad, 0x42), rd(vad, 0x44), rd(vad, 0x46)];
    let rs = (cur[0] as u64)
        | (cur[1] as u64) << 8
        | (cur[2] as u64) << 16
        | (base[0] as u64) << 24
        | (base[2] as u64) << 32
        | (base[4] as u64) << 40;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " rs=0x", rs);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);

    // ⑩-2 S2 正题：裸件 ext4 只读（s18 段）。错误码不复位（soak 完整
    //     性优先）：1=无 userdata 分区 2=LBA 越 32 位 3=Fs::open 败
    //     4=resolve 败 5=read 败 6=4MB 上限截断。先算后记（Fs 持
    //     &mut Ufs 借用期不落环）。
    let t_e2 = time::cntvct();
    let ud = gpt::find(array, entries, "userdata");
    // 哨兵全 1=无分区；正常=end<<32|start（各 32 位内）。
    let mut ud_word: u64 = 0xFFFF_FFFF_FFFF_FFFF;
    let mut open_err: u64 = 0;
    // 每靶四槽：[err, ino, nread, crc]。
    let mut slots: [[u64; 4]; 3] = [[0; 4]; 3];
    const TARGETS: [&str; 3] = ["/etc/hostname", "/etc/inittab", "/bin/busybox"];
    if let Some(p) = ud {
        ud_word = (p.end << 32) | p.start;
        if (p.start | p.end) >> 32 != 0 {
            open_err = 2;
        } else {
            let mut sc1 = [0u8; 4096];
            let mut sc2 = [0u8; 4096];
            let dev = UfsDev { ufs: &mut ufs, base: p.start as u32 };
            match ext4::Fs::open(dev, &mut sc1) {
                Err(_) => open_err = 3,
                Ok(mut fs) => {
                    for (i, path) in TARGETS.iter().enumerate() {
                        match fs.resolve(path, &mut sc2) {
                            Err(_) => slots[i][0] = 4,
                            Ok(ino) => {
                                let mut st = crc32::crc32_start();
                                let mut off = 0u64;
                                let mut nread = 0u64;
                                let mut err = 0u64;
                                loop {
                                    let buf = unsafe { &mut E2BUF };
                                    match fs.read_file(ino, off, buf, &mut sc2) {
                                        Err(_) => {
                                            err = 5;
                                            break;
                                        }
                                        Ok(0) => break,
                                        Ok(n) => {
                                            st = crc32::crc32_update(st, &buf[..n]);
                                            off += n as u64;
                                            nread += n as u64;
                                            if nread > 4 << 20 {
                                                err = 6;
                                                break;
                                            }
                                        }
                                    }
                                }
                                slots[i] = [err, ino as u64, nread, crc32::crc32_finish(st) as u64];
                            }
                        }
                    }
                }
            }
        }
    }
    let t_e2 = time::cntvct() - t_e2;
    // phase B：统一落环（借用已放）。
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s18 ud=0x", ud_word);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    if open_err != 0 {
        let txt = hex_text(&mut t, unsafe { RING.seq }, " s18 o=0x", open_err);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    } else {
        // 标签逐靶固定（fa/fb/fc——hex_text 吃静态标签）。
        let fl = [" s18 fa=0x", " s18 fb=0x", " s18 fc=0x"];
        let gl = [" s18 ga=0x", " s18 gb=0x", " s18 gc=0x"];
        let hl = [" s18 ha=0x", " s18 hb=0x", " s18 hc=0x"];
        for i in 0..3 {
            let txt = hex_text(&mut t, unsafe { RING.seq }, fl[i], slots[i][1] << 32 | slots[i][0] << 56);
            ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
            let txt = hex_text(&mut t, unsafe { RING.seq }, gl[i], slots[i][2]);
            ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
            let txt = hex_text(&mut t, unsafe { RING.seq }, hl[i], slots[i][3]);
            ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
        }
    }
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s18 t=0x", t_e2);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);

    // ⑪ 常驻 soak 环（S1-7）：三 pick（MISC 0x216 喂狗面/CHGR 0x210
    //     充电态/QG 0x248 电量计——QG/CHGR 证据码续 63x：638=QG 无副本、
    //     637=CHGR 无副本；MISC 非致命——无副本则不喂（64s 内 bark 自
    //     复位=自愈路径），o216 仍落环记账）。主环 20×(喂+30s 忙等) 一
    //     采包；#N f=0x.. t=0x.. 双字一记录（换算同 S1-5/S1-6：VµV=
    //     raw×194637/1000（QG 10A 档）、IµA=raw×305176/1000、die m°C=
    //     raw×64880/1000−273150、vbat µV=raw×194637×3/1000）。
    //     终态=永续 loop（kmain 尾即环），无复位无 PSCI 面。
    let misc = match arb.find_apid_owner(PPID_MISC as u32, 0, n_apid) {
        Some(a) => a,
        None => 0,
    };
    let v = ((PPID_MISC as u64) << 16) | ((misc as u64) << 8) | if misc != 0 {
        arb.owner(misc) as u64
    } else {
        0
    };
    let txt = hex_text(&mut t, unsafe { RING.seq }, " o216=0x", v);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let chg = match arb.find_apid_owner(PPID_CHGR as u32, 0, n_apid) {
        Some(a) => a,
        None => instrument::report_reset(PS_HOLD, 637, freq),
    };
    let v = ((PPID_CHGR as u64) << 16) | ((chg as u64) << 8) | arb.owner(chg) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " o210=0x", v);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let qg = match arb.find_apid_owner(PPID_QG as u32, 0, n_apid) {
        Some(a) => a,
        None => instrument::report_reset(PS_HOLD, 638, freq),
    };
    let v = ((PPID_QG as u64) << 16) | ((qg as u64) << 8) | arb.owner(qg) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " o248=0x", v);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // soak 主环：cyc=0 即采首包（开机基线后第一点），此后每 20 喂一包。
    // 采包耗 SPMI ~12 读+8 写（四 conv 全协议）≪30s，喂狗间隔不破。
    let mut cyc = 0u64;
    loop {
        if misc != 0 {
            // 喂狗（0x1643←1）：平地址高字节 0x16=MISC 外设号由 APID
            // 承载，cmd 收低字节 0x43（s14 同律，ka.S 同式）。
            wr(misc, 0x43, 1);
        }
        if cyc % 20 == 0 {
            let v_raw = (rd(qg, 0xC0) as u64) | (rd(qg, 0xC1) as u64) << 8;
            let i_raw = (rd(qg, 0xC2) as u64) | (rd(qg, 0xC3) as u64) << 8;
            let f = (v_raw << 48)
                | (i_raw << 32)
                | ((rd(qg, 0xBF) as u64) << 24)
                | ((rd(chg, 0x06) as u64) << 16)
                | ((rd(chg, 0x42) as u64) << 8)
                | 0x17;
            let tp = ((conv(0x06, 0x28, 0x00) & 0xffff) << 48)
                | ((conv(0x09, 0x28, 0x00) & 0xffff) << 32)
                | ((conv(0x4a, 0x18, 0x02) & 0xffff) << 16)
                | (conv(0x84, 0x28, 0x00) & 0xffff);
            let mut n = 0usize;
            push_str(&mut t, &mut n, "#");
            push_u64(&mut t, &mut n, unsafe { RING.seq });
            push_str(&mut t, &mut n, " f=0x");
            push_hex(&mut t, &mut n, f);
            push_str(&mut t, &mut n, " t=0x");
            push_hex(&mut t, &mut n, tp);
            let txt = core::str::from_utf8(&t[..n]).unwrap_or("");
            ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
        }
        cyc += 1;
        let end = time::cntvct() + freq as u64 * 30;
        while time::cntvct() < end {}
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // 999 = Rust panic 面（u4j 无此码——出现即新病，非 u4j 等价行为）
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    instrument::report_reset(PS_HOLD, 999, freq)
}
