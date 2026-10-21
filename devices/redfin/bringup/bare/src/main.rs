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
//! S3 首刀同刀（s19 段，⑩-3）：USB HS-only 设备面上电（配方真源
//! S3-USB.md——本地内核树拆账）。读序铁律：GDSC/时钟未上前 dwc3 与
//! QUSB2 空间读=总线挂死（同步异常→环+复位）——as-found 只读 GCC
//! 常活区（GPLL0/GDSCR）。PHY 十六步之前先投三路 RPMh VRM（ldoa5
//! 880mV / ldoa12 1704mV / ldoa2 3050mV，pmic5 LDO HPM=7）：地址从
//! cmd-db @0x80860000 查，apps RSC drv-2 TCS0（0x18220d00）发
//! ACTIVE 票；类型不是 VREG 或魔法对不上就不写。随后上电正典
//! （GDSC→USB30 BCR→五时钟腿→QUSB2 BCR→十六步 PHY write-readback，
//! 读后置），末段 dwc3 上线（软复位→GCTL 设备向→HS-only QSCRATCH
//! 三步→事件缓冲@NC 窗 0xB0005000→DCTL 清 SFTDISCONNECT→2s 后读
//! DSTS/GEVTCOUNT/首事件字）。全程零错误复位：超时/失配只记账
//! （e 字位掩码），刀的诊断输出=读数本身。
//!
//! S3 第二刀同刀（s20 段，⑩-4）：EP0 + CDC-ECM 枚举。配方真源
//! S3-USB.md「EP0 → CDC-ECM/NCM」。s19 上线快照之后重新拉断、武装
//! SETUP，再进 soak；枚举在 30s 喂狗窗里轮询事件缓冲。数据面是 s21：
//! ARP/ICMP/TCP 交给 smoltcp。TCP 2323 先回一行 aginx，再读命令：
//! id → redfin，ping → pong，up → 开机秒数和日志序号，
//! fb → 进 fastboot，log → 最近四条日志，bat → 电量计原样字，
//! fr → 只读全局 SMEM 421（modem 失败字符串）。
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

/// S2 ext4 分段读缓冲（64KB——busybox 级全文件滚 crc）。
static mut E2BUF: [u8; 65536] = [0; 65536];
/// ext4 开盘/路径暂存。放 BSS，不进 rust_main 栈——见 ext4::MAP_A。
static mut SC1: [u8; 4096] = [0; 4096];
static mut SC2: [u8; 4096] = [0; 4096];
/// EP0 TRB。放身份映射的普通 DRAM，不放 0xB0006000。上一刷
/// STARTTRANSFER 的 CMDACT 在取那块 NC 窗时把 AXI 挂死。
#[repr(C, align(64))]
struct Ep0Trb {
    raw: [u8; 64],
}
static mut EP0_TRB: Ep0Trb = Ep0Trb { raw: [0; 64] };
/// SMMU L1：512×8B。只填第 2 项，1GB 块恒等映射 0x80000000–0xBFFFFFFF。
#[repr(C, align(4096))]
struct SmmuL1 {
    e: [u64; 512],
}
static mut SMMU_L1: SmmuL1 = SmmuL1 { e: [0; 512] };

fn ep0_trb_addr() -> u64 {
    core::ptr::addr_of!(EP0_TRB) as u64
}

fn dcache_civac(addr: u64, len: u64) {
    let mut a = addr & !63;
    let end = addr + len;
    while a < end {
        unsafe {
            core::arch::asm!("dc civac, {a}", a = in(reg) a, options(nostack));
        }
        a += 64;
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
}

/// 只作废、不回写。cmd-db 是 XBL 落在 DRAM 的表；civac 会把核上的
/// 旧行盖回去。
fn dcache_ivac(addr: u64, len: u64) {
    let mut a = addr & !63;
    let end = addr + len;
    while a < end {
        unsafe {
            core::arch::asm!("dc ivac, {a}", a = in(reg) a, options(nostack));
        }
        a += 64;
    }
    unsafe {
        core::arch::asm!("dsb sy", "isb", options(nostack));
    }
}

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

/// MMIO 32 位读写（s19 起）：首 1GB Device 直映射，PA=VA。
fn rd32(a: usize) -> u32 {
    unsafe { (a as *const u32).read_volatile() }
}

fn rd8(a: u64) -> u8 {
    unsafe { (a as *const u8).read_volatile() }
}

fn rd16a(a: u64) -> u16 {
    unsafe { (a as *const u16).read_volatile() }
}

fn wr32(a: usize, v: u32) {
    unsafe { (a as *mut u32).write_volatile(v) }
}

/// 位域改写（读-改-写）：mask 清位后 or val。
fn rmw32(a: usize, mask: u32, val: u32) {
    let v = rd32(a);
    wr32(a, (v & !mask) | val);
}

fn push_str(buf: &mut [u8], n: &mut usize, s: &str) {
    for &b in s.as_bytes() {
        if *n >= buf.len() {
            return;
        }
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
        if *n >= buf.len() {
            return;
        }
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
        if *n >= buf.len() {
            return;
        }
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

// —— ⑩-4 s20：EP0 + CDC-ECM 枚举（S3-USB.md；dwc3 gadget.c/ep0.c）——
//
// DEPSTARTCFG 只在物理 ep0 上发一次（gadget.c start_config：number!=0
// 直接返回）。SET_CONFIGURATION 不再重发，只 SETEPCONFIG + DALEPENA。
// 发任何 DEPCMD 前清 GUSB2PHYCFG SUSPHY(bit6)/ENBLSLPM(bit8)，完后复原。
// STARTTRANSFER：PAR0=TRB 高 32 位，PAR1=低 32 位。

const PH_SETUP: u8 = 0;
const PH_DATA: u8 = 1;
const PH_WAIT: u8 = 2;
const PH_STATUS: u8 = 3;

const TRB_SETUP: u32 = 2 << 4;
const TRB_STATUS2: u32 = 3 << 4;
const TRB_STATUS3: u32 = 4 << 4;
const TRB_DATA: u32 = 5 << 4;
const TRB_NORMAL: u32 = 1 << 4;
const TRB_HWO_LST: u32 = 1 | (1 << 1) | (1 << 10) | (1 << 11);

struct Ep0 {
    lpos: u32,
    phase: u8,
    three: bool,
    nrdy: bool,
    nrdy_ep: u32,
    devst: u8,
    /// 0 无，2 连接通知已交，完成后再发 SPEED_CHANGE。
    link: u8,
    nevt: u32,
    logs: u32,
    /// 80 条普通日志写满之后，数据面仍要落环的条数。
    keep: u32,
    /// 速率通知完成时 bulk IN 还没 enable，等 alt 1 再发无偿 ARP。
    garp: bool,
}

static mut EP0: Ep0 = Ep0 {
    lpos: 0,
    phase: PH_SETUP,
    three: false,
    nrdy: false,
    nrdy_ep: 0,
    devst: 0,
    link: 0,
    nevt: 0,
    logs: 0,
    keep: 0,
    garp: false,
};
static mut NOTE_BUF: [u8; 16] = [0; 16];
/// gadget.c dwc3_alloc_trb_pool：DWC3_TRB_NUM=256。Link TRB 在最后一项，
/// 不是紧挨着 TRB0。紧挨着放一条 HWO 常置的 Link，STARTTRANSFER 时
/// TRB0 的 HWO 还是 0，控制器会预取到 Link 并转回去。
const RING_NUM: usize = 256;
#[repr(C, align(64))]
struct TrbRing {
    raw: [u32; RING_NUM * 4],
}
static mut NOTE_RING: TrbRing = TrbRing { raw: [0; RING_NUM * 4] };
static mut OUT_RING: TrbRing = TrbRing { raw: [0; RING_NUM * 4] };
static mut IN_RING: TrbRing = TrbRing { raw: [0; RING_NUM * 4] };
static mut OUT_BUF: [u8; 1536] = [0; 1536];
static mut IN_BUF: [u8; 1536] = [0; 1536];
static mut IN_BUSY: bool = false;
/// 交给 smoltcp 的一帧。OUT 环马上要复用 OUT_BUF，所以先拷出来。
static mut RX_COPY: [u8; 1536] = [0; 1536];
static mut RX_LEN: usize = 0;
static mut TCP_RX: [u8; 2560] = [0; 2560];
static mut TCP_TX: [u8; 2560] = [0; 2560];
static mut CTRL_SLOTS: [smoltcp::iface::SocketStorage<'static>; 1] =
    [smoltcp::iface::SocketStorage::EMPTY];
static mut CTRL_IFACE: Option<smoltcp::iface::Interface> = None;
static mut CTRL_HANDLE: Option<smoltcp::iface::SocketHandle> = None;
static mut CTRL_UP: bool = false;
static mut CTRL_GREETED: bool = false;
/// 监听记录不走 s20_keep 的 24 条配额。最多 16 条。
static mut TCP_NOTES: u8 = 0;
static mut TCP_ST: u8 = 0xff;
/// 停在 SynReceived 的起点。超过 1 秒就拆掉再 listen，免得唯一的套接字把后来的 SYN 拒掉。
static mut TCP_SYN_AT: u64 = 0;
static mut TCP_RXF: u8 = 0xff;
static mut TCP_LS_BAD: bool = false;
static mut CTX_UFS: *mut Ufs = core::ptr::null_mut();
static mut CTX_FREQ: u32 = 0;
static mut CTX_T: *mut [u8; 96] = core::ptr::null_mut();
/// bulk/中断端点的传输已经 START 过。下标是物理端点号。
static mut XFER_ON: u32 = 0;
static mut RSC: [u32; 8] = [0; 8];
/// 下一条要写的 TRB 下标，和刚完成的那条。gadget.c 的 trb_enqueue /
/// trb_dequeue。Link 在第 255 项，这两个下标都不取 255。
static mut TRB_ENQ: [u8; 8] = [0; 8];
static mut TRB_DEQ: [u8; 8] = [0; 8];
/// 描述符里的 iMAC 被 macOS 当成 en10 自己的地址（02:00:00:00:00:01）。
/// 手机再用同一个源地址，ARP 应答会被当成自发帧丢掉。设备地址错开一号。
const OUR_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];
const OUR_IP: [u8; 4] = [169, 254, 1, 1];
const BULK_LEN: u32 = 1536;
/// 控制口。连上先看到 `aginx`。`id` 回 `redfin`，`ping` 回 `pong`，
/// `up` 回开机秒数。`fb` 进 fastboot。`log` 回最近四条环记录。
/// `bat` 回 soak 的 f 字：V<<48|I<<32|MSOC<<24|CHGR06<<16|CHGR42<<8|0x17。
/// `fr` 只读 SMEM 421，不分配、不踢 IPCC。
const PON_PPID: u32 = 0x008;
const IMEM_RESTART: usize = 0x146A_B65C;
const MSM_PS_HOLD: usize = 0x0C26_4000;
const CTRL_PORT: u16 = 2323;
static mut CTRL_LINE: [u8; 64] = [0; 64];
static mut CTRL_LINE_N: usize = 0;
static mut CTRL_OVERFLOW: bool = false;
/// 0 闲，1 应答已交、等 IN 完成，2 再等 20ms 后拉 PS_HOLD。
static mut CTRL_FB: u8 = 0;
static mut FB_APID: u32 = 0;
static mut FB_GEN2: bool = false;
static mut FB_AT: u64 = 0;
static mut LOG_OUT: [u8; 384] = [0; 384];
static mut SOAK_QG: u32 = 0;
static mut SOAK_CHG: u32 = 0;

const DEV_DESC: [u8; 18] = [
    0x12, 0x01, 0x00, 0x02, 0xef, 0x02, 0x01, 0x40, 0x6b, 0x1d, 0x04, 0x01, 0x00, 0x01, 0x01,
    0x02, 0x00, 0x01,
];
const QUAL_DESC: [u8; 10] = [0x0a, 0x06, 0x00, 0x02, 0xef, 0x02, 0x01, 0x40, 0x01, 0x00];
/// config 88B：头 9 + IAD 8 + 控制接口 9 + header 5 + union 5 + ECM 13
/// + notify 7 + alt0 9 + alt1 9 + bulk IN 7 + bulk OUT 7。
const CFG_DESC: [u8; 88] = [
    0x09, 0x02, 0x58, 0x00, 0x02, 0x01, 0x00, 0xc0, 0x32, //
    0x08, 0x0b, 0x00, 0x02, 0x02, 0x06, 0x00, 0x00, //
    0x09, 0x04, 0x00, 0x00, 0x01, 0x02, 0x06, 0x00, 0x00, //
    0x05, 0x24, 0x00, 0x10, 0x01, //
    0x05, 0x24, 0x06, 0x00, 0x01, //
    0x0d, 0x24, 0x0f, 0x03, 0x00, 0x00, 0x00, 0x00, 0xea, 0x05, 0x00, 0x00, 0x00, //
    0x07, 0x05, 0x81, 0x03, 0x10, 0x00, 0x09, //
    0x09, 0x04, 0x01, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, //
    0x09, 0x04, 0x01, 0x01, 0x02, 0x0a, 0x00, 0x00, 0x00, //
    0x07, 0x05, 0x82, 0x02, 0x00, 0x02, 0x00, //
    0x07, 0x05, 0x01, 0x02, 0x00, 0x02, 0x00,
];
const STR0: [u8; 4] = [0x04, 0x03, 0x09, 0x04];
const STR_MFR: [u8; 12] = [
    0x0c, 0x03, 0x41, 0x00, 0x67, 0x00, 0x69, 0x00, 0x6e, 0x00, 0x78, 0x00,
];
const STR_PRD: [u8; 20] = [
    0x14, 0x03, 0x41, 0x00, 0x67, 0x00, 0x69, 0x00, 0x6e, 0x00, 0x78, 0x00, 0x20, 0x00, 0x45,
    0x00, 0x43, 0x00, 0x4d, 0x00,
];
/// iMAC = "020000000001"（ECM 描述符字符串索引 3）。
const STR_MAC: [u8; 26] = [
    0x1a, 0x03, 0x30, 0x00, 0x32, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30,
    0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x31, 0x00,
];

fn s20_log(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], label: &str, v: u64) {
    s20_emit(ufs, freq, t, label, v, false);
}

/// 枚举会把普通的 80 条用完。ep≥2 的事件、TRB 下标、通知和 ARP/ICMP
/// 走这里，最多再记 24 条，免得主机轮询把环写满。
fn s20_keep(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], label: &str, v: u64) {
    s20_emit(ufs, freq, t, label, v, true);
}

fn s20_emit(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], label: &str, v: u64, keep: bool) {
    let st = unsafe { &mut EP0 };
    if keep {
        if st.keep >= 24 {
            return;
        }
        st.keep += 1;
    } else {
        if st.logs >= 80 {
            return;
        }
        st.logs += 1;
    }
    let txt = hex_text(t, unsafe { RING.seq }, label, v);
    ring_put(ufs, freq, logring::LVL_INFO, txt);
}

/// DEPCMD。返回完成态 bits15:12；超时返回 None（不复位）。
fn dep_cmd(ep: u32, opc: u32, p0: u32, p1: u32, p2: u32) -> Option<u32> {
    let phy = USB_DWC3 + 0xc200;
    let saved = rd32(phy);
    let masked = saved & !((1 << 6) | (1 << 8));
    if masked != saved {
        wr32(phy, masked);
    }
    let base = USB_DWC3 + 0xc800 + (ep as usize) * 0x10;
    wr32(base, p2);
    wr32(base + 4, p1);
    wr32(base + 8, p0);
    wr32(base + 0xc, opc | (1 << 10));
    let freq = time::freq_or(FREQ_FALLBACK as u32) as u64;
    let end = time::cntvct() + freq / 100;
    let mut st = None;
    while time::cntvct() < end {
        let reg = rd32(base + 0xc);
        if reg & (1 << 10) == 0 {
            st = Some((reg >> 12) & 0xf);
            break;
        }
    }
    if saved & ((1 << 6) | (1 << 8)) != 0 {
        let cur = rd32(phy);
        wr32(phy, cur | (saved & ((1 << 6) | (1 << 8))));
    }
    st
}

fn s20_fail(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], ep: u32, opc: u32, st: u32) {
    s20_log(
        ufs,
        freq,
        t,
        " s20 ce=0x",
        ((ep as u64) << 24) | ((opc as u64) << 16) | st as u64,
    );
}

fn arm_xfer(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], ep: u32, buf: u64, len: u32, ctl: u32) {
    let trb = ep0_trb_addr();
    let a = trb as usize;
    wr32(a, buf as u32);
    wr32(a + 4, (buf >> 32) as u32);
    wr32(a + 8, len & 0x00ff_ffff);
    wr32(a + 12, ctl | TRB_HWO_LST);
    dcache_civac(trb, 64);
    match dep_cmd(ep, 6, (trb >> 32) as u32, trb as u32, 0) {
        Some(0) => {}
        Some(st) => s20_fail(ufs, freq, t, ep, 6, st),
        None => s20_fail(ufs, freq, t, ep, 6, 0xff),
    }
}

/// 给 USB 流 0xE0 装一张 S1 页表。成功且 S2CR 读回是转换才返回 true，
/// 调用方这时才发 STARTTRANSFER。直通被硬件改成故障，不再写直通。
fn smmu_usb_bypass(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) -> bool {
    let base = SMMU_BASE;
    let idr0 = rd32(base + 0x20);
    let cr0 = rd32(base);
    s20_log(ufs, freq, t, " s20 id=0x", ((idr0 as u64) << 32) | cr0 as u64);
    // 上一刷停在 id= 之后、i1= 之前。先读 IDR1 再扫槽，把两处分开。
    let id1 = rd32(base + 0x24);
    s20_log(ufs, freq, t, " s20 i1=0x", id1 as u64);
    let nsmr = (idr0 & 0xff) as usize;
    if nsmr == 0 || nsmr > 128 {
        return false;
    }
    // 不再扫描。直通那张镜像已经扫完 81 槽，空槽在 4，S2CR 读回过 0。
    // 只碰这一个已经读通过的槽。
    let _ = nsmr;
    let sid = USB_SID as u32;
    let i = 4usize;
    let smr = rd32(base + 0x800 + i * 4);
    s20_log(ufs, freq, t, " s20 sm=0x", smr as u64);
    let smr_id = smr & 0x7fff;
    let smr_ok = smr & (1 << 31) == 0 || (smr_id == sid && (smr >> 16) & 0x7fff == 0);
    if !smr_ok {
        return false;
    }
    let exact = smr & (1 << 31) != 0;
    let pgshift = if id1 & (1 << 31) != 0 { 16 } else { 12 };
    let numcb = (id1 & 0xff) as usize;
    if numcb == 0 || numcb > 64 {
        return false;
    }
    // 只用最高的一个上下文组。先落环再读它的 SCTLR，读挂也能看出。
    let cb = numcb - 1;
    let cb_base = SMMU_BASE + 0x80000 + (cb << pgshift);
    s20_log(ufs, freq, t, " s20 ca=0x", cb_base as u64);
    let sctlr0 = rd32(cb_base);
    s20_log(ufs, freq, t, " s20 cs=0x", sctlr0 as u64);
    if sctlr0 & 1 != 0 {
        return false;
    }
    let tbl = core::ptr::addr_of_mut!(SMMU_L1) as usize;
    // 1GB 块，AF=1，SH=inner，AP=RW，AttrIndx=0，valid。
    let desc: u64 = 0x8000_0000 | (1 << 10) | (3 << 8) | (1 << 6) | 1;
    wr32(tbl + 16, desc as u32);
    wr32(tbl + 20, (desc >> 32) as u32);
    dcache_civac(tbl as u64, 4096);
    let gr1 = base + (1usize << pgshift);
    wr32(cb_base, 0);
    wr32(gr1 + 0x800 + cb * 4, 1); // CBA2R：AArch64
    wr32(gr1 + cb * 4, (1 << 16) | (0xf << 12) | (3 << 8)); // CBAR：S1，S2 直通
    wr32(cb_base + 0x30, 25 | (1 << 8) | (1 << 10) | (3 << 12)); // T0SZ=25，4K
    let ttbr = tbl as u64;
    wr32(cb_base + 0x20, ttbr as u32);
    wr32(cb_base + 0x24, (ttbr >> 32) as u32);
    wr32(cb_base + 0x38, 0x44); // MAIR0：attr0 = NC
    // M|TRE|AFE|ASIDPNE|NSH。不置 CFCFG，缺页终止而不是把 AXI 挂住。
    wr32(cb_base, 1 | 2 | 4 | (1 << 12) | (3 << 22));
    wr32(cb_base + 0x618, 0);
    wr32(cb_base + 0x7f0, 0);
    let s2_addr = base + 0xc00 + i * 4;
    let before = rd32(s2_addr);
    s20_log(ufs, freq, t, " s20 s2=0x", ((before as u64) << 32) | ((cb as u64) << 8) | i as u64);
    wr32(s2_addr, cb as u32); // TYPE=translation，低 8 位是 CB
    if !exact {
        wr32(base + 0x800 + i * 4, (1 << 31) | sid);
    }
    let sb = rd32(s2_addr);
    s20_log(ufs, freq, t, " s20 sb=0x", sb as u64);
    // TYPE 在 bit17:16。0 才是转换。读回不是转换就不发 STARTTRANSFER。
    let ok = (sb >> 16) & 3 == 0 && (sb & 0xff) == cb as u32;
    if ok {
        unsafe { USB_CB = cb as u32 };
    }
    ok
}

fn arm_setup(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    // ep0.c ep0_out_start：TRB 缓冲指针就是 TRB 自己，长度 8，
    // CONTROL_SETUP | HWO | ISP_IMI | IOC | LST。上一刷 ena=0x3 之后
    // 死在 STARTTRANSFER 的 CMDACT 写。先落 tr= 再写命令。
    unsafe { EP0.phase = PH_SETUP };
    let ctl = TRB_SETUP | TRB_HWO_LST;
    let trb = ep0_trb_addr();
    s20_log(ufs, freq, t, " s20 tr=0x", (trb << 32) | ctl as u64);
    arm_xfer(ufs, freq, t, 0, trb, 8, TRB_SETUP);
}

fn epbuf_put(data: &[u8]) -> u32 {
    let n = data.len().min(256);
    for i in 0..n {
        unsafe { ((USB_EPBUF + i) as *mut u8).write_volatile(data[i]) };
    }
    n as u32
}

fn enable_ep(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], ep: u32, p0: u32, p1: u32) {
    match dep_cmd(ep, 1, p0, p1, 0) {
        Some(0) => {}
        Some(st) => s20_fail(ufs, freq, t, ep, 1, st),
        None => s20_fail(ufs, freq, t, ep, 1, 0xff),
    }
    rmw32(USB_DWC3 + 0xc720, 1 << ep, 1 << ep);
}

fn xfer_res(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], ep: u32) {
    match dep_cmd(ep, 2, 1, 0, 0) {
        Some(0) => {}
        Some(st) => s20_fail(ufs, freq, t, ep, 2, st),
        None => s20_fail(ufs, freq, t, ep, 2, 0xff),
    }
}

fn depcfg_control(ep: u32, mps: u32) -> (u32, u32) {
    let mut p0 = mps << 3;
    if ep & 1 == 1 {
        p0 |= (ep >> 1) << 17;
    }
    let p1 = (1 << 8) | (1 << 10) | (ep << 25);
    (p0, p1)
}

fn depcfg_bulk(ep: u32, mps: u32) -> (u32, u32) {
    let mut p0 = (2 << 1) | (mps << 3);
    if ep & 1 == 1 {
        p0 |= (ep >> 1) << 17;
    }
    let p1 = (1 << 9) | (ep << 25);
    (p0, p1)
}

fn depcfg_intr_in(ep: u32, mps: u32, interval_m1: u32) -> (u32, u32) {
    let p0 = (3 << 1) | (mps << 3) | ((ep >> 1) << 17);
    let p1 = (1 << 9) | (ep << 25) | (interval_m1 << 16);
    (p0, p1)
}

fn s20_stall(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    match dep_cmd(0, 4, 0, 0, 0) {
        Some(0) => {}
        Some(st) => s20_fail(ufs, freq, t, 0, 4, st),
        None => s20_fail(ufs, freq, t, 0, 4, 0xff),
    }
    arm_setup(ufs, freq, t);
}

fn s20_bus_reset(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    // 总线复位清掉传输资源。只重发 STARTTRANSFER 会得到状态 1
    // （ce=0x60001）。databook 8.1.5：DEPSTARTCFG(0) 之后重新
    // Set Endpoint Config、Set Transfer Resource，再武装 SETUP。
    rmw32(USB_DWC3 + 0xc700, 0x7f << 3, 0);
    wr32(USB_DWC3 + 0xc720, 0);
    unsafe {
        EP0.devst = 0;
        EP0.three = false;
        EP0.nrdy = false;
        EP0.link = 0;
        EP0.garp = false;
        IN_BUSY = false;
        XFER_ON = 0;
        TRB_ENQ = [0; 8];
        TRB_DEQ = [0; 8];
    }
    match dep_cmd(0, 9, 0, 0, 0) {
        Some(0) => {}
        Some(st) => s20_fail(ufs, freq, t, 0, 9, st),
        None => s20_fail(ufs, freq, t, 0, 9, 0xff),
    }
    for ep in [0u32, 1, 2, 3, 5] {
        xfer_res(ufs, freq, t, ep);
    }
    let (p0, p1) = depcfg_control(0, 64);
    enable_ep(ufs, freq, t, 0, p0, p1);
    let (p0, p1) = depcfg_control(1, 64);
    enable_ep(ufs, freq, t, 1, p0, p1);
    arm_setup(ufs, freq, t);
}

fn s20_setup(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    let mut b = [0u8; 8];
    let trb = ep0_trb_addr();
    dcache_civac(trb, 64);
    for i in 0..8 {
        b[i] = unsafe { ((trb as usize + i) as *const u8).read_volatile() };
    }
    let bm = b[0];
    let req = b[1];
    let wval = u16::from_le_bytes([b[2], b[3]]);
    let widx = u16::from_le_bytes([b[4], b[5]]);
    let wlen = u16::from_le_bytes([b[6], b[7]]);
    let word = (bm as u64)
        | ((req as u64) << 8)
        | ((wval as u64) << 16)
        | ((widx as u64) << 32)
        | ((wlen as u64) << 48);
    s20_log(ufs, freq, t, " s20 su=0x", word);
    let three = wlen != 0;
    unsafe {
        EP0.three = three;
    }
    let typ = (bm >> 5) & 3;
    let ok = if typ != 0 {
        wlen == 0
    } else {
        s20_std(ufs, freq, t, bm, req, wval, widx, wlen)
    };
    if !ok {
        s20_stall(ufs, freq, t);
        return;
    }
    if !three {
        unsafe { EP0.phase = PH_WAIT };
        return;
    }
    if bm & 0x80 == 0 {
        // 控制 OUT（SET_SEL 一类）本刀不收，stall。
        s20_stall(ufs, freq, t);
        return;
    }
    unsafe { EP0.phase = PH_DATA };
}

fn s20_arm_status(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], ep: u32) {
    let ctl = if unsafe { EP0.three } { TRB_STATUS3 } else { TRB_STATUS2 };
    unsafe {
        EP0.phase = PH_STATUS;
        EP0.nrdy = false;
    }
    arm_xfer(ufs, freq, t, ep, USB_EPBUF as u64, 0, ctl);
}

fn s20_std(
    ufs: &mut Ufs,
    freq: u32,
    t: &mut [u8; 96],
    _bm: u8,
    req: u8,
    wval: u16,
    widx: u16,
    wlen: u16,
) -> bool {
    match req {
        0 => {
            // GET_STATUS
            if wlen < 2 {
                return false;
            }
            let n = epbuf_put(&[0, 0]);
            arm_xfer(ufs, freq, t, 1, USB_EPBUF as u64, n.min(wlen as u32), TRB_DATA);
            true
        }
        1 | 3 => wlen == 0, // CLEAR/SET_FEATURE：本刀无 halt 状态，空状态段即收
        5 => {
            // SET_ADDRESS
            if wlen != 0 || widx != 0 || wval > 127 {
                return false;
            }
            if unsafe { EP0.devst } == 2 {
                return false;
            }
            rmw32(USB_DWC3 + 0xc700, 0x7f << 3, (wval as u32) << 3);
            unsafe { EP0.devst = if wval == 0 { 0 } else { 1 } };
            true
        }
        6 => s20_desc(ufs, freq, t, wval, wlen),
        8 => {
            let cfg = if unsafe { EP0.devst } == 2 { 1u8 } else { 0 };
            let n = epbuf_put(&[cfg]);
            arm_xfer(ufs, freq, t, 1, USB_EPBUF as u64, n.min(wlen as u32), TRB_DATA);
            true
        }
        9 => s20_set_cfg(ufs, freq, t, wval, wlen),
        10 => {
            let n = epbuf_put(&[0]);
            arm_xfer(ufs, freq, t, 1, USB_EPBUF as u64, n.min(wlen as u32), TRB_DATA);
            true
        }
        11 => s20_set_intf(ufs, freq, t, wval, widx, wlen),
        _ => false,
    }
}

fn s20_desc(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], wval: u16, wlen: u16) -> bool {
    let kind = (wval >> 8) as u8;
    let idx = (wval & 0xff) as u8;
    let data: &[u8] = match kind {
        1 => &DEV_DESC,
        2 => &CFG_DESC,
        3 => match idx {
            0 => &STR0,
            1 => &STR_MFR,
            2 => &STR_PRD,
            3 => &STR_MAC,
            _ => return false,
        },
        6 => &QUAL_DESC,
        0x0f => return false, // BOS：2.0 设备 stall
        _ => return false,
    };
    let n = epbuf_put(data);
    arm_xfer(
        ufs,
        freq,
        t,
        1,
        USB_EPBUF as u64,
        n.min(wlen as u32),
        TRB_DATA,
    );
    true
}

static mut FIFO_DEPTH: u32 = 0;

/// lito-usb.dtsi 有 tx-fifo-resize，RAM 27696。ep0.c 在 SET_CONFIG 时
/// 记下 GTXFIFOSIZ(0) 的深度并清掉其余 IN FIFO。gadget.c
/// resize_tx_fifos 再按 mdwidth 给每个 IN 端点分配。不分配的话
/// 中断 IN / bulk IN 的 FIFO 深度是 0，STARTTRANSFER 能成功但发不出去。
fn tx_fifo_reset() {
    let sz0 = rd32(USB_DWC3 + 0xc300);
    unsafe { FIFO_DEPTH = sz0 & 0xffff };
    wr32(USB_DWC3 + 0xc304, 0);
    wr32(USB_DWC3 + 0xc308, 0);
}

fn tx_fifo_alloc(index: u32) -> u32 {
    let hw = rd32(USB_DWC3 + 0xc140);
    let mut mdw = ((hw >> 8) & 0xff) >> 3;
    if mdw == 0 {
        mdw = 1;
    }
    let tmp = (1024 + mdw) + mdw;
    let depth = (tmp + mdw - 1) / mdw;
    let start = rd32(USB_DWC3 + 0xc300) & 0xffff_0000;
    let last = unsafe { FIFO_DEPTH };
    let new_last = last + depth;
    if new_last.saturating_mul(mdw) >= 27696 {
        return 0;
    }
    let fifo = depth | start.wrapping_add(last << 16);
    unsafe { FIFO_DEPTH = new_last };
    wr32(USB_DWC3 + 0xc300 + (index as usize) * 4, fifo);
    fifo
}

fn s20_set_cfg(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], wval: u16, wlen: u16) -> bool {
    if wlen != 0 || wval > 1 {
        return false;
    }
    if wval == 0 {
        let ena = rd32(USB_DWC3 + 0xc720);
        wr32(USB_DWC3 + 0xc720, ena & 3);
        unsafe { EP0.devst = 1 };
        return true;
    }
    if unsafe { EP0.devst } == 0 {
        return false;
    }
    tx_fifo_reset();
    let f1 = tx_fifo_alloc(1);
    s20_log(ufs, freq, t, " s20 f1=0x", f1 as u64);
    // notify 0x81 → 物理 3，中断，mps 16，bInterval_m1=8（32ms→HS 9，减一）。
    let (p0, p1) = depcfg_intr_in(3, 16, 8);
    enable_ep(ufs, freq, t, 3, p0, p1);
    // composite.c set_config 在状态段之前对接口 0/1 调 set_alt(0)。
    // 控制接口这下 enable(notify)，里面就 STARTTRANSFER；
    // 数据接口 alt 0 末尾 ecm_notify。裸机没有网卡，wValue 用 1
    // （ecm_open），不等 SET_INTERFACE 的状态段。
    let queued = unsafe { EP0.link } == 0 && s20_notify(ufs, freq, t, false);
    unsafe {
        EP0.devst = 2;
        if queued {
            EP0.link = 2;
        }
    }
    true
}

fn s20_set_intf(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], wval: u16, widx: u16, wlen: u16) -> bool {
    if wlen != 0 {
        return false;
    }
    if widx == 1 && wval == 1 {
        let (p0, p1) = depcfg_bulk(2, 512);
        enable_ep(ufs, freq, t, 2, p0, p1);
        let f2 = tx_fifo_alloc(2);
        s20_log(ufs, freq, t, " s20 f2=0x", f2 as u64);
        let (p0, p1) = depcfg_bulk(5, 512);
        enable_ep(ufs, freq, t, 5, p0, p1);
        s20_log(ufs, freq, t, " s20 alt=0x", 1);
        s20_arm_out(ufs, freq, t);
        if unsafe { EP0.garp } {
            unsafe { EP0.garp = false };
            s20_tx_arp(ufs, freq, t, false);
        }
    }
    true
}

/// gadget.c __dwc3_gadget_ep_enable：bulk 和中断先 STARTTRANSFER 一次
/// （TRB0 还没有 HWO，下一条是 HWO 常置的 Link TRB），资源号留在
/// DEPCMD[22:16]。之后只 UPDATETRANSFER，且清掉 CMDACT（无应答）。
fn dep_update(ep: u32, rsc: u32) {
    let phy = USB_DWC3 + 0xc200;
    let saved = rd32(phy);
    let masked = saved & !((1 << 6) | (1 << 8));
    if masked != saved {
        wr32(phy, masked);
    }
    let base = USB_DWC3 + 0xc800 + (ep as usize) * 0x10;
    // send_gadget_ep_cmd 对 Update Transfer 先把 PAR0/1/2 写成 0。
    wr32(base, 0);
    wr32(base + 4, 0);
    wr32(base + 8, 0);
    wr32(base + 0xc, (rsc << 16) | 7);
    if saved & ((1 << 6) | (1 << 8)) != 0 {
        let cur = rd32(phy);
        wr32(phy, cur | (saved & ((1 << 6) | (1 << 8))));
    }
}

/// dwc3_ep_inc_trb：加一，碰到 Link（第 255 项）就绕回 0。
fn trb_next(i: u8) -> u8 {
    let n = i.wrapping_add(1);
    if n as usize == RING_NUM - 1 { 0 } else { n }
}

fn trb_done(ep: u32) -> u8 {
    let i = unsafe { TRB_DEQ[ep as usize] };
    unsafe { TRB_DEQ[ep as usize] = trb_next(i) };
    i
}

fn s20_arm_bulk(
    ufs: &mut Ufs,
    freq: u32,
    t: &mut [u8; 96],
    ep: u32,
    trb: u64,
    buf: u64,
    len: u32,
) -> bool {
    let a = trb as usize;
    let link = a + (RING_NUM - 1) * 16;
    let on = unsafe { XFER_ON & (1 << ep) != 0 };
    if !on {
        wr32(link, trb as u32);
        wr32(link + 4, (trb >> 32) as u32);
        wr32(link + 8, 0);
        wr32(link + 12, (8 << 4) | 1);
        wr32(a, 0);
        wr32(a + 4, 0);
        wr32(a + 8, 0);
        wr32(a + 12, 0);
        dcache_civac(trb, 64);
        dcache_civac(link as u64, 16);
        match dep_cmd(ep, 6, (trb >> 32) as u32, trb as u32, 0) {
            Some(0) => {
                let reg = rd32(USB_DWC3 + 0xc800 + (ep as usize) * 0x10 + 0xc);
                let rsc = (reg >> 16) & 0x7f;
                unsafe {
                    RSC[ep as usize] = rsc;
                    XFER_ON |= 1 << ep;
                    TRB_ENQ[ep as usize] = 0;
                    TRB_DEQ[ep as usize] = 0;
                }
                s20_log(ufs, freq, t, " s20 rs=0x", ((ep as u64) << 16) | rsc as u64);
            }
            Some(st) => {
                s20_fail(ufs, freq, t, ep, 6, st);
                return false;
            }
            None => {
                s20_fail(ufs, freq, t, ep, 6, 0xff);
                return false;
            }
        }
    }
    // 完成事件把控制器拨到下一项。再写 TRB0 的话，指针已经停在
    // HWO=0 的 TRB1 上，后面的令牌不再落到这条缓冲。
    let idx = unsafe { TRB_ENQ[ep as usize] };
    let at = trb + (idx as u64) * 16;
    let slot = at as usize;
    // 设备走 NC，不窥探。一条 64B 行里有四条 TRB。先丢掉核上的旧行，
    // 免得 civac 把已经完成的那条 HWO 又写成 1。
    dcache_ivac(at, 16);
    let mut ctl = TRB_NORMAL | 1 | (1 << 11);
    if ep == 2 {
        ctl |= 1 << 3;
    }
    wr32(slot, buf as u32);
    wr32(slot + 4, (buf >> 32) as u32);
    wr32(slot + 8, len & 0x00ff_ffff);
    // databook 4.2.3.2：先让前三个字可见，再写带 HWO 的 ctrl。
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32(slot + 12, ctl);
    dcache_civac(at, 16);
    unsafe { TRB_ENQ[ep as usize] = trb_next(idx) };
    // 只记前两条。后面每来一包 IPv6 都记，24 条配额在枚举末尾就用完，
    // ping 时的 icmp 会被挤掉。
    if idx < 2 {
        s20_keep(
            ufs,
            freq,
            t,
            " s20 q=0x",
            ((ep as u64) << 8) | idx as u64,
        );
    }
    let rsc = unsafe { RSC[ep as usize] };
    dep_update(ep, rsc);
    true
}

fn s20_arm_out(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    let trb = core::ptr::addr_of!(OUT_RING) as u64;
    let buf = core::ptr::addr_of!(OUT_BUF) as u64;
    s20_arm_bulk(ufs, freq, t, 2, trb, buf, BULK_LEN);
}

fn s20_arm_in(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], len: u32) {
    if unsafe { IN_BUSY } || len == 0 || len > BULK_LEN {
        return;
    }
    let buf = core::ptr::addr_of!(IN_BUF) as u64;
    dcache_civac(buf, len as u64);
    let trb = core::ptr::addr_of!(IN_RING) as u64;
    unsafe { IN_BUSY = true };
    if !s20_arm_bulk(ufs, freq, t, 5, trb, buf, len) {
        unsafe { IN_BUSY = false };
    }
}

fn s20_tx_arp(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], req: bool) {
    if unsafe { IN_BUSY } {
        return;
    }
    unsafe {
        IN_BUF = [0; 1536];
        if req {
            IN_BUF[..6].copy_from_slice(&OUT_BUF[6..12]);
        } else {
            IN_BUF[..6].fill(0xff);
        }
        IN_BUF[6..12].copy_from_slice(&OUR_MAC);
        IN_BUF[12] = 0x08;
        IN_BUF[13] = 0x06;
        IN_BUF[14] = 0x00;
        IN_BUF[15] = 0x01;
        IN_BUF[16] = 0x08;
        IN_BUF[17] = 0x00;
        IN_BUF[18] = 6;
        IN_BUF[19] = 4;
        IN_BUF[20] = 0x00;
        IN_BUF[21] = 0x02;
        IN_BUF[22..28].copy_from_slice(&OUR_MAC);
        IN_BUF[28..32].copy_from_slice(&OUR_IP);
        if req {
            IN_BUF[32..38].copy_from_slice(&OUT_BUF[6..12]);
            IN_BUF[38..42].copy_from_slice(&OUT_BUF[28..32]);
        } else {
            IN_BUF[32..38].copy_from_slice(&OUR_MAC);
            IN_BUF[38..42].copy_from_slice(&OUR_IP);
        }
    }
    s20_arm_in(ufs, freq, t, 42);
    s20_keep(ufs, freq, t, " s20 arp=0x", 1);
}

fn s20_on_frame(n: usize) {
    if n < 14 || n > BULK_LEN as usize {
        return;
    }
    unsafe {
        if RX_LEN != 0 {
            return;
        }
        RX_COPY[..n].copy_from_slice(&OUT_BUF[..n]);
        RX_LEN = n;
    }
}

fn s20_gratuitous(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    let ena = rd32(USB_DWC3 + 0xc720);
    if ena & (1 << 5) == 0 {
        unsafe { EP0.garp = true };
        return;
    }
    s20_tx_arp(ufs, freq, t, false);
}

fn s20_on_out(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], idx: u8) {
    let base = core::ptr::addr_of!(OUT_RING) as u64;
    let trb = base + (idx as u64) * 16;
    let buf = core::ptr::addr_of!(OUT_BUF) as u64;
    dcache_ivac(trb, 16);
    dcache_ivac(buf, BULK_LEN as u64);
    let left = rd32(trb as usize + 8) & 0x00ff_ffff;
    let n = if left <= BULK_LEN { (BULK_LEN - left) as usize } else { 0 };
    s20_on_frame(n);
    s20_arm_out(ufs, freq, t);
}

fn s20_notify(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], speed: bool) -> bool {
    let n = if speed { 16 } else { 8 };
    unsafe {
        NOTE_BUF = [0; 16];
        NOTE_BUF[0] = 0xa1;
        NOTE_BUF[1] = if speed { 0x2a } else { 0x00 };
        if !speed {
            NOTE_BUF[2] = 1;
        } else {
            NOTE_BUF[6] = 8;
            let spd = rd32(USB_DWC3 + 0xc70c) & 7;
            // f_ecm.c ecm_bitrate：高速 13*512*8*1000*8，全速 19*64*1000*8。
            let bps: u32 = if spd == 0 { 425_984_000 } else { 9_728_000 };
            let b = bps.to_le_bytes();
            NOTE_BUF[8..12].copy_from_slice(&b);
            NOTE_BUF[12..16].copy_from_slice(&b);
        }
    }
    let buf = core::ptr::addr_of!(NOTE_BUF) as u64;
    dcache_civac(buf, 16);
    let trb = core::ptr::addr_of!(NOTE_RING) as u64;
    if s20_arm_bulk(ufs, freq, t, 3, trb, buf, n) {
        s20_keep(ufs, freq, t, " s20 ln=0x", if speed { 2 } else { 1 });
        return true;
    }
    false
}

/// cmd-db（lito.dtsi reserved-memory@80860000）。头 144B 后是条目，
/// 条目 id 8 字节、addr 在 +16。rpmh-regulator.c：VRM 寄存器地址 =
/// addr + index*4（0 电压 mV，1 使能，2 模式）。
const CMD_DB: usize = 0x8086_0000;
const CMD_DB_BYTES: usize = 0x2_0000;
const CMD_DB_MAGIC: u32 = 0x0c03_30db;
/// apps_rsc drv-2（0x18220000）+ qcom,tcs-offset 0xd00，ACTIVE 的 TCS0。
const RSC_TCS0: usize = 0x1822_0d00;
/// (addr >> 16) & 7 == CMD_DB_HW_VRM（cmd-db.h = 4）。
const RPMH_ACCEL_VREG: u32 = 4;

fn cmd_db_find(id0: u32, id1: u32) -> u32 {
    if rd32(CMD_DB + 4) != CMD_DB_MAGIC {
        return 0;
    }
    for i in 0..8usize {
        let base = 8 + i * 16;
        let slv_hdr = rd32(CMD_DB + base);
        if slv_hdr & 0xffff == 0 {
            break;
        }
        let header_off = (slv_hdr >> 16) & 0xffff;
        // +4 字：低 16 data_offset，高 16 cnt。
        let cnt = (rd32(CMD_DB + base + 4) >> 16) & 0xffff;
        let n = cnt.min(512);
        for j in 0..n {
            let ent = 144 + header_off as usize + (j as usize) * 24;
            if ent.saturating_add(24) > CMD_DB_BYTES {
                break;
            }
            if rd32(CMD_DB + ent) == id0 && rd32(CMD_DB + ent + 4) == id1 {
                return rd32(CMD_DB + ent + 16);
            }
        }
    }
    0
}

fn db_id(name: &[u8; 8]) -> u32 {
    cmd_db_find(
        u32::from_le_bytes([name[0], name[1], name[2], name[3]]),
        u32::from_le_bytes([name[4], name[5], name[6], name[7]]),
    )
}

fn vreg_addr(addr: u32) -> bool {
    addr != 0 && (addr >> 16) & 7 == RPMH_ACCEL_VREG
}

fn tcs_sync(reg: usize, val: u32, freq: u32) -> bool {
    wr32(RSC_TCS0 + reg, val);
    let end = time::cntvct() + (freq as u64) / 1000;
    while rd32(RSC_TCS0 + reg) != val {
        if time::cntvct() > end {
            return false;
        }
    }
    true
}

/// 清 AMC 扳机和命令使能。票超时也要走这里，免得 TCS 一直占着。
fn rpmh_quiet(freq: u32) {
    let mut ctl = rd32(RSC_TCS0 + 0x14);
    ctl &= !(1 << 24);
    if !tcs_sync(0x14, ctl, freq) {
        return;
    }
    ctl &= !(1 << 16);
    if !tcs_sync(0x14, ctl, freq) {
        return;
    }
    wr32(RSC_TCS0 + 0x1c, 0);
    wr32(RSC_TCS0 + 0x10, 0);
    wr32(RSC_TCS0 + 0x08, 1);
}

/// 一条 ACTIVE 写。返回 CMD_STATUS 低 20 位；bit31 置位=没完成。
fn rpmh_cmd(addr: u32, data: u32, freq: u32) -> u32 {
    if !rpmh_arm(freq) {
        return 0x8000_0001;
    }
    // CMD_MSGID：LEN=8，RESP_REQ，WRITE。与 rpmh-rsc.c 同值。
    let msgid = 8u32 | (1 << 8) | (1 << 16);
    wr32(RSC_TCS0 + 0x30, msgid);
    wr32(RSC_TCS0 + 0x34, addr);
    wr32(RSC_TCS0 + 0x38, data);
    wr32(RSC_TCS0 + 0x10, 1);
    wr32(RSC_TCS0 + 0x1c, 1);
    if !tcs_sync(0x14, 1 << 16, freq) {
        return 0x8000_0003;
    }
    wr32(RSC_TCS0 + 0x14, (1 << 16) | (1 << 24));
    let end = time::cntvct() + (freq as u64) / 10;
    loop {
        let sts = rd32(RSC_TCS0 + 0x3c);
        if sts & (1 << 16) != 0 {
            rpmh_quiet(freq);
            return sts & 0xfffff;
        }
        if time::cntvct() > end {
            rpmh_quiet(freq);
            return 0x8000_0000 | (sts & 0xfffff);
        }
    }
}

fn rpmh_arm(freq: u32) -> bool {
    wr32(RSC_TCS0 + 0x08, 1);
    let mut ctl = rd32(RSC_TCS0 + 0x14);
    ctl &= !(1 << 24);
    if !tcs_sync(0x14, ctl, freq) {
        return false;
    }
    ctl &= !(1 << 16);
    if !tcs_sync(0x14, ctl, freq) {
        return false;
    }
    wr32(RSC_TCS0 + 0x1c, 0);
    wr32(RSC_TCS0 + 0x10, 0);
    true
}

fn rpmh_vrm(addr: u32, mv: u32, mode: u32, freq: u32) -> u64 {
    let s0 = rpmh_cmd(addr, mv, freq);
    let s1 = rpmh_cmd(addr.wrapping_add(4), 1, freq);
    let s2 = rpmh_cmd(addr.wrapping_add(8), mode, freq);
    let mut out = (s0 as u64 & 0xfffff)
        | ((s1 as u64 & 0xfffff) << 20)
        | ((s2 as u64 & 0xfffff) << 40);
    if (s0 | s1 | s2) & 0x8000_0000 != 0 {
        out |= 1 << 63;
    }
    out
}

/// cmd-db.h：ARC 加速器是 3。地址高半字的低 3 位是这个类型。
const CMD_DB_HW_ARC: u32 = 3;
/// cmd-db.h：BCM 是 5（IPA bus vote 终点）。
const CMD_DB_HW_BCM: u32 = 5;
/// rpmh-regulator-levels.h。ARC 写进 RPMh 的是 aux 表里的下标，不是 384。
const ARC_TURBO: u16 = 384;
/// clk-rpmh.c lito：`xo.lvl` 打开时数据就是 0x3，不走电压档映射。
const XO_ON: u32 = 0x3;
/// qmp_aop msgram。这一刀只读描述符，不跑链路状态机，也不发 load_state。
const QMP_MSGRAM: usize = 0x0c30_0000;

struct DbHit {
    addr: u32,
    len: u16,
    aux: u32,
}

fn cmd_db_hit(name: &[u8; 8]) -> Option<DbHit> {
    let id0 = u32::from_le_bytes([name[0], name[1], name[2], name[3]]);
    let id1 = u32::from_le_bytes([name[4], name[5], name[6], name[7]]);
    if rd32(CMD_DB + 4) != CMD_DB_MAGIC {
        return None;
    }
    for i in 0..8usize {
        let base = 8 + i * 16;
        let slv = rd32(CMD_DB + base);
        if slv & 0xffff == 0 {
            break;
        }
        let header_off = (slv >> 16) & 0xffff;
        let w1 = rd32(CMD_DB + base + 4);
        let data_off = w1 & 0xffff;
        let cnt = ((w1 >> 16) & 0xffff).min(512);
        for j in 0..cnt {
            let ent = 144 + header_off as usize + (j as usize) * 24;
            if ent.saturating_add(24) > CMD_DB_BYTES {
                break;
            }
            if rd32(CMD_DB + ent) != id0 || rd32(CMD_DB + ent + 4) != id1 {
                continue;
            }
            let addr = rd32(CMD_DB + ent + 16);
            let lens = rd32(CMD_DB + ent + 20);
            let len = (lens & 0xffff) as u16;
            let off = (lens >> 16) & 0xffff;
            let aux = 144 + data_off + off;
            return Some(DbHit { addr, len, aux });
        }
    }
    None
}

/// aux 是 u16 档位表。只要和 `want` 相同的那一档，不挑“下一档”。
fn arc_hlvl(aux: u32, len: u16, want: u16) -> Option<u32> {
    if len == 0 || len % 2 != 0 || len > 32 {
        return None;
    }
    let start = aux as usize;
    if start < 144 || start.saturating_add(len as usize) > CMD_DB_BYTES {
        return None;
    }
    let n = (len / 2) as u32;
    let mut i = 0u32;
    while i < n {
        let w = rd16a(CMD_DB as u64 + aux as u64 + (i as u64) * 2);
        if i > 0 && w == 0 {
            break;
        }
        if w == want {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 只查档，不写 RPMh。上一张在枚举前发了这三笔，USB 没起来。
fn proxy_arc(name: &[u8; 8], slot: usize, aux_slot: usize) {
    unsafe {
        PROXY[slot] = 0;
        PROXY[slot + 1] = 0x100;
        PROXY[slot + 2] = 0;
        PROXY_AUX[aux_slot] = 0;
    }
    let Some(hit) = cmd_db_hit(name) else {
        return;
    };
    unsafe { PROXY[slot] = hit.addr; }
    if hit.len >= 4
        && (hit.aux as usize) >= 144
        && (hit.aux as usize) + 4 <= CMD_DB_BYTES
    {
        unsafe { PROXY_AUX[aux_slot] = rd32(CMD_DB + hit.aux as usize); }
    }
    if (hit.addr >> 16) & 7 != CMD_DB_HW_ARC {
        unsafe { PROXY[slot + 1] = 0x101; }
        return;
    }
    let Some(sel) = arc_hlvl(hit.aux, hit.len, ARC_TURBO) else {
        unsafe { PROXY[slot + 1] = 0x102; }
        return;
    };
    unsafe { PROXY[slot + 1] = sel; }
}

fn proxy_xo() {
    unsafe {
        PROXY[6] = 0;
        PROXY[7] = 0x100;
        PROXY[8] = 0;
    }
    let Some(hit) = cmd_db_hit(b"xo.lvl\0\0") else {
        return;
    };
    unsafe { PROXY[6] = hit.addr; }
    if (hit.addr >> 16) & 7 != CMD_DB_HW_ARC {
        unsafe { PROXY[7] = 0x101; }
        return;
    }
    unsafe { PROXY[7] = XO_ON; }
}

/// 只解析档位，不写 RPMh，也不落环。魔数已在缓存里就不再 ivac 整段。

/// IPA SVS2 路径终点 BCM 名（固定序，sm bc= 同序）。
/// bcm_probe 只读 cmd-db；ACTIVE vote / SMMU STE 在 ipa_i4_once（PAS 在 ADSP 后）。
const BCM_NAMES: [&[u8; 8]; 7] = [
    b"IP0\0\0\0\0\0",
    b"SH0\0\0\0\0\0",
    b"MC0\0\0\0\0\0",
    b"ACV\0\0\0\0\0",
    b"SN1\0\0\0\0\0",
    b"CN0\0\0\0\0\0",
    b"SH4\0\0\0\0\0",
];
/// sm 标签（与 BCM_NAMES 同序）。
const BCM_TAGS: [&str; 7] = ["IP0", "SH0", "MC0", "ACV", "SN1", "CN0", "SH4"];
/// st: 0 miss / 1 ok(HW=BCM,aux>=8) / 2 hit但HW≠BCM / 3 HW=BCM但aux短 / 4 cmd-db magic 坏
static mut BCM_ST: [u32; 7] = [0; 7];
static mut BCM_ADDR: [u32; 7] = [0; 7];
static mut BCM_UNIT: [u32; 7] = [0; 7];
static mut BCM_WIDTH: [u32; 7] = [0; 7];

fn bcm_probe() {
    if rd32(CMD_DB + 4) != CMD_DB_MAGIC {
        dcache_ivac(CMD_DB as u64, CMD_DB_BYTES as u64);
    }
    let magic_ok = rd32(CMD_DB + 4) == CMD_DB_MAGIC;
    let mut i = 0usize;
    while i < 7 {
        unsafe {
            BCM_ST[i] = 0;
            BCM_ADDR[i] = 0;
            BCM_UNIT[i] = 0;
            BCM_WIDTH[i] = 0;
        }
        if !magic_ok {
            unsafe { BCM_ST[i] = 4 };
            i += 1;
            continue;
        }
        let Some(hit) = cmd_db_hit(BCM_NAMES[i]) else {
            i += 1;
            continue;
        };
        unsafe { BCM_ADDR[i] = hit.addr };
        if (hit.addr >> 16) & 7 != CMD_DB_HW_BCM {
            unsafe { BCM_ST[i] = 2 };
            i += 1;
            continue;
        }
        if hit.len < 8
            || (hit.aux as usize) < 144
            || (hit.aux as usize).saturating_add(8) > CMD_DB_BYTES
        {
            unsafe { BCM_ST[i] = 3 };
            i += 1;
            continue;
        }
        let unit = rd32(CMD_DB + hit.aux as usize);
        let width = rd32(CMD_DB + hit.aux as usize + 4) & 0xffff;
        unsafe {
            BCM_UNIT[i] = unit;
            BCM_WIDTH[i] = width;
            BCM_ST[i] = 1;
        }
        i += 1;
    }
}

/// IPA SMMU SIDs（slot-b groups 13/14/15）。SMR+S2CR 写法同 `fastrpc_smmu`：
/// 复用 USB 已装好的 CB 恒等映射；不新开 CB、不写 PAS。
const IPA_SIDS: [u32; 3] = [0x520, 0x521, 0x522];
/// ste 每 SID：<128 = SMR 槽；fb=USB_CB 未装；fe=无空槽；fc=SID 被他槽盖住；fd=写回失败；ffffffff=未跑
static mut IPA_STE: [u32; 3] = [0xffff_ffff; 3];
/// bits 0..2：SID 0x520/521/522 绑定成功。
static mut IPA_STE_MASK: u32 = 0;
/// I4 SVS2 ACTIVE TCS（commit|valid|vote_x|vote_y）— 见 I4-VOTE-AGGREGATE.md。
const BCM_TCS: [u32; 7] = [
    0x6000_09c4, // IP0
    0x6008_c0ef, // SH0
    0x6024_8555, // MC0
    0x6000_0008, // ACV（enable bit；与 MC0 同批）
    0x6023_4528, // SN1
    0x6000_0a00, // CN0
    0x6000_0012, // SH4
];
/// cmd-db 未命中时用的地址（与 e4d9591 bc= 一致；高半字低 3 位=HW_BCM=5）。
const BCM_FALLBACK_ADDR: [u32; 7] = [
    0x0005_0070, 0x0005_0004, 0x0005_0000, 0x0005_007c, 0x0005_002c, 0x0005_0074,
    0x0005_0014,
];
/// rpmh_cmd 原始返回（bit31=失败）。
static mut BCM_VOTE_ST: [u32; 7] = [0; 7];
/// bits 0..6：七笔 ACTIVE vote 完成且无超时。
static mut BCM_BV: u32 = 0;
/// 1 = 本开机已跑过 STE+vote（只发一次）。
static mut IPA_I4_DONE: u32 = 0;

fn smmu_slot_taken(slot: usize) -> bool {
    if slot == 4 {
        return true;
    }
    let fp = unsafe { FP_SMR };
    if fp < 128 && slot == fp as usize {
        return true;
    }
    let mut i = 0usize;
    while i < 3 {
        let s = unsafe { IPA_STE[i] };
        if s < 128 && slot == s as usize {
            return true;
        }
        i += 1;
    }
    false
}

/// 单 SID：先 S2CR 再 SMR（arm_smmu_write_sme）。TYPE=translation → USB_CB。
fn ipa_smmu_bind_one(sid: u32) -> u32 {
    let cb = unsafe { USB_CB };
    if cb == 0xffff_ffff || cb > 0xff {
        return 0xffff_fffb;
    }
    let base = SMMU_BASE;
    let nsmr = (rd32(base + 0x20) & 0xff) as usize;
    if nsmr == 0 || nsmr > 128 {
        return 0xffff_fffe;
    }
    let mut free: Option<usize> = None;
    let mut i = 0usize;
    while i < nsmr {
        let smr = rd32(base + 0x800 + i * 4);
        if smr & (1 << 31) == 0 {
            if free.is_none() && !smmu_slot_taken(i) {
                free = Some(i);
            }
            i += 1;
            continue;
        }
        let id = smr & 0x7fff;
        let mask = (smr >> 16) & 0x7fff;
        if ((sid ^ id) & !mask) == 0 {
            let s2 = rd32(base + 0xc00 + i * 4);
            if mask == 0 && id == sid && (s2 >> 16) & 3 == 0 && (s2 & 0xff) == cb {
                return i as u32;
            }
            return 0xffff_fffc;
        }
        i += 1;
    }
    let Some(slot) = free else {
        return 0xffff_fffe;
    };
    // SHCFG=NSH（bit9:8=3），同 fastrpc_smmu。
    let s2_val = cb | (3 << 8);
    wr32(base + 0xc00 + slot * 4, s2_val);
    wr32(base + 0x800 + slot * 4, (1 << 31) | sid);
    let sb = rd32(base + 0xc00 + slot * 4);
    let sm = rd32(base + 0x800 + slot * 4);
    if (sb >> 16) & 3 == 0
        && (sb & 0xff) == cb
        && sm & (1 << 31) != 0
        && (sm & 0x7fff) == sid
    {
        return slot as u32;
    }
    0xffff_fffd
}

fn ipa_smmu_ste() {
    let mut i = 0usize;
    while i < 3 {
        if unsafe { IPA_STE[i] } != 0xffff_ffff {
            i += 1;
            continue;
        }
        let r = ipa_smmu_bind_one(IPA_SIDS[i]);
        unsafe { IPA_STE[i] = r };
        if r < 128 {
            unsafe { IPA_STE_MASK |= 1 << i };
        }
        i += 1;
    }
}

/// 七笔 ACTIVE BCM vote。无 pas_go。addr 优先 cmd-db，否则 fallback。
fn ipa_bcm_vote(freq: u32) {
    let mut i = 0usize;
    while i < 7 {
        let addr = if unsafe { BCM_ST[i] } == 1 {
            unsafe { BCM_ADDR[i] }
        } else {
            BCM_FALLBACK_ADDR[i]
        };
        let sts = rpmh_cmd(addr, BCM_TCS[i], freq);
        unsafe { BCM_VOTE_ST[i] = sts };
        if sts & 0x8000_0000 == 0 {
            unsafe { BCM_BV |= 1 << i };
        }
        i += 1;
    }
}

/// IPA wrapper / GSI EE0 只读（lito DTS：ipa@1e00000 + gsi@1e04000；IPAv4.7→GSI 2.7）。
/// 基址：IPA=0x1e00000 reg_base=0x40000；GSI=0x1e04000。无 doorbell / 无 CH_CMD。
/// 同次另闩 CH7 CNTXT→gc=（初读；ALLOC/START 见 ipa_gsi_ch7_cfg）。
const IPA_WRAP: usize = 0x01e0_0000;
const IPA_REG_BASE_OFST: usize = 0x0004_0000;
const IPA_VER_OFF: usize = 0x34;
const IPA_SMEM_OFF: usize = 0x54;
const IPA_SW_RAM_OFF: usize = 0x1_0000; // v4.5+ IPA_SW_AREA_RAM_DIRECT_ACCESS_n
const GSI_BASE: usize = 0x01e0_4000;
const GSI_CFG_OFF: usize = 0x0;
const GSI_EE0_STATUS: usize = 0x1_2000;
const GSI_EE0_HW_PARAM2: usize = 0x1_2040; // GSI 2.5/2.7
const GSI_EE0_SW_VER: usize = 0x1_2044;
const GSI_EE0_CH0_CNTXT0: usize = 0xf000;
/// EE0 CH_k CNTXT：基 0xf000 + 0x80*k（gsi_reg_v2）。CH7 = APPS_WAN_CONS。
const GSI_EE0_CH7_CNTXT0: usize = 0xf000 + 0x80 * 7; // 0xf380
const GSI_EE0_TYPE_IRQ: usize = 0x1_2080;
const GSI_EE0_ERROR_LOG: usize = 0x1_2200; // 只读；禁 ERROR_LOG_CLR
/// gsi_reg_v2 EE0：CH_CMD / EV_CMD / IRQ 掩码 / doorbell / INTSET（GSI 2.7）。
const GSI_EE0_CH_CMD: usize = 0x1_2008;
const GSI_EE0_EV_CMD: usize = 0x1_2010;
const GSI_EE0_TYPE_IRQ_MSK: usize = 0x1_2088;
const GSI_EE0_SRC_CH_IRQ: usize = 0x1_2090;
const GSI_EE0_SRC_EV_IRQ: usize = 0x1_2094;
const GSI_EE0_SRC_CH_IRQ_MSK: usize = 0x1_2098;
const GSI_EE0_SRC_EV_IRQ_MSK: usize = 0x1_209c;
const GSI_EE0_SRC_CH_IRQ_CLR: usize = 0x1_20a0;
const GSI_EE0_SRC_EV_IRQ_CLR: usize = 0x1_20a4;
const GSI_EE0_IEOB_IRQ_CLR: usize = 0x1_20c0;
const GSI_EE0_IEOB_IRQ_MSK: usize = 0x1_20b8;
const GSI_EE0_INTSET: usize = 0x1_2180;
/// EV0 CNTXT0 @ EE0（0x10000 + 0x80*0）
const GSI_EE0_EV0_CNTXT0: usize = 0x1_0000;
/// CH7 doorbell 0/1（0x11000 + 8*7）
const GSI_EE0_CH7_DB0: usize = 0x1_1000 + 8 * 7;
const GSI_EE0_CH7_DB1: usize = GSI_EE0_CH7_DB0 + 4;
/// EV0 doorbell 0/1
const GSI_EE0_EV0_DB0: usize = 0x1_1100;
const GSI_EE0_EV0_DB1: usize = GSI_EE0_EV0_DB0 + 4;
/// CH7 QOS（v2.5：CNTXT 块内 +0x5c）
const GSI_EE0_CH7_QOS: usize = GSI_EE0_CH7_CNTXT0 + 0x5c;
/// CH_CMD / EV_CMD opcodes（msm_gsi.h）
const GSI_CH_ALLOCATE: u32 = 0x0;
const GSI_CH_START: u32 = 0x1;
const GSI_EVT_ALLOCATE: u32 = 0x0;
/// CHSTATE（CNTXT0[23:20]）
const GSI_CHSTATE_ALLOCATED: u32 = 0x1;
const GSI_CHSTATE_STARTED: u32 = 0x2;
/// GPI FROM_GSI / re=16 / EVT GPI_EV+IRQ（msm_gsi.h）
const GSI_PROT_GPI: u32 = 0x2;
const GSI_DIR_FROM_GSI: u32 = 0x0;
const GSI_RE_SIZE_16: u32 = 16;
const GSI_EVT_CHTYPE_GPI_EV: u32 = 0x2;
const GSI_INTR_IRQ: u32 = 0x1;
const GSI_ESCAPE_BUF_ONLY: u32 = 0x1;
const GSI_RE_XFER: u32 = 0x2;
/// IPA_GENERIC_RX_BUFF_BASE_SZ（ipa_dp.c）— TRE len 用基值，不对齐 SKB 宏
const GSI_RX_BUF_SZ: u64 = 0x2000;
/// 最少 RX TRE 数（Linux 会灌满 pool；本刀只证 doorbell 非空）
const GSI_RX_TRE_MIN: u32 = 4;
/// ALLOC/START 轮询超时（ms）。Linux：ALLOC wait 5*HZ；START 1s×5 poll ≈5s。
const GSI_CMD_TIMEOUT_MS: u64 = 5000;
/// ipa_4_7_mem_part canary 位点（相对 SW SRAM；canary 在 section-4）
const IPA47_CA_OFF: usize = 0x684; // modem_hdr_ofst(0x688)-4
const IPA47_CB_OFF: usize = 0x284; // v4_flt_hash_ofst(0x288)-4
const IPA_CANARY: u32 = 0xdeadbeef;

/// `_ipa_init_sram_v3`：`sram[(offset-4)/4] = 0xdeadbeef`。
fn ipa_sram_canary(sram: usize, offset: usize) {
    if offset >= 4 {
        wr32(sram + offset - 4, IPA_CANARY);
    }
}

/// IPA 4.7（hw >= 4.5）的 canary 位点。不 START GSI，不写 doorbell。
fn ipa_sram_canaries(sram: usize) {
    let pair = [
        0x288usize, 0x308, 0x388, 0x408, 0x488, 0x508, 0x588, 0x608, 0x688, 0xad0,
    ];
    let mut i = 0usize;
    while i < pair.len() {
        ipa_sram_canary(sram, pair[i] - 4);
        ipa_sram_canary(sram, pair[i]);
        i += 1;
    }
    ipa_sram_canary(sram, 0x17a0 - 12);
    ipa_sram_canary(sram, 0x2500 - 12);
    ipa_sram_canary(sram, 0x27d8 - 4);
    ipa_sram_canary(sram, 0x27d8);
    ipa_sram_canary(sram, 0x3000);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
}

/// `ipa3_init_smem_region` 把这几段写成 0。这里用 SRAM 直写，不走立即命令。
fn ipa_sram_zero(sram: usize, off: usize, size: usize) {
    let mut i = 0usize;
    while i < size {
        wr32(sram + off + i, 0);
        i += 4;
    }
}

/// 开机不写。第一次 sm 之后写，并刷新 ig 的 ca/cb。
fn ipa_canary_late() {
    if unsafe { CAN_DONE } != 0 || unsafe { CFG_ARM } == 0 {
        return;
    }
    let ipa_ok = unsafe { IPA_ST } & 0x8000_0000 != 0
        && unsafe { IPA_P[0] } == 0
        && unsafe { IPA_P[1] } == 0
        && unsafe { IPA_P[2] } == 0;
    if !ipa_ok {
        return;
    }
    let ipa_mmio = IPA_WRAP + IPA_REG_BASE_OFST;
    let smem = rd32(ipa_mmio + IPA_SMEM_OFF);
    let restr = (((smem >> 16) & 0xffff) as usize).saturating_mul(8);
    let sram = ipa_mmio + IPA_SW_RAM_OFF + restr;
    ipa_sram_canaries(sram);
    unsafe {
        IG_CA = rd32(sram + IPA47_CA_OFF);
        IG_CB = rd32(sram + IPA47_CB_OFF);
        CAN_DONE = 1;
    }
}

fn ipa_gsi_ro_probe() {
    if unsafe { IG_DONE } != 0 {
        return;
    }
    let ipa_ok = unsafe { IPA_ST } & 0x8000_0000 != 0
        && unsafe { IPA_P[0] } == 0
        && unsafe { IPA_P[1] } == 0
        && unsafe { IPA_P[2] } == 0;
    if !ipa_ok {
        return;
    }
    let ipa_mmio = IPA_WRAP + IPA_REG_BASE_OFST;
    let iver = rd32(ipa_mmio + IPA_VER_OFF);
    let smem = rd32(ipa_mmio + IPA_SMEM_OFF);
    let gcfg = rd32(GSI_BASE + GSI_CFG_OFF);
    let gstat = rd32(GSI_BASE + GSI_EE0_STATUS);
    let gver = rd32(GSI_BASE + GSI_EE0_SW_VER);
    let hp2 = rd32(GSI_BASE + GSI_EE0_HW_PARAM2);
    let ch0 = rd32(GSI_BASE + GSI_EE0_CH0_CNTXT0);
    // CH7（APPS_WAN_CONS）CNTXT0..3 + TYPE_IRQ + ERROR_LOG — 只读；无 CH_CMD/doorbell。
    let c7_0 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0);
    let c7_1 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 4);
    let c7_2 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 8);
    let c7_3 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 12);
    let tirq = rd32(GSI_BASE + GSI_EE0_TYPE_IRQ);
    let errl = rd32(GSI_BASE + GSI_EE0_ERROR_LOG);
    // SHARED_MEM_SIZE：baddr[31:16]/size[15:0]，单位 8B → restr_bytes = baddr*8
    let baddr_u = (smem >> 16) & 0xffff;
    let restr = (baddr_u as usize).saturating_mul(8);
    let sram = ipa_mmio + IPA_SW_RAM_OFF + restr;
    // 先清 modem_hdr / proc_ctx / modem，再写 canary。对照 ipa3_init_q6_smem。
    ipa_sram_zero(sram, 0x688, 0x240);
    ipa_sram_zero(sram, 0xad0, 0xac0);
    ipa_sram_zero(sram, 0x27d8, 0x800);
    ipa_sram_canaries(sram);
    let ca = rd32(sram + IPA47_CA_OFF);
    let cb = rd32(sram + IPA47_CB_OFF);
    unsafe {
        IG_IVER = iver;
        IG_GCFG = gcfg;
        IG_GSTAT = gstat;
        IG_GVER = gver;
        IG_HP2 = hp2;
        IG_CH0 = ch0;
        IG_CA = ca;
        IG_CB = cb;
        IG_SMEM = smem;
        IG_R = restr as u32;
        IG_ST = 1;
        IG_DONE = 1;
        GC_C0 = c7_0;
        GC_C1 = c7_1;
        GC_C2 = c7_2;
        GC_C3 = c7_3;
        GC_IRQ = tirq;
        GC_ERR = errl;
        GC_ST = 1;
    }
}

/// 只分配：清零 CH7 ring/EVT/RX 划区并 civac；不写 GSI CNTXT / CH_CMD / doorbell。
fn ipa_gsi_ring_carve() {
    if unsafe { GR_DONE } != 0 {
        return;
    }
    let ipa_ok = unsafe { IPA_ST } & 0x8000_0000 != 0
        && unsafe { IPA_P[0] } == 0
        && unsafe { IPA_P[1] } == 0
        && unsafe { IPA_P[2] } == 0;
    if !ipa_ok {
        return;
    }
    unsafe {
        core::ptr::write_bytes(GSI_RING_BASE as *mut u8, 0, GSI_CARVE_SZ as usize);
    }
    dcache_civac(GSI_RING_BASE, GSI_CARVE_SZ);
    unsafe {
        GR_CH = (GSI_RING_BASE + GSI_CH_RING_OFF) as u32;
        GR_EVT = (GSI_RING_BASE + GSI_EVT_RING_OFF) as u32;
        GR_RX = (GSI_RING_BASE + GSI_RX_POOL_OFF) as u32;
        GR_SZ = GSI_CARVE_SZ as u32;
        GR_ST = 1;
        GR_DONE = 1;
    }
}

/// 轮询 CH/EV CNTXT0.CHSTATE 至 want，或超时。返回 true=命中。
/// 不依赖 GIC：HW 改 CNTXT0 即可；顺带清 SRC_*_IRQ 位（best-effort）。
fn gsi_poll_chstate(cntxt0: usize, irq_src: usize, irq_bit: u32, want: u32, freq: u32) -> bool {
    let end = time::cntvct() + (freq as u64) * GSI_CMD_TIMEOUT_MS / 1000;
    loop {
        let v = rd32(GSI_BASE + cntxt0);
        let st = (v >> 20) & 0xf;
        if st == want {
            // 清 pending（若有）
            let pend = rd32(GSI_BASE + irq_src);
            if pend & irq_bit != 0 {
                wr32(
                    GSI_BASE
                        + if irq_src == GSI_EE0_SRC_CH_IRQ {
                            GSI_EE0_SRC_CH_IRQ_CLR
                        } else {
                            GSI_EE0_SRC_EV_IRQ_CLR
                        },
                    irq_bit,
                );
            }
            return true;
        }
        if time::cntvct() > end {
            return false;
        }
    }
}

/// 写一颗 GPI TRE（16B）到 CH ring @ idx。公式：gsi.h `struct gsi_tre` +
/// ipa_dp.c replenish flags = EOT|EOB|BEI、type=GSI_RE_XFER。
fn gsi_write_rx_tre(ring_pa: u64, idx: u32, buf_pa: u64, buf_len: u16) {
    let p = (ring_pa + (idx as u64) * 16) as *mut u32;
    unsafe {
        p.add(0).write_volatile(buf_pa as u32);
        p.add(1).write_volatile((buf_pa >> 32) as u32);
        p.add(2).write_volatile(buf_len as u32);
        // flags@12 = bei|ieot|ieob；re_type@14 = XFER
        p.add(3).write_volatile(0x0002_0700);
    }
}

/// GSI CH7 config/start（划区 PA；INIT 另刀；无 EP16 cfg）。
/// 序对照 Linux：EV ALLOC→program→prime → CH ALLOC→program → 填 TRE →
/// doorbell → CH START → DB MSB。完成判据：CHSTATE=STARTED（超时 5s 记 st）。
/// sm gc= 在本刀后重读 CNTXT（含 CHSTATE）；gr= 不变。
fn ipa_gsi_ch7_cfg() {
    if unsafe { GC_CFG_DONE } != 0 {
        return;
    }
    if unsafe { GR_ST } != 1 || unsafe { IG_ST } != 1 {
        return;
    }
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    let ch_pa = GSI_RING_BASE + GSI_CH_RING_OFF;
    let evt_pa = GSI_RING_BASE + GSI_EVT_RING_OFF;
    let rx_pa = GSI_RING_BASE + GSI_RX_POOL_OFF;
    let ring_len: u32 = GSI_CH_RING_SZ as u32; // 0x1000
    let re: u32 = GSI_RE_SIZE_16;
    let max_elem = (ring_len / re) - 1; // 255

    // IRQ 面：TYPE CH_CTRL|EV_CTRL；SRC mask CH7 + EV0；INTSET=IRQ
    wr32(
        GSI_BASE + GSI_EE0_TYPE_IRQ_MSK,
        rd32(GSI_BASE + GSI_EE0_TYPE_IRQ_MSK) | 0x3,
    );
    wr32(
        GSI_BASE + GSI_EE0_SRC_CH_IRQ_MSK,
        rd32(GSI_BASE + GSI_EE0_SRC_CH_IRQ_MSK) | (1 << 7),
    );
    wr32(
        GSI_BASE + GSI_EE0_SRC_EV_IRQ_MSK,
        rd32(GSI_BASE + GSI_EE0_SRC_EV_IRQ_MSK) | 1,
    );
    wr32(GSI_BASE + GSI_EE0_INTSET, GSI_INTR_IRQ);

    // ---- EV0 ALLOC ----
    wr32(
        GSI_BASE + GSI_EE0_EV_CMD,
        (GSI_EVT_ALLOCATE << 24) | 0u32, // chid=0
    );
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    let ev_ok = gsi_poll_chstate(
        GSI_EE0_EV0_CNTXT0,
        GSI_EE0_SRC_EV_IRQ,
        1,
        GSI_CHSTATE_ALLOCATED,
        freq,
    );
    if !ev_ok {
        // 超时：仍读回 gc，st=2
        unsafe {
            GC_C0 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0);
            GC_C1 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 4);
            GC_C2 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 8);
            GC_C3 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 12);
            GC_IRQ = rd32(GSI_BASE + GSI_EE0_TYPE_IRQ);
            GC_ERR = rd32(GSI_BASE + GSI_EE0_ERROR_LOG);
            GC_ST = 2; // EV ALLOC timeout
            GC_CFG_DONE = 1;
        }
        return;
    }

    // EV0 CNTXT program（gsi_program_evt_ring_ctx）
    // CNTXT0: ELEMENT_SIZE<<24 | INTYPE<<16 | CHTYPE
    let ev_c0 = (re << 24) | (GSI_INTR_IRQ << 16) | GSI_EVT_CHTYPE_GPI_EV;
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0, ev_c0);
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0 + 4, ring_len); // R_LENGTH
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0 + 8, evt_pa as u32);
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0 + 12, (evt_pa >> 32) as u32);
    // CNTXT8: int_modc=1, int_modt=16（ipa_dp.c 非 napi）
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0 + 0x20, (1u32 << 16) | 16);
    // CNTXT9..13 MSI/RP = 0
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0 + 0x24, 0);
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0 + 0x28, 0);
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0 + 0x2c, 0);
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0 + 0x30, 0);
    wr32(GSI_BASE + GSI_EE0_EV0_CNTXT0 + 0x34, 0);

    // prime EVT：wp = base + max_elem * re（gsi_prime_evt_ring）
    let ev_wp = evt_pa + (max_elem as u64) * (re as u64);
    wr32(GSI_BASE + GSI_EE0_EV0_DB1, (ev_wp >> 32) as u32);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32(GSI_BASE + GSI_EE0_EV0_DB0, ev_wp as u32);
    wr32(GSI_BASE + GSI_EE0_IEOB_IRQ_CLR, 1);
    wr32(
        GSI_BASE + GSI_EE0_IEOB_IRQ_MSK,
        rd32(GSI_BASE + GSI_EE0_IEOB_IRQ_MSK) | 1,
    );

    // ---- CH7 ALLOC ----
    wr32(
        GSI_BASE + GSI_EE0_CH_CMD,
        (GSI_CH_ALLOCATE << 24) | 7u32,
    );
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    let ch_ok = gsi_poll_chstate(
        GSI_EE0_CH7_CNTXT0,
        GSI_EE0_SRC_CH_IRQ,
        1 << 7,
        GSI_CHSTATE_ALLOCATED,
        freq,
    );
    if !ch_ok {
        unsafe {
            GC_C0 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0);
            GC_C1 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 4);
            GC_C2 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 8);
            GC_C3 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 12);
            GC_IRQ = rd32(GSI_BASE + GSI_EE0_TYPE_IRQ);
            GC_ERR = rd32(GSI_BASE + GSI_EE0_ERROR_LOG);
            GC_ST = 3; // CH ALLOC timeout
            GC_CFG_DONE = 1;
        }
        return;
    }

    // CH7 CNTXT（gsi_program_chan_ctx）：GPI FROM_GSI erindex=0 re=16
    let ch_c0 = (re << 24) | (0u32 << 14) /* ERINDEX=0 */ | (GSI_DIR_FROM_GSI << 3) | GSI_PROT_GPI;
    wr32(GSI_BASE + GSI_EE0_CH7_CNTXT0, ch_c0);
    wr32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 4, ring_len);
    wr32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 8, ch_pa as u32);
    wr32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 12, (ch_pa >> 32) as u32);
    // QOS v2.5：weight=1, max_prefetch=0, use_db=DIRECT(0), prefetch=ESCAPE_BUF_ONLY, empty=0
    let qos = (0u32 << 16) /* empty */ | (GSI_ESCAPE_BUF_ONLY << 10) | (0 << 9) | (0 << 8) | 1;
    wr32(GSI_BASE + GSI_EE0_CH7_QOS, qos);
    // scratch 清零（IPA4.5+ smart prefetch 由 HW；MCS 字段保持 0）
    wr32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 0x60, 0);
    wr32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 0x64, 0);
    wr32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 0x68, 0);
    wr32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 0x6c, 0);

    // ---- 最小 RX TRE（Linux 在 START 前 replenish；公式已 attested）----
    for i in 0..GSI_RX_TRE_MIN {
        let buf = rx_pa + (i as u64) * GSI_RX_BUF_SZ;
        gsi_write_rx_tre(ch_pa, i, buf, GSI_RX_BUF_SZ as u16);
    }
    dcache_civac(ch_pa, (GSI_RX_TRE_MIN as u64) * 16);
    dcache_civac(rx_pa, (GSI_RX_TRE_MIN as u64) * GSI_RX_BUF_SZ);
    let ch_wp = ch_pa + (GSI_RX_TRE_MIN as u64) * (re as u64);

    // doorbell：先 EVT（FROM_GSI 路径 gsi_ring_chan_doorbell），再 CH LSB
    wr32(GSI_BASE + GSI_EE0_EV0_DB1, (ev_wp >> 32) as u32);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32(GSI_BASE + GSI_EE0_EV0_DB0, ev_wp as u32);
    wr32(GSI_BASE + GSI_EE0_CH7_DB1, (ch_wp >> 32) as u32);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32(GSI_BASE + GSI_EE0_CH7_DB0, ch_wp as u32);

    // ---- CH7 START ----
    wr32(
        GSI_BASE + GSI_EE0_CH_CMD,
        (GSI_CH_START << 24) | 7u32,
    );
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    let st_ok = gsi_poll_chstate(
        GSI_EE0_CH7_CNTXT0,
        GSI_EE0_SRC_CH_IRQ,
        1 << 7,
        GSI_CHSTATE_STARTED,
        freq,
    );
    // START 后 Linux 再写 DB MSB（本 PA 高 32 位=0）
    wr32(GSI_BASE + GSI_EE0_CH7_DB1, (ch_wp >> 32) as u32);

    // 重读 gc=
    let c0 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0);
    let c1 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 4);
    let c2 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 8);
    let c3 = rd32(GSI_BASE + GSI_EE0_CH7_CNTXT0 + 12);
    let tirq = rd32(GSI_BASE + GSI_EE0_TYPE_IRQ);
    let errl = rd32(GSI_BASE + GSI_EE0_ERROR_LOG);
    unsafe {
        GC_C0 = c0;
        GC_C1 = c1;
        GC_C2 = c2;
        GC_C3 = c3;
        GC_IRQ = tirq;
        GC_ERR = errl;
        // st: 1=STARTED 绿；4=START 超时（CNTXT 仍可读）
        GC_ST = if st_ok { 1 } else { 4 };
        GC_CFG_DONE = 1;
    }
}

/// STE（0x520/521/522）→ 七笔 ACTIVE rpmh_cmd。开机 ADSP 后 / 首次 sm 各最多一次。
fn ipa_i4_once(freq: u32) {
    if unsafe { IPA_I4_DONE } != 0 {
        return;
    }
    unsafe { IPA_I4_DONE = 1 };
    ipa_smmu_ste();
    ipa_bcm_vote(freq);
}

/// AUTH 前 pil-cdsp bus vote（单变量；勿与 sfr_wipe/cx 同刀）。
/// DTS `qcom,msm-bus,name = "pil-cdsp"`：`<154 821 0 0>` / `<154 821 0 1>`。
/// master 154=`MSM_BUS_MASTER_NPU` → `mas_qnm_npu` bcm **CO2**；
/// slave 821=`MSM_BUS_SLAVE_CDSP_GEM_NOC` → `slv_qns_cdsp_gem_noc` bcm **CO0**。
/// case1 ab=0 ib=1 KBps → KBTOB+div64 保底 vote_y=1 → TCS commit|valid|0|1。
const CDSP_BUS_TCS: u32 = 0x6000_0001;
const CDSP_BUS_NAMES: [&[u8; 8]; 2] = [b"CO2\0\0\0\0\0", b"CO0\0\0\0\0\0"];

fn cdsp_bus_vote(freq: u32) {
    unsafe { CDSP_CB = 0 };
    if rd32(CMD_DB + 4) != CMD_DB_MAGIC {
        dcache_ivac(CMD_DB as u64, CMD_DB_BYTES as u64);
    }
    let mut i = 0usize;
    while i < 2 {
        let Some(hit) = cmd_db_hit(CDSP_BUS_NAMES[i]) else {
            i += 1;
            continue;
        };
        if (hit.addr >> 16) & 7 != CMD_DB_HW_BCM {
            i += 1;
            continue;
        }
        let sts = rpmh_cmd(hit.addr, CDSP_BUS_TCS, freq);
        if sts & 0x8000_0000 == 0 {
            unsafe { CDSP_CB |= 1 << i };
        }
        i += 1;
    }
}

fn proxy_vote(_ufs: &mut Ufs, _freq: u32, _t: &mut [u8; 96]) {
    if rd32(CMD_DB + 4) != CMD_DB_MAGIC {
        dcache_ivac(CMD_DB as u64, CMD_DB_BYTES as u64);
    }
    proxy_arc(b"cx.lvl\0\0", 0, 0);
    proxy_arc(b"mss.lvl\0", 3, 1);
    proxy_xo();
}

fn aop_kick() {
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32(0x4080_0c, 0);
}

fn aop_wait(freq: u32, addr: usize, want: u32, ms: u64) -> bool {
    let end = time::cntvct() + (freq as u64) * ms / 1000;
    while rd32(addr) != want {
        if time::cntvct() > end {
            return false;
        }
    }
    true
}

/// `pil_notify_aop("on")`。链路和通道按刚才 `qn`/`qc` 的实测来。
/// 返回 0 表示 AOP 把 96 字节取走了。
fn aop_image(freq: u32, text: &[u8]) -> u32 {
    if rd32(QMP_MSGRAM) != 0x4d41_494c {
        return 0xe1;
    }
    if rd32(QMP_MSGRAM + 36) != 0xffff {
        let ulink = rd32(QMP_MSGRAM + 12);
        if ulink != 0xffff {
            return 0xe2;
        }
        let sz = rd32(QMP_MSGRAM + 52);
        let off = rd32(QMP_MSGRAM + 56);
        wr32(QMP_MSGRAM + 16, ulink);
        wr32(QMP_MSGRAM + 36, 0xffff);
        wr32(QMP_MSGRAM + 40, 0xffff_0000);
        wr32(QMP_MSGRAM + 44, 0xffff_0000);
        wr32(QMP_MSGRAM + 48, 0xffff_0000);
        wr32(QMP_MSGRAM + 52, sz);
        wr32(QMP_MSGRAM + 56, off);
        aop_kick();
        // qmp_startup 等链路 5 秒（QMP_TOUT_MS）。
        if !aop_wait(freq, QMP_MSGRAM + 40, 0xffff, 5000) {
            return 0xe3;
        }
    }
    let uch = rd32(QMP_MSGRAM + 20);
    let uack = rd32(QMP_MSGRAM + 24);
    if uch != uack {
        wr32(QMP_MSGRAM + 24, uch);
        aop_kick();
    }
    if rd32(QMP_MSGRAM + 44) != 0xffff {
        wr32(QMP_MSGRAM + 44, 0xffff);
        aop_kick();
        if !aop_wait(freq, QMP_MSGRAM + 48, 0xffff, 5000) {
            return 0xe4;
        }
    }
    // 本机通道打开之后 AOP 才把自己的通道拉到 CONNECTED。这时再补 ack。
    let mut uch = rd32(QMP_MSGRAM + 20);
    let mut uack = rd32(QMP_MSGRAM + 24);
    if uch != uack {
        wr32(QMP_MSGRAM + 24, uch);
        aop_kick();
        uch = rd32(QMP_MSGRAM + 20);
        uack = rd32(QMP_MSGRAM + 24);
    }
    let mch = rd32(QMP_MSGRAM + 44);
    let mack = rd32(QMP_MSGRAM + 48);
    unsafe {
        QMPW[1] = uch;
        QMPW[2] = uack;
        QMPW[3] = mch;
        QMPW[4] = mack;
    }
    if uch != 0xffff || uack != 0xffff || mch != 0xffff || mack != 0xffff {
        return 0xe6;
    }
    let slot = QMP_MSGRAM + 0x128;
    if rd32(slot) != 0 {
        return 0xe5;
    }
    if text.len() > 96 {
        return 0xe7;
    }
    let mut msg = [0u8; 96];
    msg[..text.len()].copy_from_slice(text);
    let mut i = 0;
    while i < 24 {
        let b = i * 4;
        let w = msg[b] as u32
            | (msg[b + 1] as u32) << 8
            | (msg[b + 2] as u32) << 16
            | (msg[b + 3] as u32) << 24;
        wr32(slot + 4 + i * 4, w);
        i += 1;
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32(slot, 96);
    let _ = rd32(slot);
    aop_kick();
    // QMP_TX_TOUT_MS。长度没清零就不要进 PAS。
    let end = time::cntvct() + freq as u64;
    let mut left = rd32(slot);
    while left != 0 && time::cntvct() < end {
        left = rd32(slot);
    }
    left
}

fn rpmh_usb_rails(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    // 上一张停在 s19 a1 之后、本函数第一条记录之前。先落环，再碰
    // cmd-db。整段 ivac 放在读之前，读挂死时环上看不出卡在哪。
    s20_log(ufs, freq, t, " s20 p0=0x", 1);
    let ver = rd32(CMD_DB) as u64;
    let magic = rd32(CMD_DB + 4) as u64;
    s20_log(ufs, freq, t, " s20 db=0x", (ver << 32) | magic);
    if magic as u32 != CMD_DB_MAGIC {
        s20_log(ufs, freq, t, " s20 p1=0x", 1);
        dcache_ivac(CMD_DB as u64, CMD_DB_BYTES as u64);
        let ver2 = rd32(CMD_DB) as u64;
        let magic2 = rd32(CMD_DB + 4) as u64;
        s20_log(ufs, freq, t, " s20 db2=0x", (ver2 << 32) | magic2);
    }
    let l5 = db_id(b"ldoa5\0\0\0");
    let l12 = db_id(b"ldoa12\0\0");
    let l2 = db_id(b"ldoa2\0\0\0");
    s20_log(ufs, freq, t, " s20 l5=0x", l5 as u64);
    s20_log(ufs, freq, t, " s20 l12=0x", l12 as u64);
    s20_log(ufs, freq, t, " s20 l2=0x", l2 as u64);
    if !vreg_addr(l5) && !vreg_addr(l12) && !vreg_addr(l2) {
        s20_log(ufs, freq, t, " s20 v0=0x", 0xe4);
        return;
    }
    let ctl = rd32(RSC_TCS0 + 0x14) as u64;
    let ena = rd32(RSC_TCS0 + 0x1c) as u64;
    s20_log(ufs, freq, t, " s20 tc=0x", (ctl << 32) | ena);
    if ctl == 0xffff_ffff || ena == 0xffff_ffff {
        s20_log(ufs, freq, t, " s20 v0=0x", 0xee);
        return;
    }
    // phy-msm-snps-hs.c enable_power：vdd 880mV，vdda18 1704mV，vdda33 3050mV。
    // 三路负载都过 pmic5 LDO 的 HPM 阈值（模式值 7）。
    // 状态字：每条命令 20 位（电压、使能、模式），任一超时 bit63=1。
    let v5 = if vreg_addr(l5) { rpmh_vrm(l5, 880, 7, freq) } else { 0xe4 };
    s20_log(ufs, freq, t, " s20 v5=0x", v5);
    let v12 = if vreg_addr(l12) { rpmh_vrm(l12, 1704, 7, freq) } else { 0xe4 };
    s20_log(ufs, freq, t, " s20 v12=0x", v12);
    let v2 = if vreg_addr(l2) { rpmh_vrm(l2, 3050, 7, freq) } else { 0xe4 };
    s20_log(ufs, freq, t, " s20 v2=0x", v2);
}

/// qpnp-pdphy.c pd_phy_open，角色用 pd_engine.c init_pd_phy_params：
/// UFP + SNK、USBPD_REV_20=1、FRAME_FILTER = SOP|HARD_RESET（0x21）、
/// EN_CONTROL 先 0 再 1、RX_ACKNOWLEDGE=0。vdd 是 pm8150_l2，调用方
/// 须已经投过 ldoa2。不装中断：没有处理函数。owner 不是 APPS 就不写。
fn pd_phy_open(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    let arb = spmi::Arb {
        core: CORE,
        chnls: CHNLS,
        obsrvr: OBSRVR,
        cnfg: CNFG,
    };
    let n = spmi::apid_count(arb.features());
    let Some(ap) = arb.find_apid_owner(0x217, 0, n) else {
        s20_log(ufs, freq, t, " s20 pd2=0x", 0xee);
        return;
    };
    let own = arb.owner(ap);
    if own != 0 {
        s20_log(ufs, freq, t, " s20 pd2=0x", 0xee00_0000 | own as u64);
        return;
    }
    let rd = |off: u8| -> Option<u8> {
        let mut b = [0u8; 1];
        arb.read(ap, off, &mut b).ok().map(|_| b[0])
    };
    let wr = |off: u8, val: u8| arb.write(ap, off, &[val]).is_ok();
    let Some(cfg0) = rd(0x40) else {
        s20_log(ufs, freq, t, " s20 pd2=0x", 0xef);
        return;
    };
    let cfg = (cfg0 & !0x0f) | 0x01;
    let ok = wr(0x40, cfg)
        && wr(0x46, 0)
        && wr(0x46, 1)
        && wr(0x4c, 0x21)
        && wr(0x4b, 0);
    let en = rd(0x46).unwrap_or(0xff) as u64;
    let msg = rd(0x40).unwrap_or(0xff) as u64;
    let filt = rd(0x4c).unwrap_or(0xff) as u64;
    let mut back = en | (msg << 8) | (filt << 16);
    if !ok {
        back |= 1 << 63;
    }
    s20_log(ufs, freq, t, " s20 pd2=0x", back);
    if !ok || en & 1 == 0 {
        return;
    }
    // policy_engine：SourceCap 到达后、还没发 Request，只要 PDO0 带
    // USB Communications Capable 就 start_usb_peripheral。GoodCRC 由
    // PHY 按 MSG_CONFIG 自己回。这里在 500ms 内把缓冲区读走并放回，
    // 对第一条 SourceCap 的 PDO1（vSafe5V）发 Request。
    let deadline = time::cntvct() + (freq as u64) / 2;
    let mut seen: u64 = 0;
    let mut nmsg: u64 = 0;
    while time::cntvct() < deadline && nmsg < 6 {
        let ack = rd(0x4b).unwrap_or(0);
        if ack == 0 {
            continue;
        }
        nmsg += 1;
        let mut buf = [0u8; 8];
        let _ = arb.read(ap, 0x80, &mut buf);
        let _ = wr(0x4b, 0);
        let hdr = u16::from_le_bytes([buf[0], buf[1]]);
        let pdo = u32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]);
        if seen == 0 {
            seen = (pdo as u64) << 16 | hdr as u64;
        }
        let nobj = (hdr >> 12) & 7;
        let typ = hdr & 0x1f;
        if nobj == 0 || typ != 1 || (pdo >> 30) != 0 {
            continue;
        }
        let ma10 = pdo & 0x3ff;
        let usb_comm = (pdo >> 26) & 1;
        let rdo = (1u32 << 28) | (usb_comm << 25) | (1 << 24) | (ma10 << 10) | ma10;
        if rd(0x4b).unwrap_or(1) != 0 {
            s20_log(ufs, freq, t, " s20 rx=0x", seen | (nmsg << 48));
            s20_log(ufs, freq, t, " s20 rq=0x", 0xee);
            return;
        }
        let hdr_tx = 0x1042u16.to_le_bytes();
        let sent = arb.write(ap, 0x60, &hdr_tx).is_ok()
            && arb.write(ap, 0x62, &rdo.to_le_bytes()).is_ok()
            && wr(0x42, 5)
            && wr(0x44, 0);
        let pause = time::cntvct() + (freq.max(1) as u64) / 500_000;
        while time::cntvct() < pause {}
        let sent = sent && wr(0x44, 0x61);
        s20_log(ufs, freq, t, " s20 rx=0x", seen | (nmsg << 48));
        s20_log(ufs, freq, t, " s20 rq=0x", if sent { rdo as u64 } else { 0xee });
        return;
    }
    s20_log(ufs, freq, t, " s20 rx=0x", seen | (nmsg << 48));
}

fn s20_start(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    // 先落环再碰新寄存器。上一刷死在本函数入口与第一条 s20 记录之间，
    // 看门狗 64s 后踢回 b 槽，环上只剩 s19 t=。
    let dctl = rd32(USB_DWC3 + 0xc704);
    s20_log(ufs, freq, t, " s20 go=0x", dctl as u64);
    if dctl & (1 << 30) != 0 {
        s20_log(ufs, freq, t, " s20 rst=0x", dctl as u64);
        return;
    }
    let phy = rd32(USB_DWC3 + 0xc200);
    s20_log(ufs, freq, t, " s20 phy=0x", phy as u64);
    // 上一刷停在 phy= 之后。值已是 USBTRDTIM=9 就不要重写这一下。
    let phy2 = (phy & !(0xf << 10)) | (9 << 10);
    if phy2 != phy {
        s20_log(ufs, freq, t, " s20 w=0x", phy2 as u64);
        wr32(USB_DWC3 + 0xc200, phy2);
    } else {
        s20_log(ufs, freq, t, " s20 sk=0x", phy as u64);
    }
    // 丢掉 s19 两秒窗里积的事件，lpos 跟上控制器写指针。
    let stale = (rd32(USB_DWC3 + 0xc40c) & 0xfffc).min(4096);
    unsafe {
        EP0.lpos = stale % 4096;
        EP0.nrdy = false;
    }
    if stale != 0 {
        wr32(USB_DWC3 + 0xc40c, stale);
    }
    wr32(USB_DWC3 + 0xc708, 1 | (1 << 1) | (1 << 2)); // DISCONNECT|RESET|CONNECT
    s20_log(ufs, freq, t, " s20 en=0x", rd32(USB_DWC3 + 0xc708) as u64);
    // 清 SUSPHY/ENBLSLPM。上一刷 n=0x2400 之后、写 0xc80c 时总线停摆。
    // 当时 DCTL=0xf00002：SFTDISCONNECT=1 且 RUN_STOP=0。命令接口在这个
    // 状态下写 CMDACT 会把 AXI 挂死，轮询根本执行不到。这里不再先拉断，
    // 先置 RUN_STOP，再只读 0xc80c。读都不返回就跳过命令，进常驻喂狗。
    let saved = rd32(USB_DWC3 + 0xc200);
    s20_log(ufs, freq, t, " s20 m=0x", saved as u64);
    let masked = saved & !((1 << 6) | (1 << 8));
    if masked != saved {
        wr32(USB_DWC3 + 0xc200, masked);
    }
    s20_log(ufs, freq, t, " s20 n=0x", rd32(USB_DWC3 + 0xc200) as u64);
    rmw32(USB_DWC3 + 0xc704, (1 << 31) | (1 << 1), 1 << 31);
    s20_log(ufs, freq, t, " s20 rs=0x", rd32(USB_DWC3 + 0xc704) as u64);
    let dep = rd32(USB_DWC3 + 0xc80c);
    s20_log(ufs, freq, t, " s20 r=0x", dep as u64);
    // 低 8 位对齐才发 CMDACT。对不上就只记 xn=，进常驻，不挂总线。
    let x0 = rd32(USB_HSPHY + 0x6c);
    let x1 = rd32(USB_HSPHY + 0x70);
    let x2 = rd32(USB_HSPHY + 0x74);
    // lito-redfin-usb.dtsi 只覆盖 X0/X1。X2 不写，不拿它当闸门。
    let xn_ok = x0 & 0xff == 0xc7 && x1 & 0xff == 0xcc;
    if xn_ok {
        match dep_cmd(0, 9, 0, 0, 0) {
            Some(0) => {
                for ep in [0u32, 1, 2, 3, 5] {
                    xfer_res(ufs, freq, t, ep);
                }
                let (p0, p1) = depcfg_control(0, 64);
                enable_ep(ufs, freq, t, 0, p0, p1);
                let (p0, p1) = depcfg_control(1, 64);
                enable_ep(ufs, freq, t, 1, p0, p1);
                s20_log(ufs, freq, t, " s20 ena=0x", rd32(USB_DWC3 + 0xc720) as u64);
                // 内核在 RUN_STOP 之前武装 SETUP。我们 RUN_STOP 已经是 1，
                // 先拉断再发 STARTTRANSFER，完成后再接上，避免链路半开时命令挂死。
                rmw32(USB_DWC3 + 0xc704, 1 << 1, 1 << 1);
                s20_log(ufs, freq, t, " s20 dc=0x", rd32(USB_DWC3 + 0xc704) as u64);
                // 直通被硬件改成故障（sb=0x20000）。改为一张 S1 页表：
                // 1GB 块恒等映射 0x80000000–0xBFFFFFFF，S2CR 指向空闲 CB。
                if smmu_usb_bypass(ufs, freq, t) {
                    arm_setup(ufs, freq, t);
                    s20_log(ufs, freq, t, " s20 arm=0x", rd32(USB_DWC3 + 0xc80c) as u64);
                    // dwc3-qcom.c：QSCRATCH HS_PHY_CTRL 把 VBUS 有效交给控制器。
                    // 只有 PHY 的 VBUSVLDEXT 时，DSTS 一直停在断开（USBLNKST=4）。
                    let hs = USB_DWC3 + 0xf8810;
                    rmw32(hs, (1 << 20) | (1 << 28), (1 << 20) | (1 << 28));
                    s20_log(ufs, freq, t, " s20 vb=0x", rd32(hs) as u64);
                    // APSD 结果是 CDP。dwc3_msm_vbus_notifier 在启动 gadget
                    // 之前调用 msm_hsphy_drive_dp_pulse(200ms)：拉上 FS 的 D+，
                    // 到时把 TERMSEL、XCVRSEL 和两路覆盖位都清掉，再等 20ms。
                    // 电源保持开着，后面还要枚举。
                    rmw32(USB_HSPHY + 0x94, (1 << 0) | (1 << 1), (1 << 0) | (1 << 1));
                    rmw32(USB_HSPHY + 0x3c, (0x3 << 3) | (1 << 5), 1 << 5);
                    rmw32(USB_HSPHY + 0x40, 1, 1);
                    let d1 = (rd32(USB_HSPHY + 0x3c) as u64 & 0xff)
                        | ((rd32(USB_HSPHY + 0x40) as u64 & 0xff) << 8)
                        | ((rd32(USB_HSPHY + 0x94) as u64 & 0xff) << 16);
                    s20_log(ufs, freq, t, " s20 d1=0x", d1);
                    let end = time::cntvct() + (freq as u64) / 5;
                    while time::cntvct() < end {}
                    rmw32(USB_HSPHY + 0x3c, 1 << 5, 0);
                    rmw32(USB_HSPHY + 0x40, 1, 0);
                    rmw32(USB_HSPHY + 0x94, (1 << 0) | (1 << 1), 0);
                    let end = time::cntvct() + (freq as u64) / 50;
                    while time::cntvct() < end {}
                    let d0 = (rd32(USB_HSPHY + 0x3c) as u64 & 0xff)
                        | ((rd32(USB_HSPHY + 0x40) as u64 & 0xff) << 8)
                        | ((rd32(USB_HSPHY + 0x94) as u64 & 0xff) << 16);
                    s20_log(ufs, freq, t, " s20 d0=0x", d0);
                    rmw32(USB_DWC3 + 0xc704, 1 << 1, 0);
                } else {
                    s20_log(ufs, freq, t, " s20 ns=0x", 1);
                }
            }
            Some(st) => s20_fail(ufs, freq, t, 0, 9, st),
            None => s20_fail(ufs, freq, t, 0, 9, 0xff),
        }
    } else {
        let packed = (x0 as u64 & 0xff) | ((x1 as u64 & 0xff) << 8) | ((x2 as u64 & 0xff) << 16);
        s20_log(ufs, freq, t, " s20 xn=0x", packed);
    }
    let end = time::cntvct() + freq as u64 / 20;
    while time::cntvct() < end {}
    let dsts = rd32(USB_DWC3 + 0xc70c) as u64;
    let dctl = rd32(USB_DWC3 + 0xc704) as u64;
    s20_log(ufs, freq, t, " s20 lk=0x", (dsts << 32) | dctl);
}

fn s20_event(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], ev: u32) {
    if ev & 1 == 1 {
        let kind = (ev >> 8) & 0xf;
        if kind == 1 {
            s20_log(ufs, freq, t, " s20 rst=0x", unsafe { EP0.nevt } as u64);
            s20_bus_reset(ufs, freq, t);
        } else if kind == 2 {
            let dsts = rd32(USB_DWC3 + 0xc70c) as u64;
            let dcfg = rd32(USB_DWC3 + 0xc700) as u64;
            s20_log(ufs, freq, t, " s20 cd=0x", (dsts << 32) | dcfg);
        }
        return;
    }
    // core.h dwc3_event_depevt：类型在 bits 9:6，状态在 bits 15:12。
    // 设备事件的类型才在 bits 11:8。上一轮复位之间被丢掉的就是这里。
    let ep = (ev >> 1) & 0x1f;
    let kind = (ev >> 6) & 0xf;
    if ep >= 2 {
        static mut EVN: u32 = 0;
        if unsafe { EVN } < 8 {
            unsafe { EVN += 1 };
            s20_keep(ufs, freq, t, " s20 ev=0x", ev as u64);
        }
    }
    let status = (ev >> 12) & 0xf;
    let phase = unsafe { EP0.phase };
    // gadget.c set_ep_config：非控制端点只开 XFER_IN_PROGRESS_EN（bit 9）。
    // 完成事件是 XferInProgress（类型 2）。类型 1 也认，ep0 仍走下面。
    let done = kind == 1 || kind == 2;
    if ep == 3 && done {
        let _slot = trb_done(ep);
        if unsafe { EP0.link } == 2 {
            s20_notify(ufs, freq, t, true);
            unsafe { EP0.link = 0 };
            s20_gratuitous(ufs, freq, t);
        }
        return;
    }
    if ep == 2 && done {
        let idx = trb_done(ep);
        s20_on_out(ufs, freq, t, idx);
        return;
    }
    if ep == 5 && done {
        let _slot = trb_done(ep);
        unsafe { IN_BUSY = false };
        return;
    }
    if kind == 1 {
        match phase {
            PH_SETUP => s20_setup(ufs, freq, t),
            PH_DATA => {
                if unsafe { EP0.nrdy } {
                    let ep = unsafe { EP0.nrdy_ep };
                    s20_arm_status(ufs, freq, t, ep);
                } else {
                    unsafe { EP0.phase = PH_WAIT };
                }
            }
            PH_STATUS => {
                arm_setup(ufs, freq, t);
            }
            _ => {}
        }
    } else if kind == 3 && status == 2 {
        // NRDY(STATUS)。DATA 还没完成就先到的，记下，完成时再武装。
        if phase == PH_WAIT {
            s20_arm_status(ufs, freq, t, ep);
        } else if phase == PH_DATA {
            unsafe {
                EP0.nrdy = true;
                EP0.nrdy_ep = ep;
            }
        }
    }
}

struct CtrlRx(usize);
struct CtrlTx;

impl smoltcp::phy::RxToken for CtrlRx {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let n = self.0.min(1536);
        f(unsafe { &RX_COPY[..n] })
    }
}

impl smoltcp::phy::TxToken for CtrlTx {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let n = len.min(1536);
        let r = unsafe { f(&mut IN_BUF[..n]) };
        let ufs = unsafe { &mut *CTX_UFS };
        let buf = unsafe { &mut *CTX_T };
        s20_arm_in(ufs, unsafe { CTX_FREQ }, buf, n as u32);
        r
    }
}

struct CtrlPhy;

impl smoltcp::phy::Device for CtrlPhy {
    type RxToken<'a> = CtrlRx;
    type TxToken<'a> = CtrlTx;

    fn capabilities(&self) -> smoltcp::phy::DeviceCapabilities {
        let mut caps = smoltcp::phy::DeviceCapabilities::default();
        // 一次只能挂一条 IN。MTU 含以太网头，不含 FCS。
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(1);
        caps
    }

    fn receive(
        &mut self,
        _timestamp: smoltcp::time::Instant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let n = unsafe { RX_LEN };
        if n < 14 || unsafe { IN_BUSY } {
            return None;
        }
        unsafe { RX_LEN = 0 };
        Some((CtrlRx(n), CtrlTx))
    }

    fn transmit(&mut self, _timestamp: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        if unsafe { IN_BUSY } {
            None
        } else {
            Some(CtrlTx)
        }
    }
}

fn tcp_mark(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], label: &str, v: u64) {
    if unsafe { TCP_NOTES } >= 16 {
        return;
    }
    unsafe { TCP_NOTES += 1 };
    let txt = hex_text(t, unsafe { RING.seq }, label, v);
    ring_put(ufs, freq, logring::LVL_INFO, txt);
}

fn tcp_listen_rc(r: Result<(), smoltcp::socket::tcp::ListenError>) -> u8 {
    match r {
        Ok(()) => 0,
        Err(smoltcp::socket::tcp::ListenError::InvalidState) => 1,
        Err(smoltcp::socket::tcp::ListenError::Unaddressable) => 2,
    }
}

/// 0 closed，1 listen，2 syn-received，3 established，10 syn-sent，其后是关闭过程。
fn tcp_state_code(state: smoltcp::socket::tcp::State) -> u8 {
    use smoltcp::socket::tcp::State::*;
    match state {
        Closed => 0,
        Listen => 1,
        SynReceived => 2,
        Established => 3,
        FinWait1 => 4,
        FinWait2 => 5,
        CloseWait => 6,
        Closing => 7,
        LastAck => 8,
        TimeWait => 9,
        SynSent => 10,
    }
}

/// 待交给 smoltcp 的帧如果是发往 2323 的 IPv4 TCP，返回标志字节。
fn tcp_rx_flags() -> Option<u8> {
    unsafe {
        let n = RX_LEN;
        if n < 40 || n > RX_COPY.len() {
            return None;
        }
        let f = &RX_COPY[..n];
        if f[12] != 0x08 || f[13] != 0x00 || f[14] >> 4 != 4 {
            return None;
        }
        let ihl = ((f[14] & 0x0f) as usize) * 4;
        if ihl < 20 || 14 + ihl + 14 > n || f[23] != 6 {
            return None;
        }
        let dport = ((f[14 + ihl + 2] as u16) << 8) | (f[14 + ihl + 3] as u16);
        if dport != CTRL_PORT {
            return None;
        }
        Some(f[14 + ihl + 13])
    }
}

fn ctrl_boot(freq: u32) -> u8 {
    use smoltcp::iface::{Interface, SocketSet};
    use smoltcp::socket::tcp;
    use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr};

    let mut phy = CtrlPhy;
    let mut config = smoltcp::iface::Config::new(HardwareAddress::Ethernet(EthernetAddress(OUR_MAC)));
    config.random_seed = time::cntvct();
    let mut iface = Interface::new(
        config,
        &mut phy,
        smoltcp::time::Instant::from_millis(0),
    );
    iface.update_ip_addrs(|ip| {
        ip.push(IpCidr::new(IpAddress::v4(169, 254, 1, 1), 16)).ok();
    });
    let rx_mem = unsafe { &mut *core::ptr::addr_of_mut!(TCP_RX) };
    let tx_mem = unsafe { &mut *core::ptr::addr_of_mut!(TCP_TX) };
    let rx = tcp::SocketBuffer::new(&mut rx_mem[..]);
    let tx = tcp::SocketBuffer::new(&mut tx_mem[..]);
    let mut sock = tcp::Socket::new(rx, tx);
    let rc = tcp_listen_rc(sock.listen(CTRL_PORT));
    let slots = unsafe { &mut *core::ptr::addr_of_mut!(CTRL_SLOTS) };
    let mut set = SocketSet::new(&mut slots[..]);
    let handle = set.add(sock);
    unsafe {
        CTRL_HANDLE = Some(handle);
        CTRL_IFACE = Some(iface);
        CTRL_UP = true;
    }
    let _ = freq;
    rc
}

fn pon_rmw(arb: &spmi::Arb, apid: u32, addr: u8, mask: u8, val: u8) -> bool {
    let mut b = [0u8; 1];
    if arb.read(apid, addr, &mut b).is_err() {
        return false;
    }
    b[0] = (b[0] & !mask) | (val & mask);
    arb.write(apid, addr, &b).is_ok()
}

/// 只读检查。pm8150 SID0 PON@0x800 是 system-reset，且开了
/// store-hard-reset-reason。gen2 的原因字左移 1 位写 0x8F。
fn fb_check(arb: &spmi::Arb, n_apid: u32) -> Result<(), u64> {
    let Some(ap) = arb.find_apid_owner(PON_PPID, 0, n_apid) else {
        return Err(0xE1);
    };
    if arb.owner(ap) != 0 {
        return Err(0xE2);
    }
    let mut sub = [0u8; 1];
    if arb.read(ap, 0x05, &mut sub).is_err() {
        return Err(0xE3);
    }
    let gen2 = sub[0] == 0x04 || sub[0] == 0x05;
    let gen1 = sub[0] == 0x01 || sub[0] == 0x02 || sub[0] == 0x03;
    if !gen1 && !gen2 {
        return Err(0xE400 | sub[0] as u64);
    }
    unsafe {
        FB_APID = ap;
        FB_GEN2 = gen2;
    }
    Ok(())
}

/// msm-poweroff.c `bootloader`：原因 0x02，IMEM 0x77665500，
/// PS_HOLD 配成热复位后再写 0。写完不返回。
fn fb_go(arb: &spmi::Arb, freq: u32) -> bool {
    let apid = unsafe { FB_APID };
    let gen2 = unsafe { FB_GEN2 };
    let (mask, val) = if gen2 { (0xFEu8, 0x04u8) } else { (0xFCu8, 0x08u8) };
    if !pon_rmw(arb, apid, 0x8F, mask, val) {
        return false;
    }
    wr32(IMEM_RESTART, 0x7766_5500);
    let mut rev = [0u8; 1];
    let _ = arb.read(apid, 0x01, &mut rev);
    let rst_en = if !gen2 && rev[0] == 0 { 0x5Au8 } else { 0x5Bu8 };
    if !pon_rmw(arb, apid, rst_en, 0x80, 0) {
        return false;
    }
    let wait = time::cntvct() + freq as u64 / 2000;
    while time::cntvct() < wait {}
    if !pon_rmw(arb, apid, 0x5A, 0x0F, 0x01) {
        return false;
    }
    if !pon_rmw(arb, apid, rst_en, 0x80, 0x80) {
        return false;
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32(MSM_PS_HOLD, 0);
    loop {
        core::hint::spin_loop();
    }
}

fn fb_tick(arb: &spmi::Arb, freq: u32, ufs: &mut Ufs, t: &mut [u8; 96]) {
    let st = unsafe { CTRL_FB };
    if st == 0 || freq == 0 {
        return;
    }
    let now = time::cntvct();
    if st == 1 {
        if unsafe { IN_BUSY } {
            return;
        }
        unsafe {
            CTRL_FB = 2;
            FB_AT = now + freq as u64 / 50;
        }
        return;
    }
    if now < unsafe { FB_AT } {
        return;
    }
    unsafe { CTRL_FB = 0 };
    if !fb_go(arb, freq) {
        s20_keep(ufs, freq, t, " s20 fb=0x", 0xE5);
    }
}

/// 最近四条环记录，旧的在前。读失败不复位。
fn ring_tail(ufs: &mut Ufs, dst: &mut [u8]) -> usize {
    let (base, slot) = unsafe { (RING.base, RING.slot) };
    if base == 0 {
        return 0;
    }
    let mut n = 0usize;
    for back in (0..4u32).rev() {
        let idx = (slot + RING_BLOCKS - 1 - back) % RING_BLOCKS;
        if ufs.read10(base + idx, 1, DATA).is_err() {
            continue;
        }
        let blk = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
        if logring::parse(blk).is_none() {
            continue;
        }
        let mut i = 32usize;
        while i < 112 && blk[i] != 0 && n + 1 < dst.len() {
            dst[n] = blk[i];
            n += 1;
            i += 1;
        }
        if n < dst.len() {
            dst[n] = b'\n';
            n += 1;
        }
    }
    n
}

/// 电量计快照，寄存器序与 soak 的 f 字相同。读失败不复位。
/// modem.mdt，9420 字节。在 .local，不进 git。
const MODEM_MDT: &[u8] = include_bytes!(
    "/Users/sophiehe/Documents/aginxos-next/.local/device/redfin/modem/modem.mdt"
);

#[repr(C, align(4096))]
struct PasMeta {
    raw: [u8; 12288],
}
static mut PAS_META: PasMeta = PasMeta { raw: [0; 12288] };

/// modem 镜像止于 0x9D800000，cdsp_sec 从 0x9F400000 起。
/// 窗口长度等于 lito.dtsi 里 qcom,sharedmem-uio 的 0x280000。
/// 后面四段各 0x280000 是主机侧的 fs1/fs2/fsc/fsg，modem 看不见。
const RFS_ADDR: u64 = 0x9E00_0000;
const RFS_SIZE: u64 = 0x28_0000;
/// 对账 Linux：modemst1/2/fsg 均为 2621440；IOVEC 可读 offset=1024 size=2620416。
const SH_SIZE: u64 = 0x28_0000;
const SH0: u64 = RFS_ADDR + RFS_SIZE;

#[repr(C, align(4096))]
struct RfsScm([u8; 256]);
static mut RFS_SCM: RfsScm = RfsScm([0; 256]);

fn scm_put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at] = v as u8;
    b[at + 1] = (v >> 8) as u8;
    b[at + 2] = (v >> 16) as u8;
    b[at + 3] = (v >> 24) as u8;
}

fn scm_put_u64(b: &mut [u8], at: usize, v: u64) {
    scm_put_u32(b, at, v as u32);
    scm_put_u32(b, at + 4, (v >> 32) as u32);
}

/// msm_sharedmem.c：源 VMID_HLOS，目的 HLOS 和 MSS_MSA，都是 RW。
/// SCM_SIP_FNID(SCM_SVC_MP=0xC, MEM_PROT_ASSIGN_ID=0x16)。
/// 参数超过 4 个时 x5 是额外参数缓冲的物理地址，从 args[3] 起。
fn rfs_assign() -> bool {
    if unsafe { RFS_READY } == 1 {
        return true;
    }
    if unsafe { RFS_READY } == 2 {
        return false;
    }
    let page = unsafe { &mut RFS_SCM.0 };
    page.fill(0);
    scm_put_u64(page, 0, RFS_ADDR);
    scm_put_u64(page, 8, RFS_SIZE);
    scm_put_u32(page, 16, 3);
    let mut d = 32usize;
    for vm in [3u32, 0xF] {
        scm_put_u32(page, d, vm);
        scm_put_u32(page, d + 4, 6);
        d += 24;
    }
    let base = core::ptr::addr_of!(RFS_SCM) as u64;
    let extra = base + 80;
    scm_put_u64(page, 80, 4);
    scm_put_u64(page, 88, base + 32);
    scm_put_u64(page, 96, 48);
    dcache_civac(base, 256);
    let fnid = 0x0200_0C16u64 | 0x4000_0000;
    let (r0, r1) = psci::smc_ret2(fnid, 0x1117, base, 16, base + 16, extra);
    unsafe { RF_AS = r0 as u32 };
    if r0 == 0 && r1 == 0 {
        let mut a = RFS_ADDR;
        let end = RFS_ADDR + RFS_SIZE;
        while a < end {
            wr32(a as usize, 0);
            a += 4;
        }
        dcache_civac(RFS_ADDR, RFS_SIZE);
        unsafe { RFS_READY = 1 };
        true
    } else {
        unsafe { RFS_READY = 2 };
        false
    }
}

fn rfs_u32(at: u64) -> u32 {
    rd8(at) as u32
        | (rd8(at + 1) as u32) << 8
        | (rd8(at + 2) as u32) << 16
        | (rd8(at + 3) as u32) << 24
}

fn rfs_copy(dst: u64, src: u64, n: u64) {
    let mut i = 0u64;
    while i + 8 <= n {
        let v = unsafe { ((src + i) as *const u64).read_volatile() };
        unsafe { ((dst + i) as *mut u64).write_volatile(v) };
        i += 8;
    }
}

/// 第一笔读写之前把这块影子清成空白 EFS。modem 自己会写格式。
/// 若开机已从 GPT 灌过（RFS_SEED），不再清零——保留真 EFS/fsg。
fn rfs_shadow_ready(caller: u32) {
    let bit = 1u8 << caller;
    if unsafe { RFS_Z } & bit != 0 {
        return;
    }
    let mut a = SH0 + (caller as u64) * SH_SIZE;
    let end = a + SH_SIZE;
    while a < end {
        wr32(a as usize, 0);
        a += 4;
    }
    unsafe { RFS_Z |= bit };
}

/// 从 GPT 分区灌 4 块 rmtfs 影子（对账 radio README：
/// modemst1/2、fsc、fsg ↔ fs1/fs2/fsc/fsg）。每槽最多 0x280000（对账 modemst 分区）。
/// 这是冲 0x45 的 EFS 前置：空白影子 → modem DOG → 永不 spawn wlan_pd。
fn rfs_seed_efs(ufs: &mut Ufs) {
    // modemst/fsg 在 sdf（对账 radio README），常见 UFS LUN 5；
    // sda=LUN0 的 GPT 只有 modem_a/b。逐 LUN 读 GPT 名查。
    const MAP: &[(&[&str], u32)] = &[
        (&["modemst1", "modem_st1"], 0),
        (&["modemst2", "modem_st2"], 1),
        (&["fsc"], 2),
        (&["fsg"], 3),
    ];
    unsafe {
        RFS_GNN = 0;
        RFS_GN = [0; 64];
    }
    // 临时 GPT：头进 DATA，条目进 ARRAY（会覆盖 LUN0 缓存；名查完即用完）
    let mut lun: u8 = 0;
    while lun < 8 {
        if ufs.read10_lun(lun, 1, 1, DATA).is_err() {
            lun = lun.wrapping_add(1);
            continue;
        }
        let hdr = unsafe { core::slice::from_raw_parts_mut(DATA as *mut u8, 4096) };
        let Ok(h) = gpt::check(hdr) else {
            lun = lun.wrapping_add(1);
            continue;
        };
        if !gpt::header_crc_ok(hdr, &h) {
            lun = lun.wrapping_add(1);
            continue;
        }
        if ufs.read10_lun(lun, h.entry_lba, 1, ARRAY).is_err() {
            lun = lun.wrapping_add(1);
            continue;
        }
        let array = unsafe { core::slice::from_raw_parts(ARRAY as *const u8, 4096) };
        if !gpt::array_crc_ok(hdr, array, &h) {
            lun = lun.wrapping_add(1);
            continue;
        }
        let entries = h.array_bytes >> 7;
        // 记录本 LUN 上命中的候选名
        for &(names, _slot) in MAP {
            for name in names {
                if gpt::find(array, entries, name).is_some() {
                    unsafe {
                        let mut p = RFS_GNN as usize;
                        if p > 0 && p < 60 {
                            RFS_GN[p] = b',';
                            p += 1;
                        }
                        // 前缀 L 数字
                        if p + 2 < 60 {
                            RFS_GN[p] = b'L';
                            RFS_GN[p + 1] = b'0' + lun;
                            RFS_GN[p + 2] = b':';
                            p += 3;
                        }
                        for &c in name.as_bytes() {
                            if p < 63 {
                                RFS_GN[p] = c;
                                p += 1;
                            }
                        }
                        RFS_GNN = p as u8;
                    }
                }
            }
        }
        for &(names, slot) in MAP {
            if unsafe { RFS_SEED } & (1 << slot) != 0 {
                continue;
            }
            let mut part = None;
            for name in names {
                if let Some(p) = gpt::find(array, entries, name) {
                    part = Some(p);
                    break;
                }
            }
            let Some(p) = part else {
                continue;
            };
            if p.start > u32::MAX as u64 || p.end < p.start {
                continue;
            }
            let start = p.start as u32;
            let nblk = ((p.end - p.start + 1) as u64).min(SH_SIZE / 4096) as u32;
            if nblk == 0 {
                continue;
            }
            let dest = SH0 + (slot as u64) * SH_SIZE;
            let mut i = 0u32;
            let mut ok = true;
            while i < nblk {
                if ufs.read10_lun(lun, start + i, 1, DATA).is_err() {
                    ok = false;
                    break;
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        DATA as *const u8,
                        (dest + (i as u64) * 4096) as *mut u8,
                        4096,
                    );
                }
                i += 1;
            }
            if ok {
                dcache_civac(dest, (nblk as u64) * 4096);
                unsafe {
                    RFS_Z |= 1 << slot;
                    RFS_SEED |= 1 << slot;
                }
            }
        }
        lun = lun.wrapping_add(1);
    }
    let _ = MAP;
}


fn rfs_one(caller: u32, dir: u8, sector: u32, phys: u32, nsec: u32) -> bool {
    if caller == 0 || caller > 4 || nsec == 0 {
        return caller >= 1 && caller <= 4;
    }
    let slot = caller - 1;
    if slot == 3 && dir != 0 {
        return false;
    }
    let bytes = (nsec as u64) * 512;
    let file_at = (sector as u64) * 512;
    if file_at.saturating_add(bytes) > SH_SIZE {
        return false;
    }
    let win = if unsafe { RFS_IOFF } != 0 {
        phys as u64
    } else if (phys as u64) < RFS_ADDR {
        return false;
    } else {
        (phys as u64) - RFS_ADDR
    };
    if win.saturating_add(bytes) > RFS_SIZE {
        return false;
    }
    rfs_shadow_ready(slot);
    let sh = SH0 + (slot as u64) * SH_SIZE + file_at;
    let mem = RFS_ADDR + win;
    if dir == 0 {
        rfs_copy(mem, sh, bytes);
        dcache_civac(mem, bytes);
    } else {
        dcache_ivac(mem, bytes);
        rfs_copy(sh, mem, bytes);
    }
    true
}

fn rfs_iovec(fifo: u64, off: u64, chunk: u64, caller: u32) -> bool {
    let w1 = rd32((fifo + off + 52) as usize);
    let msglen = ((w1 >> 8) & 0xffff) as u64;
    if chunk < 40 || msglen < 3 || 7 + msglen > chunk - 32 || msglen > 512 {
        return false;
    }
    let base = fifo + off + 55;
    let mut p = 0u64;
    let mut got_dir = false;
    let mut dir = 0u8;
    let mut saw = false;
    let mut ok = true;
    while p + 3 <= msglen {
        let t = rd8(base + p);
        let ln = rd8(base + p + 1) as u64 | ((rd8(base + p + 2) as u64) << 8);
        p += 3;
        if p + ln > msglen {
            return false;
        }
        if t == 2 && ln >= 1 {
            dir = rd8(base + p);
            got_dir = true;
        } else if t == 3 && ln >= 1 {
            let count = rd8(base + p) as u64;
            if count > 8 || 1 + count * 12 > ln {
                return false;
            }
            let mut e = 0u64;
            while e < count {
                let at = base + p + 1 + e * 12;
                if !rfs_one(caller, dir, rfs_u32(at), rfs_u32(at + 4), rfs_u32(at + 8)) {
                    ok = false;
                    break;
                }
                saw = true;
                e += 1;
            }
        }
        p += ln;
    }
    got_dir && saw && ok
}

fn le_u32(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | (b[o + 1] as u32) << 8 | (b[o + 2] as u32) << 16 | (b[o + 3] as u32) << 24
}

fn le_u64(b: &[u8], o: usize) -> u64 {
    le_u32(b, o) as u64 | (le_u32(b, o + 4) as u64) << 32
}

fn name_is(buf: &[u8], s: &[u8]) -> bool {
    buf.len() >= s.len() && &buf[..s.len()] == s && buf.get(s.len()).copied().unwrap_or(1) == 0
}

/// vendor 的 ext4 起点，4K LBA。GPT 上没有就进 super 的 liblp。
/// 几何头在 super+4096，主 metadata 在 +12288，槽距 metadata_max_size。
fn vendor_lba(ufs: &mut Ufs) -> Result<u32, u32> {
    let array = unsafe { core::slice::from_raw_parts(ARRAY as *const u8, 4096) };
    if let Some(p) = gpt::find(array, 32, "vendor_a").or_else(|| gpt::find(array, 32, "vendor")) {
        if (p.start | p.end) >> 32 != 0 {
            return Err(0x13);
        }
        return Ok(p.start as u32);
    }
    let super_p = gpt::find(array, 32, "super").ok_or(0x60u32)?;
    if super_p.start >> 32 != 0 {
        return Err(0x61);
    }
    let base = super_p.start as u32;
    ufs.read10(base + 1, 1, DATA).map_err(|_| 0x62u32)?;
    let geo = unsafe { core::slice::from_raw_parts(DATA as *const u8, 64) };
    if le_u32(geo, 0) != 0x616c_4467 {
        return Err(0x63);
    }
    let max_sz = le_u32(geo, 40);
    let slots = le_u32(geo, 44).min(2);
    let block = le_u32(geo, 48);
    if max_sz == 0 || max_sz > 65536 || (block != 512 && block != 4096) {
        return Err(0x64);
    }
    for slot in 0..slots {
        let byte = 12288u64 + slot as u64 * max_sz as u64;
        let mut got = 0usize;
        while got < max_sz as usize {
            let lba = base + ((byte + got as u64) >> 12) as u32;
            ufs.read10(lba, 1, DATA).map_err(|_| 0x65u32)?;
            let bo = ((byte + got as u64) & 0xFFF) as usize;
            let take = (4096 - bo).min(max_sz as usize - got);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (DATA as *const u8).add(bo),
                    E2BUF.as_mut_ptr().add(got),
                    take,
                );
            }
            got += take;
        }
        let meta = unsafe { &E2BUF[..got] };
        if le_u32(meta, 0) != 0x414c_5030 {
            continue;
        }
        let header_size = le_u32(meta, 8) as usize;
        if header_size < 128 || header_size >= got {
            return Err(0x67);
        }
        let part_off = le_u32(meta, 80) as usize;
        let part_n = le_u32(meta, 84) as usize;
        let part_esz = le_u32(meta, 88) as usize;
        let ext_off = le_u32(meta, 92) as usize;
        let ext_n = le_u32(meta, 96) as usize;
        let ext_esz = le_u32(meta, 100) as usize;
        if part_esz < 52 || ext_esz < 24 || part_n > 64 || ext_n > 128 {
            return Err(0x68);
        }
        let mut fallback = None;
        for i in 0..part_n {
            let e = header_size + part_off + i * part_esz;
            if e + 52 > got {
                return Err(0x69);
            }
            let exact = name_is(&meta[e..e + 36], b"vendor_a");
            let short = name_is(&meta[e..e + 36], b"vendor");
            if !exact && !short {
                continue;
            }
            let first = le_u32(meta, e + 40) as usize;
            let num = le_u32(meta, e + 44);
            if num != 1 || first >= ext_n {
                return Err(0x6a);
            }
            let ee = header_size + ext_off + first * ext_esz;
            if ee + 24 > got {
                return Err(0x6b);
            }
            if le_u32(meta, ee + 8) != 0 {
                return Err(0x6c);
            }
            let sectors = le_u64(meta, ee + 12);
            let src = le_u32(meta, ee + 20);
            let bd_off = le_u32(meta, 116) as usize;
            let bd_n = le_u32(meta, 120) as usize;
            let bd_esz = le_u32(meta, 124) as usize;
            let mut extra = 0u64;
            if (src as usize) < bd_n && bd_esz >= 8 {
                let be = header_size + bd_off + src as usize * bd_esz;
                if be + 8 <= got {
                    extra = le_u64(meta, be);
                }
            }
            let scales = [512u64, 512, block as u64];
            let adds = [0u64, extra, 0];
            let mut cands = [0u32; 3];
            let mut nc = 0usize;
            for i in 0..3 {
                let Some(sum) = sectors.checked_add(adds[i]) else { continue };
                let Some(bytes) = sum.checked_mul(scales[i]) else { continue };
                if bytes & 0xFFF != 0 {
                    continue;
                }
                let lba = base as u64 + (bytes >> 12);
                if lba > u32::MAX as u64 {
                    continue;
                }
                let lba = lba as u32;
                if cands[..nc].contains(&lba) {
                    continue;
                }
                cands[nc] = lba;
                nc += 1;
            }
            if nc == 0 {
                return Err(0x6e);
            }
            // 候选已是数字。探测只读 DATA。
            let saved = cands;
            let nsave = nc;
            let prefer = exact;
            let mut erofs = None;
            for i in 0..nsave {
                let kind = fs_at(ufs, saved[i]);
                if kind == 1 {
                    if prefer {
                        return Ok(saved[i]);
                    }
                    fallback = Some(saved[i]);
                    break;
                }
                if kind == 2 && erofs.is_none() {
                    erofs = Some(saved[i]);
                }
            }
            if prefer {
                if let Some(lba) = erofs {
                    unsafe {
                        ADSP_P[0] = lba;
                        ADSP_P[1] = 0xE0F5_E1E2;
                    }
                    return Err(0x71);
                }
                let _ = ufs.read10(saved[0], 1, DATA);
                let b = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
                unsafe {
                    ADSP_P[0] = saved[0];
                    ADSP_P[1] = le_u32(b, 1024);
                    ADSP_P[2] = b[1080] as u32 | (b[1081] as u32) << 8;
                }
                return Err(0x72);
            }
        }
        if let Some(lba) = fallback {
            return Ok(lba);
        }
    }
    Err(0x70)
}

/// 1=ext4（偏移 1080 半字 0xEF53），2=erofs（偏移 1024 的 0xE0F5E1E2）。
fn fs_at(ufs: &mut Ufs, lba: u32) -> u32 {
    if ufs.read10(lba, 1, DATA).is_err() {
        return 0;
    }
    let b = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
    let ext4m = b[1080] as u32 | (b[1081] as u32) << 8;
    if ext4m == 0xef53 {
        return 1;
    }
    if le_u32(b, 1024) == 0xE0F5_E1E2 {
        return 2;
    }
    0
}

fn elf_u32(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | (b[o + 1] as u32) << 8 | (b[o + 2] as u32) << 16 | (b[o + 3] as u32) << 24
}

/// qcom,lpass：pas-id 1，固件名 adsp，内存 0x89200000 / 0x2800000。
/// 代理电源是 ARC `lcx.lvl`、`lmx.lvl`，档位和 cx 一样用 TURBO 下标 6。
fn load_adsp(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) -> Result<u16, u32> {
    let lcx = db_id(b"lcx.lvl\0");
    let lmx = db_id(b"lmx.lvl\0");
    let scx = if lcx != 0 { rpmh_cmd(lcx, 6, freq) } else { 0 };
    let smx = if lmx != 0 { rpmh_cmd(lmx, 6, freq) } else { 0 };
    let txt = hex_text(t, unsafe { RING.seq }, " adsp lcx=0x", (lcx as u64) << 32 | scx as u64);
    ring_put(ufs, freq, logring::LVL_INFO, txt);
    let txt = hex_text(t, unsafe { RING.seq }, " adsp lmx=0x", (lmx as u64) << 32 | smx as u64);
    ring_put(ufs, freq, logring::LVL_INFO, txt);
    if lcx == 0 || lmx == 0 || scx != 0x10101 || smx != 0x10101 {
        return Err(0x10);
    }
    unsafe {
        ADSP_P[3] = lcx;
        ADSP_P[4] = lmx;
    }
    let base = vendor_lba(ufs)?;
    unsafe { ADSP_P[0] = base };
    ufs.read10(base, 1, DATA).map_err(|_| 0x14u32)?;
    let mag = unsafe { le_u32(core::slice::from_raw_parts(DATA as *const u8, 4096), 1024) };
    let ext4m = unsafe {
        let b = core::slice::from_raw_parts(DATA as *const u8, 4096);
        b[1080] as u32 | (b[1081] as u32) << 8
    };
    unsafe {
        ADSP_P[1] = mag;
        ADSP_P[2] = ext4m;
    }
    let dev = UfsDev { ufs, base };
    let mut fs = ext4::Fs::open(dev, unsafe { &mut *core::ptr::addr_of_mut!(SC1) }).map_err(|e| {
        0x1400u32 | match e {
            ext4::Bad::Io => 1,
            ext4::Bad::Magic => 2,
            ext4::Bad::BlockSize => 3,
            ext4::Bad::InodeSize => 4,
            ext4::Bad::NoExtent => 5,
            ext4::Bad::BadExtent => 6,
            ext4::Bad::TooDeep => 7,
            ext4::Bad::NoSuchInode => 8,
            ext4::Bad::NotDir => 9,
            ext4::Bad::BadDirent => 10,
            ext4::Bad::NotFound => 11,
        }
    })?;
    const MDTS: [&str; 3] = [
        "/firmware/adsp.mdt",
        "/firmware/image/adsp.mdt",
        "/etc/firmware/adsp.mdt",
    ];
    let mut which = 0xffu32;
    let mut ino = 0u32;
    for (i, path) in MDTS.iter().enumerate() {
        match fs.resolve(path, unsafe { &mut *core::ptr::addr_of_mut!(SC2) }) {
            Ok(n) => {
                which = i as u32;
                ino = n;
                break;
            }
            Err(_) => {}
        }
    }
    if which == 0xff {
        return Err(0x15);
    }
    let mut got = 0usize;
    loop {
        if got >= 12288 {
            return Err(0x16);
        }
        let n = {
            let dst = unsafe { &mut PAS_META.raw[got..] };
            let sc = unsafe { &mut *core::ptr::addr_of_mut!(SC2) };
            fs.read_file(ino, got as u64, dst, sc).map_err(|_| 0x17u32)?
        };
        if n == 0 {
            break;
        }
        got += n;
    }
    if got < 52 {
        return Err(0x18);
    }
    let mdt = unsafe { &PAS_META.raw[..got] };
    if mdt[0] != 0x7f || mdt[1] != b'E' || mdt[2] != b'L' || mdt[3] != b'F' || mdt[4] != 1 {
        return Err(0x19);
    }
    let phoff = elf_u32(mdt, 28) as usize;
    let phentsize = u16::from_le_bytes([mdt[42], mdt[43]]) as usize;
    let phnum = u16::from_le_bytes([mdt[44], mdt[45]]) as usize;
    if phentsize < 32 || phnum == 0 || phnum > 64 || phoff + phnum * phentsize > got {
        return Err(0x1a);
    }
    let mut min_pa = u32::MAX;
    let mut max_pa = 0u32;
    let mut nload = 0u16;
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let ty = elf_u32(mdt, o);
        let flags = elf_u32(mdt, o + 24);
        let memsz = elf_u32(mdt, o + 20);
        let pa = elf_u32(mdt, o + 12);
        if ty != 1 || memsz == 0 || flags & (7 << 24) == (2 << 24) {
            continue;
        }
        min_pa = min_pa.min(pa);
        let end = pa.saturating_add(memsz);
        if end > max_pa {
            max_pa = end;
        }
        nload += 1;
    }
    if nload == 0 || min_pa == u32::MAX {
        return Err(0x1b);
    }
    let reloc = min_pa < 0x8920_0000 || max_pa > 0x8920_0000 + 0x0280_0000;
    unsafe {
        core::ptr::write_bytes(0x8920_0000 as *mut u8, 0, 0x0280_0000);
    }
    let prefix: &str = match which {
        0 => "/firmware/adsp.b",
        1 => "/firmware/image/adsp.b",
        _ => "/etc/firmware/adsp.b",
    };
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let ty = elf_u32(mdt, o);
        let flags = elf_u32(mdt, o + 24);
        let filesz = elf_u32(mdt, o + 16) as usize;
        let pa = elf_u32(mdt, o + 12);
        if ty != 1 || elf_u32(mdt, o + 20) == 0 || flags & (7 << 24) == (2 << 24) {
            continue;
        }
        if filesz == 0 {
            continue;
        }
        let dest = if reloc {
            0x8920_0000u64 + (pa - min_pa) as u64
        } else {
            pa as u64
        };
        if dest < 0x8920_0000 || dest + filesz as u64 > 0x8920_0000 + 0x0280_0000 {
            return Err(0x20 + i as u32);
        }
        let mut name = [0u8; 40];
        let pb = prefix.as_bytes();
        name[..pb.len()].copy_from_slice(pb);
        name[pb.len()] = b'0' + (i / 10) as u8;
        name[pb.len() + 1] = b'0' + (i % 10) as u8;
        let name = core::str::from_utf8(&name[..pb.len() + 2]).unwrap_or("");
        let sino = fs
            .resolve(name, unsafe { &mut *core::ptr::addr_of_mut!(SC2) })
            .map_err(|_| 0x30 + i as u32)?;
        let mut off = 0usize;
        while off < filesz {
            let n = {
                let chunk = filesz - off;
                let take = chunk.min(65536);
                let out = unsafe { &mut E2BUF[..take] };
                let sc = unsafe { &mut *core::ptr::addr_of_mut!(SC2) };
                fs.read_file(sino, off as u64, out, sc).map_err(|e| {
                    let k = match e {
                        ext4::Bad::Io => 1,
                        ext4::Bad::NotDir => 2,
                        ext4::Bad::BadExtent => 3,
                        ext4::Bad::TooDeep => 4,
                        ext4::Bad::NoExtent => 5,
                        _ => 6,
                    };
                    0x4000 + (i as u32) * 16 + k
                })?
            };
            if n == 0 {
                return Err(0x50 + i as u32);
            }
            unsafe {
                core::ptr::copy_nonoverlapping(E2BUF.as_ptr(), (dest + off as u64) as *mut u8, n);
            }
            off += n;
        }
    }
    let _ = which;
    Ok(nload)
}



/// qcom,ipa_fws：pas-id 0xf，固件名 ipa_fws，内存 0x8BC00000 / 0x10000。
/// DTS 无 signal-aop / 无 SMP2P。Linux slot-b 在 ADSP 前出复位，但裸机
/// **禁** IPA-before-ADSP（ceeb6fd 硬挂无 ECM）。本刀：ADSP PAS 成功后再装。
fn load_ipa_fws(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) -> Result<u16, u32> {
    let _ = freq;
    let base = vendor_lba(ufs)?;
    ufs.read10(base, 1, DATA).map_err(|_| 0x14u32)?;
    let mag = unsafe { le_u32(core::slice::from_raw_parts(DATA as *const u8, 4096), 1024) };
    let ext4m = unsafe {
        let b = core::slice::from_raw_parts(DATA as *const u8, 4096);
        b[1080] as u32 | (b[1081] as u32) << 8
    };
    let txt = hex_text(t, unsafe { RING.seq }, " ipa v=0x", (mag as u64) << 32 | ext4m as u64);
    ring_put(ufs, freq, logring::LVL_INFO, txt);
    let dev = UfsDev { ufs, base };
    let mut fs = ext4::Fs::open(dev, unsafe { &mut *core::ptr::addr_of_mut!(SC1) }).map_err(|e| {
        0x1400u32
            | match e {
                ext4::Bad::Io => 1,
                ext4::Bad::Magic => 2,
                ext4::Bad::BlockSize => 3,
                ext4::Bad::InodeSize => 4,
                ext4::Bad::NoExtent => 5,
                ext4::Bad::BadExtent => 6,
                ext4::Bad::TooDeep => 7,
                ext4::Bad::NoSuchInode => 8,
                ext4::Bad::NotDir => 9,
                ext4::Bad::BadDirent => 10,
                ext4::Bad::NotFound => 11,
            }
    })?;
    const MDTS: [&str; 4] = [
        "/firmware/ipa_fws.mdt",
        "/firmware/image/ipa_fws.mdt",
        "/etc/firmware/ipa_fws.mdt",
        "/vendor/firmware/ipa_fws.mdt",
    ];
    let mut which = 0xffu32;
    let mut ino = 0u32;
    for (i, path) in MDTS.iter().enumerate() {
        match fs.resolve(path, unsafe { &mut *core::ptr::addr_of_mut!(SC2) }) {
            Ok(n) => {
                which = i as u32;
                ino = n;
                break;
            }
            Err(_) => {}
        }
    }
    if which == 0xff {
        return Err(0x15);
    }
    let mut got = 0usize;
    loop {
        if got >= 12288 {
            return Err(0x16);
        }
        let n = {
            let dst = unsafe { &mut PAS_META.raw[got..] };
            let sc = unsafe { &mut *core::ptr::addr_of_mut!(SC2) };
            fs.read_file(ino, got as u64, dst, sc).map_err(|_| 0x17u32)?
        };
        if n == 0 {
            break;
        }
        got += n;
    }
    if got < 52 {
        return Err(0x18);
    }
    let mdt = unsafe { &PAS_META.raw[..got] };
    if mdt[0] != 0x7f || mdt[1] != b'E' || mdt[2] != b'L' || mdt[3] != b'F' || mdt[4] != 1 {
        return Err(0x19);
    }
    let phoff = elf_u32(mdt, 28) as usize;
    let phentsize = u16::from_le_bytes([mdt[42], mdt[43]]) as usize;
    let phnum = u16::from_le_bytes([mdt[44], mdt[45]]) as usize;
    if phentsize < 32 || phnum == 0 || phnum > 64 || phoff + phnum * phentsize > got {
        return Err(0x1a);
    }
    let mut min_pa = u32::MAX;
    let mut max_pa = 0u32;
    let mut nload = 0u16;
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let ty = elf_u32(mdt, o);
        let flags = elf_u32(mdt, o + 24);
        let memsz = elf_u32(mdt, o + 20);
        let pa = elf_u32(mdt, o + 12);
        if ty != 1 || memsz == 0 || flags & (7 << 24) == (2 << 24) {
            continue;
        }
        min_pa = min_pa.min(pa);
        let end = pa.saturating_add(memsz);
        if end > max_pa {
            max_pa = end;
        }
        nload += 1;
    }
    if nload == 0 || min_pa == u32::MAX {
        return Err(0x1b);
    }
    const IPA_BASE: u64 = 0x8BC0_0000;
    const IPA_SIZE: u64 = 0x0001_0000;
    let reloc = min_pa < IPA_BASE as u32 || max_pa > (IPA_BASE + IPA_SIZE) as u32;
    unsafe {
        core::ptr::write_bytes(IPA_BASE as *mut u8, 0, IPA_SIZE as usize);
    }
    let prefix: &str = match which {
        0 => "/firmware/ipa_fws.b",
        1 => "/firmware/image/ipa_fws.b",
        2 => "/etc/firmware/ipa_fws.b",
        _ => "/vendor/firmware/ipa_fws.b",
    };
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let ty = elf_u32(mdt, o);
        let flags = elf_u32(mdt, o + 24);
        let filesz = elf_u32(mdt, o + 16) as usize;
        let fileoff = elf_u32(mdt, o + 4) as usize;
        let pa = elf_u32(mdt, o + 12);
        if ty != 1 || elf_u32(mdt, o + 20) == 0 || flags & (7 << 24) == (2 << 24) {
            continue;
        }
        if filesz == 0 {
            continue;
        }
        let dest = if reloc {
            IPA_BASE + (pa - min_pa) as u64
        } else {
            pa as u64
        };
        if dest < IPA_BASE || dest + filesz as u64 > IPA_BASE + IPA_SIZE {
            return Err(0x20 + i as u32);
        }
        // 优先 .bXX；没有则尝试 mdt 内嵌（小固件常见）。
        let mut name = [0u8; 48];
        let pb = prefix.as_bytes();
        name[..pb.len()].copy_from_slice(pb);
        name[pb.len()] = b'0' + (i / 10) as u8;
        name[pb.len() + 1] = b'0' + (i % 10) as u8;
        let name = core::str::from_utf8(&name[..pb.len() + 2]).unwrap_or("");
        match fs.resolve(name, unsafe { &mut *core::ptr::addr_of_mut!(SC2) }) {
            Ok(sino) => {
                let mut off = 0usize;
                while off < filesz {
                    let n = {
                        let chunk = filesz - off;
                        let take = chunk.min(65536);
                        let out = unsafe { &mut E2BUF[..take] };
                        let sc = unsafe { &mut *core::ptr::addr_of_mut!(SC2) };
                        fs.read_file(sino, off as u64, out, sc).map_err(|e| {
                            let k = match e {
                                ext4::Bad::Io => 1,
                                ext4::Bad::NotDir => 2,
                                ext4::Bad::BadExtent => 3,
                                ext4::Bad::TooDeep => 4,
                                ext4::Bad::NoExtent => 5,
                                _ => 6,
                            };
                            0x4000 + (i as u32) * 16 + k
                        })?
                    };
                    if n == 0 {
                        return Err(0x50 + i as u32);
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            E2BUF.as_ptr(),
                            (dest + off as u64) as *mut u8,
                            n,
                        );
                    }
                    off += n;
                }
            }
            Err(_) => {
                if fileoff + filesz > got {
                    return Err(0x30 + i as u32);
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        mdt[fileoff..].as_ptr(),
                        dest as *mut u8,
                        filesz,
                    );
                }
            }
        }
    }
    let _ = which;
    Ok(nload)
}


/// qcom,turing / pil-tz：pas-id 18，固件名 cdsp，内存 0x87400000 / 0x1E00000。
/// 代理电源只要 `vdd_cx`（cx.lvl TURBO）；与 ADSP 的 lcx/lmx 不同。
/// 仅在 IPA+ADSP 绿之后调用（见 boot 门）；禁冷启 CDSP-before-modem（8d88cd5）。
fn load_cdsp(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) -> Result<u16, u32> {
    let cx = db_id(b"cx.lvl\0\0");
    let scx = if cx != 0 { rpmh_cmd(cx, 6, freq) } else { 0 };
    let txt = hex_text(t, unsafe { RING.seq }, " cdsp cx=0x", (cx as u64) << 32 | scx as u64);
    ring_put(ufs, freq, logring::LVL_INFO, txt);
    let _ = scx;
    let base = vendor_lba(ufs)?;
    ufs.read10(base, 1, DATA).map_err(|_| 0x14u32)?;
    let mag = unsafe { le_u32(core::slice::from_raw_parts(DATA as *const u8, 4096), 1024) };
    let ext4m = unsafe {
        let b = core::slice::from_raw_parts(DATA as *const u8, 4096);
        b[1080] as u32 | (b[1081] as u32) << 8
    };
    let txt = hex_text(t, unsafe { RING.seq }, " cdsp v=0x", (mag as u64) << 32 | ext4m as u64);
    ring_put(ufs, freq, logring::LVL_INFO, txt);
    let dev = UfsDev { ufs, base };
    let mut fs = ext4::Fs::open(dev, unsafe { &mut *core::ptr::addr_of_mut!(SC1) }).map_err(|e| {
        0x1400u32
            | match e {
                ext4::Bad::Io => 1,
                ext4::Bad::Magic => 2,
                ext4::Bad::BlockSize => 3,
                ext4::Bad::InodeSize => 4,
                ext4::Bad::NoExtent => 5,
                ext4::Bad::BadExtent => 6,
                ext4::Bad::TooDeep => 7,
                ext4::Bad::NoSuchInode => 8,
                ext4::Bad::NotDir => 9,
                ext4::Bad::BadDirent => 10,
                ext4::Bad::NotFound => 11,
            }
    })?;
    const MDTS: [&str; 3] = [
        "/firmware/cdsp.mdt",
        "/firmware/image/cdsp.mdt",
        "/etc/firmware/cdsp.mdt",
    ];
    let mut which = 0xffu32;
    let mut ino = 0u32;
    for (i, path) in MDTS.iter().enumerate() {
        match fs.resolve(path, unsafe { &mut *core::ptr::addr_of_mut!(SC2) }) {
            Ok(n) => {
                which = i as u32;
                ino = n;
                break;
            }
            Err(_) => {}
        }
    }
    if which == 0xff {
        return Err(0x15);
    }
    let mut got = 0usize;
    loop {
        if got >= 12288 {
            return Err(0x16);
        }
        let n = {
            let dst = unsafe { &mut PAS_META.raw[got..] };
            let sc = unsafe { &mut *core::ptr::addr_of_mut!(SC2) };
            fs.read_file(ino, got as u64, dst, sc).map_err(|_| 0x17u32)?
        };
        if n == 0 {
            break;
        }
        got += n;
    }
    if got < 52 {
        return Err(0x18);
    }
    let mdt = unsafe { &PAS_META.raw[..got] };
    if mdt[0] != 0x7f || mdt[1] != b'E' || mdt[2] != b'L' || mdt[3] != b'F' || mdt[4] != 1 {
        return Err(0x19);
    }
    let phoff = elf_u32(mdt, 28) as usize;
    let phentsize = u16::from_le_bytes([mdt[42], mdt[43]]) as usize;
    let phnum = u16::from_le_bytes([mdt[44], mdt[45]]) as usize;
    if phentsize < 32 || phnum == 0 || phnum > 64 || phoff + phnum * phentsize > got {
        return Err(0x1a);
    }
    let mut min_pa = u32::MAX;
    let mut max_pa = 0u32;
    let mut nload = 0u16;
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let ty = elf_u32(mdt, o);
        let flags = elf_u32(mdt, o + 24);
        let memsz = elf_u32(mdt, o + 20);
        let pa = elf_u32(mdt, o + 12);
        if ty != 1 || memsz == 0 || flags & (7 << 24) == (2 << 24) {
            continue;
        }
        min_pa = min_pa.min(pa);
        let end = pa.saturating_add(memsz);
        if end > max_pa {
            max_pa = end;
        }
        nload += 1;
    }
    if nload == 0 || min_pa == u32::MAX {
        return Err(0x1b);
    }
    const CDSP_BASE: u64 = 0x8740_0000;
    const CDSP_SIZE: u64 = 0x01E0_0000;
    let reloc = min_pa < CDSP_BASE as u32 || max_pa > (CDSP_BASE + CDSP_SIZE) as u32;
    unsafe {
        core::ptr::write_bytes(CDSP_BASE as *mut u8, 0, CDSP_SIZE as usize);
    }
    let prefix: &str = match which {
        0 => "/firmware/cdsp.b",
        1 => "/firmware/image/cdsp.b",
        _ => "/etc/firmware/cdsp.b",
    };
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let ty = elf_u32(mdt, o);
        let flags = elf_u32(mdt, o + 24);
        let filesz = elf_u32(mdt, o + 16) as usize;
        let pa = elf_u32(mdt, o + 12);
        if ty != 1 || elf_u32(mdt, o + 20) == 0 || flags & (7 << 24) == (2 << 24) {
            continue;
        }
        if filesz == 0 {
            continue;
        }
        let dest = if reloc {
            CDSP_BASE + (pa - min_pa) as u64
        } else {
            pa as u64
        };
        if dest < CDSP_BASE || dest + filesz as u64 > CDSP_BASE + CDSP_SIZE {
            return Err(0x20 + i as u32);
        }
        let mut name = [0u8; 40];
        let pb = prefix.as_bytes();
        name[..pb.len()].copy_from_slice(pb);
        name[pb.len()] = b'0' + (i / 10) as u8;
        name[pb.len() + 1] = b'0' + (i % 10) as u8;
        let name = core::str::from_utf8(&name[..pb.len() + 2]).unwrap_or("");
        let sino = fs
            .resolve(name, unsafe { &mut *core::ptr::addr_of_mut!(SC2) })
            .map_err(|_| 0x30 + i as u32)?;
        let mut off = 0usize;
        while off < filesz {
            let n = {
                let chunk = filesz - off;
                let take = chunk.min(65536);
                let out = unsafe { &mut E2BUF[..take] };
                let sc = unsafe { &mut *core::ptr::addr_of_mut!(SC2) };
                fs.read_file(sino, off as u64, out, sc).map_err(|e| {
                    let k = match e {
                        ext4::Bad::Io => 1,
                        ext4::Bad::NotDir => 2,
                        ext4::Bad::BadExtent => 3,
                        ext4::Bad::TooDeep => 4,
                        ext4::Bad::NoExtent => 5,
                        _ => 6,
                    };
                    0x4000 + (i as u32) * 16 + k
                })?
            };
            if n == 0 {
                return Err(0x50 + i as u32);
            }
            unsafe {
                core::ptr::copy_nonoverlapping(E2BUF.as_ptr(), (dest + off as u64) as *mut u8, n);
            }
            off += n;
        }
    }
    let _ = which;
    Ok(nload)
}

fn pas_go(pas_id: u64, addr: u64, size: u64) -> (i64, i64, i64) {
    let pa = core::ptr::addr_of!(PAS_META) as u64;
    dcache_civac(pa, 12288);
    let fnid = 0x0200_0201u64 | 0x4000_0000;
    let mut i0 = 0i64;
    let mut i1 = 0u64;
    for _ in 0..8 {
        let (a, b) = psci::smc_ret2(fnid, 0x82, pas_id, pa, 0, 0);
        i0 = a;
        i1 = b;
        if i0 != -12 {
            break;
        }
    }
    if i0 != 0 || i1 != 0 {
        return (i0, i1 as i64, -1);
    }
    let fnid = 0x0200_0202u64 | 0x4000_0000;
    let (m0, m1) = psci::smc_ret2(fnid, 3, pas_id, addr, size, 0);
    if m0 != 0 || m1 != 0 {
        return (0, m0, m1 as i64);
    }
    dcache_civac(addr, size);
    let fnid = 0x0200_0205u64 | 0x4000_0000;
    let (a0, a1) = psci::smc_ret2(fnid, 1, pas_id, 0, 0, 0);
    (a0, a1 as i64, 0)
}

fn pas_init_image(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    let n = MODEM_MDT.len();
    if n > 12288 {
        s20_log(ufs, freq, t, " s20 pas=0x", 0xE0);
        return;
    }
    unsafe {
        PAS_META.raw[..n].copy_from_slice(MODEM_MDT);
    }
    let pa = core::ptr::addr_of!(PAS_META) as u64;
    dcache_civac(pa, n as u64);
    // SCM_SIP_FNID(SCM_SVC_PIL=2, PAS_INIT_IMAGE=1) | SMC64_MASK
    // arginfo = SCM_ARGS(2, SCM_VAL, SCM_RW) = 0x82
    let fnid = 0x0200_0201u64 | 0x4000_0000;
    s20_log(ufs, freq, t, " s20 pas0=0x", pa);
    let mut r0 = 0i64;
    let mut r1 = 0u64;
    for _ in 0..8 {
        let (a, b) = psci::smc_ret2(fnid, 0x82, 4, pa, 0, 0);
        r0 = a;
        r1 = b;
        if r0 != -12 {
            break;
        }
    }
    s20_log(ufs, freq, t, " s20 pas=0x", r0 as u64);
    s20_log(ufs, freq, t, " s20 pas1=0x", r1);
    if r0 != 0 || r1 != 0 {
        return;
    }
    // PAS_MEM_SETUP：ELF 可重定位跨度 0x11800000，放在 modem_wlan
    // 起始。下一块保留内存从 0x9F400000 起，这段止于 0x9D800000。
    let fnid = 0x0200_0202u64 | 0x4000_0000;
    let (m0, m1) = psci::smc_ret2(fnid, 3, 4, 0x8c00_0000, 0x1180_0000, 0);
    s20_log(ufs, freq, t, " s20 pas2=0x", m0 as u64);
    s20_log(ufs, freq, t, " s20 pas3=0x", m1);
    if m0 != 0 || m1 != 0 {
        return;
    }
    match load_modem_segs(ufs) {
        Ok(n) => {
            s20_keep(ufs, freq, t, " s20 ld=0x", n as u64);
            // MODEM_FAT 已装：开机一次查 SO，不碰 smem_scan / 不真喂 RRQ。
            tf_boot_probe(ufs);
        }
        Err(i) => {
            s20_keep(ufs, freq, t, " s20 ld=0x", 0xE000 | i as u64);
            return;
        }
    }
    dcache_civac(0x8c00_0000, 0x1180_0000);
    // PAS_AUTH_AND_RESET，SCM_ARGS(1) = 1，参数只有 pas-id。
    let fnid = 0x0200_0205u64 | 0x4000_0000;
    let (a0, a1) = psci::smc_ret2(fnid, 1, 4, 0, 0, 0);
    s20_keep(ufs, freq, t, " s20 pas4=0x", a0 as u64);
    s20_keep(ufs, freq, t, " s20 pas5=0x", a1);
}

/// SMEM 只读快照。`g` 的位见 `S4-LINUX-PATH.md`。
struct SmemSnap {
    init: u32,
    ver: u32,
    nent: u32,
    part: u32,
    free: u32,
    g: u32,
    slave: u32,
    smpm: u32,
    d0: u32,
    d1: u32,
    d2: u32,
    d3: u32,
    rx0: u32,
    boff: u32,
    b0: u32,
    b1: u32,
    blast: u32,
    k: u32,
    cached: u32,
    ev: u32,
    n0: u32,
    gk: u32,
}

static mut SMEM_ENTS: [u32; 10] = [0; 10];
static mut SMEM_RX: [u32; 22] = [0; 22];
/// NEW_SERVER 的服务号、实例，最多 8 对。
static mut SMEM_SVC: [u32; 16] = [0; 16];
static mut SMEM_NSVC: u32 = 0;
/// WLFW（svc 0x45）目击闩。扫描每轮只留 8 个 NEW_SERVER，槽 b 的
/// `0x45` 在 node 0 端口 88，排在这 8 个之后，`q=` 看不住它。
/// 高位 1 = 见过；低 31 位是实例字。不随扫描清掉。
static mut SAW_45: u32 = 0;
/// IPA QMI（svc 0x31）NEW_SERVER 目击闩：sm iq=seen.inst（零写，不限首 8 窗）。
/// tip：另闩 node:port（偏 inst 0x201），供 INIT_MODEM_DRIVER 单发。
static mut IQ_SEEN: u32 = 0;
static mut IQ_INST: u32 = 0;
static mut IQ_NODE: u32 = 0;
static mut IQ_PORT: u32 = 0;
/// IPA QMI 客户端口（本机 node1）；避开 0x10/11/12/14。
const IPA_QRTR_PORT: u32 = 0x15;
/// AP 侧 IPA 服务端。和客户端口分开。
const IPA_SVC_PORT: u32 = 0x16;
/// qmi_send_new_server 写 version | (instance << 8)。
/// qmi_add_server(0x31, vers 1, instance 1) → 0x101。slot-b 是 0x31/0x101。
const IPA_A5_INST: u32 = 0x101;
/// 服务端口上第一笔和最近一笔 QMI 请求的 msgid。0 表示还没见到。
static mut IPA_REQ: u32 = 0;
static mut IPA_LAST: u32 = 0;
/// QMI_IPA_CONFIG_REQ（0x27）。内核回 TLV 0x02 result 成功。
static mut CFG_PEND: bool = false;
static mut CFG_TXN: u32 = 0;
static mut CFG_NODE: u32 = 0;
static mut CFG_PORT: u32 = 0;
static mut CFG_SENT: u32 = 0;
static mut CFG_DONE_TXN: u32 = 0xffff_ffff;
static mut CFG_DONE_PORT: u32 = 0xffff_ffff;
/// 成功的 CONFIG 应答会在几秒内弄掉 USB。先不回，只留请求前 4 个字。
static mut CFG_ARM: u8 = 0;
static mut CAN_DONE: u8 = 0;
static mut CFG_CAP: [u32; 4] = [0; 4];
static mut CFG_CAP_N: u32 = 0;
/// 0x20 登记。应答成功后，若 INIT 已完成，再发 0x22。
static mut IR_PEND: bool = false;
static mut IR_TXN: u32 = 0;
static mut IR_NODE: u32 = 0;
static mut IR_PORT: u32 = 0;
static mut IR_DONE: u8 = 0;
static mut IND_SENT: u8 = 0;
/// 0x15/0x16 上第一笔不是 0x27 的包：port<<16 | qtype<<8 | msgid。
static mut JX_OTHER: u32 = 0;
/// QMI_IPA_INIT_MODEM_DRIVER_REQ_V01。
const IPA_INIT_MSGID: u32 = 0x0021;
/// sm ii=st.txn.res — st:0未跑 1已发待RESP 2RESP到 3TX失败 4缺前置；res=TLV02 或 ffffffff。
static mut II_ST: u32 = 0;
/// 最近一次没发出的原因：1无环 2已发 3无口 4实例不对 5 GSI/ig 未绿 6发送环忙 7 TX 失败。
static mut II_WHY: u32 = 0;
/// why=6 时发送环 head/tail，以及 tail 上 8 个字（头、intent、长度、源口、大小、目的、QMI）。
static mut II_HX: u32 = 0;
static mut II_TL: u32 = 0;
static mut II_PK: [u32; 8] = [0; 8];
static mut II_TXN: u32 = 0;
static mut II_RES: u32 = 0xffff_ffff;
static mut II_PEND: bool = false;
static mut II_SENT: u32 = 0;
/// modem 的 INIT_MODEM_DRIVER_CMPLT（msgid 0x35）请求。ic=见到次数.已回次数。
static mut IC_PEND: bool = false;
static mut IC_TXN: u32 = 0;
static mut IC_NODE: u32 = 0;
static mut IC_PORT: u32 = 0;
static mut IC_N: u32 = 0;
static mut IC_SENT: u32 = 0;
/// NEW_LOOKUP（类型 10）的服务号、实例字，最多 8 对。
static mut SMEM_LOOK: [u32; 16] = [0; 16];
static mut SMEM_NLOOK: u32 = 0;
static mut SMEM_QSTOP: u32 = 0;
static mut SMEM_QCMD: u32 = 0;
static mut GLINK_ACKED: bool = false;
static mut GLINK_HELLO: bool = false;
static mut GLINK_INTENTS: bool = false;
static mut GLINK_LOOKUP: bool = false;
static mut GLINK_LOC: bool = false;
/// 0 还没公告，1=0x35 已发，2 rmtfs，3 tftp，4 pd-mapper 齐。
static mut HOST_ADV: u32 = 0;
static mut LISTEN_TICK: u32 = 0;
static mut LAST_IID: u32 = 0;
static mut LAST_ISZ: u32 = 0;
/// 两次活体：HELLO 的 DATA 一次是 lcid 1（0x10009），一次是 lcid 2（0x20009）。
fn qrtr_lcid(lcid: u32) -> bool {
    lcid == 1 || lcid == 2
}
/// 对端通道上见过的最大 intent，不论够不够 52。
static mut SEEN_ISZ: u32 = 0;
static mut HAVE_IID: bool = false;
/// 已经归还过 intent 的接收偏移。qcom_glink_rx_done 每个 TX_DATA 只还一次。
static mut RX_VIS: u64 = 0;
/// 本轮从 0 走到的接收偏移，以及 item 480 的长度。消费尾按这个往前推。
static mut RX_PARSED: u64 = 0;
static mut GLINK_RXLEN: u64 = 0;
static mut RX_LIID: [u32; 16] = [0; 16];
static mut RX_LIID_N: u32 = 0;
static mut RX_DONE_N: u32 = 0;
/// confirm_rx 置位时要回 RESUME_TX，否则对方这条流的下一包不发。
static mut CF_VIS: u64 = 0;
static mut CF_PEND: bool = false;
static mut CF_SN: u32 = 0;
static mut CF_SP: u32 = 0;
static mut CF_DN: u32 = 0;
static mut CF_DP: u32 = 0;
static mut CF_N: u32 = 0;
/// 收到的 modem HELLO 次数。没收到之前不发 NEW_SERVER。
static mut HELLO_VIS: u64 = 0;
static mut HELLO_RX: u32 = 0;
static mut PD_NAME: [u8; 64] = [0; 64];
static mut PD_NAME_N: u8 = 0;
/// 任一笔 GET_DOMAIN_LIST 要过 kernel/elf_loader 就记 1。pn 只留最后一次。
static mut PD_SAW_ELF: u8 = 0;
/// 上次编进应答的命中数，以及 QMI 载荷长度（不含 7 字节头）。
static mut PD_HITS: u8 = 0;
static mut PD_QLEN: u16 = 0;
static mut PD_PEND: bool = false;
static mut PD_TXN: u32 = 0;
static mut PD_NODE: u32 = 0;
static mut PD_PORT: u32 = 0;
static mut PD_SEEN: [u32; 8] = [0xffff_ffff; 8];
static mut PD_SEEN_N: u32 = 0;
static mut PD_N: u32 = 0;
/// rmtfs 队列非空。tftp 仍是单槽。同一次 FIFO 里的 OPEN/ALLOC 都要留下。
static mut RF_PEND: bool = false;
static mut RF_TXN: u32 = 0;
static mut RF_NODE: u32 = 0;
static mut RF_PORT: u32 = 0;
static mut RF_MSG: u32 = 0;
static mut RF_OFF: u64 = 0;
static mut RF_FIFO: u64 = 0;
/// 已处理键：client_port<<16 | txn。不可只记 txn——多路 OPEN 常从 txn=1 重开。
static mut RF_SEEN: [u32; 32] = [0xffff_ffff; 32];
static mut RF_SEEN_N: u32 = 0;
static mut RF_N: u32 = 0;
static mut RF_PATH: [u8; 64] = [0; 64];
static mut RF_PATH_N: u8 = 0;
static mut RF_ID: u32 = 0xffff_ffff;
static mut RF_SZ: u32 = 0;
/// hyp_assign 的 x0。0xffffffff 还没叫。
static mut RF_AS: u32 = 0xffff_ffff;
/// 打到 0x10 并收进队列的次数。
static mut RF_HIT: u32 = 0;
/// 丢掉：bit0=seen，bit1=队列满。
static mut RF_DROP: u32 = 0;
/// 最近 8 次 msgid（低 8 位）| txn<<8。
static mut RF_HIST: [u32; 8] = [0; 8];
static mut RF_HIST_N: u32 = 0;
/// IOVEC rfs_iovec 成功/失败次数（冲 0x45：影子过短曾全失败）。
static mut RF_IOK: u32 = 0;
static mut RF_IFAIL: u32 = 0;
/// 上一笔 OPEN 应答前 12 字节（对账 wire）。
static mut RF_RSP: [u8; 12] = [0; 12];
static mut RF_RSP_N: u8 = 0;
/// 活体 ik=4.0 时单槽就够。8 深队列是过期任务加回来的，已收成 1。
const RF_Q_CAP: usize = 1;
#[derive(Copy, Clone)]
struct RfSlot {
    txn: u32,
    node: u32,
    port: u32,
    msg: u32,
    off: u64,
    fifo: u64,
    path: [u8; 64],
    path_n: u8,
}
static mut RF_Q: [RfSlot; RF_Q_CAP] = [RfSlot {
    txn: 0,
    node: 0,
    port: 0,
    msg: 0,
    off: 0,
    fifo: 0,
    path: [0; 64],
    path_n: 0,
}; RF_Q_CAP];
static mut RF_QH: u32 = 0;
static mut RF_QN: u32 = 0;
/// 任意打到本机 node1 的 DATA：port|msgid<<8|qtype<<16，最近 8 条。
static mut QP_HIST: [u32; 8] = [0; 8];
static mut QP_HIST_N: u32 = 0;
static mut QP_VIS: u64 = 0;
/// pm-service / SSREQ。对账 Linux qrtr-lookup：svc=0x35 inst=0x1001 at 1:16407。
const PM_SVC: u32 = 0x35;
const PM_INST: u32 = 0x1001;
const PM_QRTR_PORT: u32 = 16407;
/// 端口 PM_QRTR_PORT 收到的请求数。
static mut PM_HIT: u32 = 0;
static mut PM_PEND: bool = false;
static mut PM_TXN: u32 = 0;
static mut PM_NODE: u32 = 0;
static mut PM_PORT: u32 = 0;
static mut PM_MSG: u32 = 0;
static mut PM_N: u32 = 0;
/// SERVREG_NOTIF 客户端（冲 0x45）：对 modem 0x42@0xb401 听 wlan_pd。
/// 不动 glink TX intent pick；只走 glink_tx_payload + LAST_IID。
const SR_SVC: u32 = 0x42;
const SR_INST: u32 = 0xb401;
const SR_QRTR_PORT: u32 = 0x14;
const SR_NAME: &[u8] = b"msm/modem/wlan_pd";
/// MPSS SMP2P：pub 返回码、出向 valid_entries、入向名位（bit0=ipa bit1=wlan）。
static mut MPSS_PUB: u32 = 0xffff_ffff;
static mut MPSS_VOUT: u32 = 0;
static mut MPSS_IN: u32 = 0;
/// 出向 ipa 条目的 value 地址。应答时钟查询时写成 bit0|bit1。
static mut IPA_SMP_VAL: u64 = 0;
static mut IPA_SMP_SENT: u8 = 0;

/// CAF：UP=0x1FFFFFFF DOWN=0x0FFFFFFF UNINIT=0x7FFFFFFF。
const SR_STATE_UP: u32 = 0x1fff_ffff;
const SR_STATE_DOWN: u32 = 0x0fff_ffff;
const SR_STATE_UNINIT: u32 = 0x7fff_ffff;
static mut SR_NODE: u32 = 0;
static mut SR_PORT: u32 = 0;
static mut SR_HAVE: bool = false;
/// REGISTER_LISTENER 成功发出次数（sm sr= 左）。
static mut SR_HIT: u32 = 0;
/// RESP/IND curr_state；无 TLV / 未回 = 0xffffffff。
/// 0x7fffffff = UNINIT（真状态，不是哨兵）。
static mut SR_STATE: u32 = 0xffff_ffff;
/// REGISTER RESP TLV 0x02 result|error；未回 = ffffffff。
static mut SR_RES: u32 = 0xffff_ffff;
static mut SR_TXN: u32 = 1;
static mut SR_PEND: bool = false;
static mut SR_NEED_RP: bool = false;
/// RESTART_PD 发出次数 / RESP result（或 ffffffff）。
static mut SR_RP_N: u32 = 0;
static mut SR_RP_ERR: u32 = 0xffff_ffff;
static mut SR_RP_PEND: bool = false;
/// STATE_UPDATED IND(0x22) → SET_ACK(0x23) 待发。
static mut SR_ACK_PEND: bool = false;
static mut SR_ACK_TXN: u16 = 0;
static mut SR_IND_N: u32 = 0;
/// LOOKUP 待回 NEW_SERVER：svc|inst<<16；0=无。
static mut LOOK_PEND: u32 = 0;
static mut LOOK_N: u32 = 0;
static mut RFS_READY: u8 = 0;
/// rmt_storage_alloc_buff_cb：buff_size 0 表示 IOVEC 里的地址是窗口内偏移。
static mut RFS_IOFF: u8 = 0;
/// 四块影子各清一次。bit0..3 对应 fs1/fs2/fsc/fsg。
static mut RFS_Z: u8 = 0;
/// 开机从 GPT 灌入的影子位图（同 slot）。es= 导出。
static mut RFS_SEED: u8 = 0;
/// gpt_verify 留下的条目数，供 rfs_seed 名查。
static mut GPT_ENTRIES: usize = 0;
/// 扫到的 EFS 相关 GPT 名（拼接，供 sm gn=）。
static mut RFS_GN: [u8; 64] = [0; 64];
static mut RFS_GNN: u8 = 0;

/// IPA fws（pas-id 0xf @ 0x8BC00000 / 0x10000）。ADSP PAS 成功之后才跑。
/// sm ip=st.p0.p1.p2 — st 同 ADSP（高位置位=段数）；p0/1/2 = INIT/MEM/AUTH x0。
static mut IPA_ST: u32 = 0;
static mut IPA_P: [u32; 3] = [0xffff_ffff; 3];

/// IPA/GSI 只读 MMIO 闩（PAS 绿后一次）。
/// sm ig=st.iver.gcfg.gstat.gver.hp2.ch0.ca.cb.sm.r
/// sm=IPA_SHARED_MEM_SIZE 原字；r=restr_bytes=baddr*8（INIT 绝对地址偏量）。
/// 同次 gc=st.c0.c1.c2.c3.irq.err（CH7 态；仍零写）。
/// 零写：无 doorbell / 无 ring / 无 QMI INIT / 不改 CDSP·TFTP·TX。
static mut IG_DONE: u32 = 0;
/// st: 0 未跑 / 1 已采 / 2 IPA 未绿跳过（不应持久）
static mut IG_ST: u32 = 0;
static mut IG_IVER: u32 = 0xffff_ffff;
static mut IG_GCFG: u32 = 0xffff_ffff;
static mut IG_GSTAT: u32 = 0xffff_ffff;
static mut IG_GVER: u32 = 0xffff_ffff;
static mut IG_HP2: u32 = 0xffff_ffff;
static mut IG_CH0: u32 = 0xffff_ffff;
/// SRAM canary 字：ca=@modem_hdr-4 (0x684)；cb=@v4_flt_hash-4 (0x284)；未映=ffffffff
static mut IG_CA: u32 = 0xffff_ffff;
static mut IG_CB: u32 = 0xffff_ffff;
/// IPA_SHARED_MEM_SIZE@cfg+0x54：baddr[31:16]|size[15:0]（单位 8B）
static mut IG_SMEM: u32 = 0xffff_ffff;
/// smem_restricted_bytes = baddr*8 — INIT TLV 绝对地址 = ofst + r
static mut IG_R: u32 = 0xffff_ffff;

/// GSI CH7：RO 闩后可由 ch7_cfg 覆写。sm gc=st.c0.c1.c2.c3.irq.err
/// c0=CNTXT0（含 CHSTATE）; c1=R_LENGTH; c2/c3=ring base LS/MS; irq=TYPE_IRQ; err=ERROR_LOG。
/// st: 0 未跑 / 1 STARTED（或 RO 已采）/ 2 EV ALLOC 超时 / 3 CH ALLOC 超时 / 4 START 超时。
/// tip：CONFIG+ALLOC+START+最小 TRE；INIT 见 ipa_init_tick（本刀后）。无 EP16 cfg。
static mut GC_ST: u32 = 0;
static mut GC_CFG_DONE: u32 = 0;
static mut GC_C0: u32 = 0xffff_ffff;
static mut GC_C1: u32 = 0xffff_ffff;
static mut GC_C2: u32 = 0xffff_ffff;
static mut GC_C3: u32 = 0xffff_ffff;
static mut GC_IRQ: u32 = 0xffff_ffff;
static mut GC_ERR: u32 = 0xffff_ffff;

/// GSI CH7 DRAM 划区（allocate-only；无 GSI MMIO 写 / 无 doorbell / 无 INIT）。
/// 基址：modem_wlan PAS 尾 `0x9D800000` → RFS `0x9E000000` 前 8MiB 空洞（DTS 无 no-map）。
/// 布局：CH@+0 (0x1000) + EVT@+0x1000 (0x1000) + RX pool@+0x2000 (余下至 2MiB)。
/// SMMU：I4 STE 0x520→USB_CB 已恒等映 0x80000000–0xBFFFFFFF；本区无需新 STE。
/// sm gr=st.ch.evt.rx.sz — st=1 已清零；ch/evt/rx=PA；sz=总字节。
const GSI_RING_BASE: u64 = 0x9D80_0000;
const GSI_CH_RING_OFF: u64 = 0x0;
const GSI_CH_RING_SZ: u64 = 0x1000; // 2 * (256 * 8) — lito wan-rx-desc default
const GSI_EVT_RING_SZ: u64 = 0x1000; // dedicated EVT（WAN_CONS 非 common）
const GSI_EVT_RING_OFF: u64 = GSI_CH_RING_OFF + GSI_CH_RING_SZ;
const GSI_RX_POOL_OFF: u64 = GSI_EVT_RING_OFF + GSI_EVT_RING_SZ;
const GSI_CARVE_SZ: u64 = 0x20_0000; // 2MiB；RX ≈ 192×8KiB + slack
const _: () = assert!(GSI_RX_POOL_OFF == 0x2000);
const _: () = assert!(GSI_CARVE_SZ > GSI_RX_POOL_OFF);
static mut GR_DONE: u32 = 0;
static mut GR_ST: u32 = 0;
static mut GR_CH: u32 = 0xffff_ffff;
static mut GR_EVT: u32 = 0xffff_ffff;
static mut GR_RX: u32 = 0xffff_ffff;
static mut GR_SZ: u32 = 0xffff_ffff;

/// CDSP（pas-id 18 @ 0x87400000 / 0x1E00000）。仅 IPA+ADSP 绿后、modem 前。
/// sm cd=st.p0.p1.p2 — 同 ip=：st 高位|段数；p0/1/2 = INIT/MEM/AUTH x0。
static mut CDSP_ST: u32 = 0;
static mut CDSP_P: [u32; 3] = [0xffff_ffff; 3];
/// pil-cdsp bus：bit0=CO2 bit1=CO0 ACTIVE 成功；0=未跑/全 miss。
static mut CDSP_CB: u32 = 0;
/// SMP2P 出向分配码 / 入向 slave / 等待毫秒。sm ce=pub.rdy.wt。
static mut CDSP_PUB: u32 = 0xffff_ffff;
static mut CDSP_RDY: u32 = 0x8000_0000;
static mut CDSP_WT: u32 = 0xffff_ffff;
/// pas_go 前 sfr_wipe(601) 结果。sm cz=st.zb（对齐 ADSP az=）。
static mut CDSP_Z: u32 = 0xff;
static mut CDSP_ZB: u32 = 0xff;
static mut CDSP_FLUSHED: u64 = 0;
static mut CDSP_HDR: u64 = 0;
static mut CDSP_ACKED: u32 = 0;


/// ADSP 加载码。0x80000000 以上是段数，其余是失败码。p0/p1/p2 是 PAS 三步的 x0。
static mut ADSP_ST: u32 = 0;
static mut ADSP_P: [u32; 5] = [0xffff_ffff; 5];
/// LPASS SMP2P。pub 是出向项 429 的分配码。slv 是入向 443 的 slave-kernel。
/// bit1 才是 err-ready；0x80000000 这一位表示还没有读到这个条目。wt 是开机等它的毫秒。
static mut ADSP_PUB: u32 = 0xffff_ffff;
static mut ADSP_SLV: u32 = 0x8000_0000;
/// pil_remove_proxy_vote 的两笔 x0。0xffffffff 还没撤。
static mut ADSP_UV: u32 = 0xffff_ffff;
/// 第一次 sm 只读。第二次才撤代理票，免得撤票和 2323 抢同一段开机。
static mut SM_ARMED: bool = false;
static mut ADSP_WT: u32 = 0xffff_ffff;
/// PAS 之前的 slave-kernel。和等到的值一样，说明 bit1 是上一轮留下的。
static mut ADSP_PRE: u32 = 0x8000_0000;
/// bit0 第一次变成 1 的毫秒，从 PAS 返回算起。0xffffffff 还没出现。
static mut ADSP_FT: u32 = 0xffff_ffff;
/// 开机前清 SMEM 423 的结果。0x10 清掉了非空串，0x11 本来就是空的。
/// 低字节是清掉之前的首字节。
static mut ADSP_Z: u32 = 0xff;
static mut ADSP_ZB: u32 = 0xff;
/// 出向项头的物理地址。0x80AFE000 以上靠着 TOC，不写。
static mut LPASS_HDR: u64 = 0;
/// 等于 LPASS_HDR 时，上半区这一项的缓存行已经作废，可以经 NC 窗口写。
static mut LPASS_FLUSHED: u64 = 0;
/// 入向头三个字：版本/特性、条目数、flags。ack：1 改过并踢了，2 版本还不是 1，
/// 3 没有出向项，4 不用改，5 读失败。
static mut ADSP_IN: [u32; 3] = [0xffff_ffff; 3];
static mut ADSP_ACK: u32 = 0;
static mut LPASS_ACKED: u8 = 0;
static mut ADSP_SFR: Sfr = Sfr {
    st: 0xff,
    sz: 0,
    gp: 0,
    at: 0,
    c: 0,
    w: [0; 32],
};
static mut TF_PEND: bool = false;
/// 0 空闲，1 已发 OACK，2 已发 DATA，3 短块送完，9 对端 ERROR。
static mut TF_ST: u8 = 0;
/// 对 ota_firewall/ruleset 回过几次 file-not-found，以及最后一次用的源端口。
static mut TF_REJ: u32 = 0;
static mut TF_REJ_PORT: u32 = 0;
/// 每次拒绝换一个口，从 0x1000 起，避开服务口。
static mut TF_TID: u32 = 0x1000;
static mut TF_HASOPT: u8 = 0;
/// 请求里出现过 rsize 键。值为 0 是 stat，不是“按整文件送”。
static mut TF_HAS_RS: u8 = 0;
static mut TF_RSIZE: u32 = 0;
/// 刀 G5：RRQ 里的 seek 键（stock tqftpserv 段落读用它定文件偏移）。
/// DATA 的起点 = seek + ack*blksize，OACK 要原样回显。
static mut TF_HAS_SK: u8 = 0;
static mut TF_SEEK: u32 = 0;
/// RRQ 里的选项。OACK 只回请求里有的项，再加 tqftpserv 总会回的 timeoutms。
/// wsize 缺省 0：请求没带就不写进 OACK。
static mut TF_REQ_BLK: u32 = 512;
static mut TF_REQ_WSZ: u32 = 0;
static mut TF_REQ_TMO: u32 = 1000;
static mut TF_HAS_TS: u8 = 0;
static mut TF_BLKNO: u16 = 0;
static mut TF_ACK_BLK: u16 = 0;
static mut TF_ACK_OFF: u32 = 0xffff_ffff;
static mut TF_ACK_PEND: bool = false;
/// 旧的固定数据口。SO 的这次传输改用 TF_XFER_PORT。
const TF_DATA_PORT: u32 = 0x21;
/// 我们播报的 TFTP 服务口（svc 4096→0x11，RRQ 的目的地）。客户端可能把 ACK
/// 回到这里而不是 OACK 源口——tf_note_ack 一并监听。
const TF_SVC_PORT: u32 = 0x11;
/// 这次 SO 传输的源端口。0 表示还没发 OACK。
static mut TF_XFER_PORT: u32 = 0;
/// OACK 发出时刻（CNTVCT）。宽限刀用：OACK 后 2s 无 ACK-0 就合成 ack=0 直发 DATA 块 1
/// （非规客户端 OACK 后不 ACK 只等数据的假设 E）。0 = 未发。
static mut TF_OACK_AT: u64 = 0;
/// TX 环快照（tf_reply 每次进来时读）：head.tail.环容。tail 推进=modem 消费了
/// 我们的包（送达到 glink 层）；tail 冻结=块压根没被收（假设 F：大块传输层被弃）。
static mut TF_TXH: u32 = 0;
static mut TF_TXT: u32 = 0;
static mut TF_TXSZ: u32 = 0;
/// OACK 之后收包滚动备忘：最近 4 包的 (dst_port, TFTP op)，时间序旧→新。
/// 单发闩被 servreg 包（0x12 口 QMI 头 00 04 撞 ACK 码）吃掉的教训。
static mut TF_RX_LOG_P: [u32; 4] = [0; 4];
static mut TF_RX_LOG_O: [u32; 4] = [0; 4];
/// 刀 G6：第三元=op5 错误码 / op4 块号（ffff=没读到）。
static mut TF_RX_LOG_A: [u32; 4] = [0; 4];
// 刀 G8：DATA 路径四级计数器（纯插桩，不改行为）——G7 实测立即 DATA 零上环
// （tt=be8 全靴 ~3KB＝纯 OACK 体积），静态推演所有门都该过，必须实测堵点。
// dv=进入.读零.tx拒.环闸挡：进DATA分支 / so_read==0 / glink_tx_payload 假 /
// tf_reply 顶环容量闸挡（head≠tail 且 head+8192>环）。
static mut TF_DV: u32 = 0;
static mut TF_DZ: u32 = 0;
static mut TF_DX: u32 = 0;
static mut TF_DC: u32 = 0;
// 刀 G12：tf_reply 四分位计数＋OACK TX 成败（pq=call.noarm.nost.nopend、
// ox=ok.no）。G8 起全靴 dv=0 与静态推演矛盾（G7 synth 置 pend、pump 每圈
// 免费评估、TF_DC=0、II_ST=2 吸死）——call/dc/noarm/nost/nopend/dv0 六格
// 完整分割每次调用，堵点只许实测不许猜（G10 教训）。
static mut TF_PQ_CALL: u32 = 0;
static mut TF_PQ_NOARM: u32 = 0;
static mut TF_PQ_NOST: u32 = 0;
static mut TF_PQ_NOPEND: u32 = 0;
static mut TF_PQ_TXOK: u32 = 0;
static mut TF_PQ_TXNO: u32 = 0;
// 刀 G10：转录插桩（零行为变化）——最近 2 笔 op4/op5 入站包原文 ≤24B
// （op+码+错误消息串）。op5 的消息文本＝客户端拒收理由原文，一眼定谳。
static mut TF_RXR: [[u8; 24]; 2] = [[0; 24]; 2];
static mut TF_RXR_L: [u8; 2] = [0; 2];
static mut TF_RXR_N: u32 = 0;
// 刀 G9c：retry 路 OACK 服务计数（st=2/3 新到 pend 的应答数）。
static mut TF_DR: u32 = 0;
static mut TF_RX_LOG_N: u32 = 0;
/// DATA 缓冲。8200 = 4 字节 TFTP 头 + 8192 上限（so_read 单发双扇区 4096×2）。
static mut TF_BLKBUF: [u8; 8200] = [0; 8200];
static mut TF_NODE: u32 = 0;
static mut TF_PORT: u32 = 0;
/// 刀 G2：RRQ 客户端地址观测。rq=本笔 node.port，rq2=上一笔——port 逐笔换新
/// = 客户端每笔重开 socket（回信追着死口打，立刻现形）；port 稳定 = socket 常驻。
static mut TF_RQ_NODE: u32 = 0;
static mut TF_RQ_PORT: u32 = 0;
static mut TF_RQ2_NODE: u32 = 0;
static mut TF_RQ2_PORT: u32 = 0;
/// 刀 G2 探针：CTRL 口(0xfffffffe) OACK 副本是否已发（0/1）。我们答 LOOKUP 用的
/// 正是 (1,CTRL)→客户端 这个源址形状且客户端据此找上门；若它只认这种源址，
/// 副本一收住它就有反应（tf 冻结或 rx 现形）。
static mut TF_CTRL_PROBE: u32 = 0;
/// 刀 G3：本笔（最新刷到的）RRQ 客户端地址的捕获时刻（CNTVCT）。
/// rd= OACK 时刻减它 = 回信时延，对赌客户端 ~1s 的 socket 存活窗。
static mut TF_RQ_AT: u64 = 0;
/// seen 表环形游标。满 8 后不再冻结：永远记最新 8 个偏移。
static mut TF_SEEN_I: u32 = 0;
static mut TF_OP: u32 = 0;
static mut TF_OFF: u32 = 0;
static mut TF_SEEN: [u32; 8] = [0xffff_ffff; 8];
static mut TF_SEEN_N: u32 = 0;
static mut TF_N: u32 = 0;
static mut TF_NAME: [u8; 64] = [0; 64];
static mut TF_NAME_N: u8 = 0;
/// 开机一次 FAT 探针结果（不真喂）。0=未跑 1=命中；失败 TZ_ST=4/errcode，TZ_CL=errcode(|depth<<8)。
/// sm：tz=st.sz.cl
static mut TZ_ST: u32 = 0;
static mut TZ_SZ: u32 = 0;
static mut TZ_CL: u32 = 0;
/// 文件头。TZ_MAG 是前 4 字节小端（ELF 为 0x464c457f）。
/// TZ_EH 低 16 位 e_type，高 16 位 e_machine。Hexagon 是 164（0xa4）。
static mut TZ_MAG: u32 = 0;
static mut TZ_EH: u32 = 0;
/// cx addr/sel/sts，mss addr/sel/sts，xo addr/data/sts。sel 0x100 没找到，
/// 0x101 不是 ARC，0x102 aux 里没有 384。
static mut PROXY: [u32; 9] = [0; 9];
/// cx、mss 的 aux 前 4 字节，各是两档 u16。
static mut PROXY_AUX: [u32; 2] = [0; 2];
static mut QMPW: [u32; 8] = [0; 8];
/// `log_failure_reason` 的快照。st：0 非空，1 空串，2 没有这项，
/// 3 没有全局分区，4 链表坏，5 分区头不对，6 地址出了这块 SMEM，
/// 7 没拿到锁，8 项号超过 item_count，9 TOC 不对，a 不是版本 12，b 未初始化。
struct Sfr {
    st: u32,
    sz: u32,
    gp: u32,
    at: u32,
    c: u32,
    w: [u32; 32],
}
static mut SFR: Sfr = Sfr {
    st: 0,
    sz: 0,
    gp: 0,
    at: 0,
    c: 0,
    w: [0; 32],
};

static mut SMEM_SNAP: SmemSnap = SmemSnap {
    init: 0,
    ver: 0,
    nent: 0,
    part: 0,
    free: 0,
    g: 0,
    slave: 0,
    smpm: 0,
    d0: 0,
    d1: 0,
    d2: 0,
    d3: 0,
    rx0: 0,
    boff: 0,
    b0: 0,
    b1: 0,
    blast: 0,
    k: 0,
    cached: 0,
    ev: 0,
    n0: 0,
    gk: 0,
};

/// 只读 SMEM 头。入口把 DAIF.A 掩着，被拒绝的 load 会停在指令上，
/// 不进异常桩——2026-10-17 整段扫描（含区尾 TOC）把核冻死，ping 不再回。
fn l2_swap(index: usize, desc: u64, va_page: u64) {
    let slot = unsafe { (core::ptr::addr_of_mut!(L2) as *mut u64).add(index) };
    unsafe { slot.write_volatile(desc) };
    dcache_civac(slot as u64, 8);
    unsafe {
        core::arch::asm!(
            "dsb sy",
            "tlbi vaae1, {va}",
            "dsb sy",
            "isb",
            va = in(reg) va_page,
            options(nostack),
        );
    }
}

/// SMEM 区 0x80900000–0x80B00000。0x80A00000 以下是 WB，先 ivac 再读；
/// 以上在 TOC 那块 NC 映射里直读。不 ivac NC 区。
fn smem_window(a: u64, n: u64) -> bool {
    n > 0 && a >= 0x8090_0000 && a.checked_add(n).is_some_and(|e| e <= 0x80B0_0000)
}

fn smem_rd32(a: u64) -> Option<u32> {
    if a % 4 != 0 || !smem_window(a, 4) {
        return None;
    }
    if a < 0x80A0_0000 {
        dcache_ivac(a, 4);
    }
    Some(rd32(a as usize))
}

fn smem_rd8(a: u64) -> Option<u8> {
    if !smem_window(a, 1) {
        return None;
    }
    if a < 0x80A0_0000 {
        dcache_ivac(a, 1);
    }
    Some(unsafe { (a as *const u8).read_volatile() })
}

fn sfr_words(at: u64, n: u32, w: &mut [u32; 32]) -> bool {
    let n = n.min(128);
    let mut i = 0u32;
    while i < n {
        let Some(b) = smem_rd8(at + i as u64) else {
            return false;
        };
        w[(i / 4) as usize] |= (b as u32) << ((i % 4) * 8);
        i += 1;
    }
    true
}

fn sfr_lock(freq: u32) -> bool {
    const MUTEX: usize = 0x01f4_3000;
    let end = time::cntvct() + if freq == 0 { 1 } else { freq as u64 };
    loop {
        wr32(MUTEX, 1);
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        if rd32(MUTEX) == 1 {
            return true;
        }
        if time::cntvct() >= end {
            return false;
        }
    }
}

fn sfr_unlock() {
    wr32(0x01f4_3000, 0);
}

/// 只清这一项的载荷。下半区先丢掉旧行再写再刷回；上半区只作废这一项，
/// 不碰到 0x80AFE000。调用时 TOC 那一页已经是 NC。
fn sfr_zero_item(at: u64, n: u32) -> bool {
    let n = (n as u64).min(256);
    if n == 0 || !smem_window(at, n) {
        return false;
    }
    let start = at & !63;
    let end = (at + n + 63) & !63;
    if end > 0x80AFE000 || start < 0x8090_0000 {
        return false;
    }
    if at < 0x80A0_0000 && at + n > 0x80A0_0000 {
        return false;
    }
    if at < 0x80A0_0000 {
        dcache_ivac(at, n);
    } else {
        dcache_civac(at, n);
    }
    let mut i = 0u64;
    while i < n {
        unsafe {
            (at as *mut u8).add(i as usize).write_volatile(0);
        }
        i += 1;
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    if at < 0x80A0_0000 {
        dcache_civac(at, n);
    }
    true
}

/// `qcom_smem_get(QCOM_SMEM_HOST_ANY, 421)`。版本 12 走全局分区
/// （host 0xfffe），链表格式和私有分区一样。只读，用完放开 TCSR 锁 3。
fn sfr_read(freq: u32) {
    unsafe { SFR = sfr_load(freq, 421); }
}

fn sfr_load(freq: u32, want: u32) -> Sfr {
    unsafe {
        core::arch::asm!("msr daifclr, #8", "isb", options(nostack));
    }
    const SMEM_BASE: u64 = 0x8090_0000;
    dcache_ivac(SMEM_BASE, 0x100);
    let init = rd32((SMEM_BASE + 0xC0) as usize);
    let ver = rd32((SMEM_BASE + 0x5C) as usize);
    if init != 1 {
        return Sfr {
            st: 0xb,
            sz: 0,
            gp: 0,
            at: 0,
            c: 0,
            w: [0; 32],
        };
    }
    if ver >> 16 != 12 {
        return Sfr {
            st: 0xa,
            sz: 0,
            gp: 0,
            at: 0,
            c: 0,
            w: [0; 32],
        };
    }
    let toc = SMEM_BASE + 0x20_0000 - 0x1000;
    let index = ((0x80A0_0000u64 - 0x8000_0000) >> 21) as usize;
    let slot = unsafe { (core::ptr::addr_of_mut!(L2) as *mut u64).add(index) };
    let old = unsafe { slot.read_volatile() };
    l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
    let got = sfr_in_map(freq, toc, want, false);
    l2_swap(index, old, toc >> 12);
    got
}

fn sfr_in_map(freq: u32, toc: u64, want: u32, wipe: bool) -> Sfr {
    let mut r = Sfr {
        st: 9,
        sz: 0,
        gp: 0,
        at: 0,
        c: 0,
        w: [0; 32],
    };
    let Some(magic) = smem_rd32(toc) else {
        r.st = 6;
        return r;
    };
    if magic != 0x434f_5424 {
        return r;
    }
    let Some(pt_ver) = smem_rd32(toc + 4) else {
        r.st = 6;
        return r;
    };
    if pt_ver != 1 {
        return r;
    }
    let Some(nent) = smem_rd32(toc + 8) else {
        r.st = 6;
        return r;
    };
    let fit = (0x1000u32 - 32) / 48;
    let n = nent.min(fit);
    let mut gp_off = 0u32;
    let mut gp_sz = 0u32;
    let mut gp_cache = 0u32;
    let mut e = toc + 32;
    for _ in 0..n {
        let Some(off) = smem_rd32(e) else {
            r.st = 6;
            return r;
        };
        let Some(sz) = smem_rd32(e + 4) else {
            r.st = 6;
            return r;
        };
        let Some(hosts) = smem_rd32(e + 12) else {
            r.st = 6;
            return r;
        };
        let Some(cache) = smem_rd32(e + 16) else {
            r.st = 6;
            return r;
        };
        e += 48;
        if (hosts & 0xffff) == 0xfffe && (hosts >> 16) == 0xfffe {
            gp_off = off;
            gp_sz = sz;
            gp_cache = cache;
            break;
        }
    }
    if gp_off == 0 || gp_sz < 32 {
        r.st = 3;
        return r;
    }
    r.gp = gp_off;
    if gp_off.checked_add(gp_sz).is_none_or(|end| end > 0x20_0000) {
        r.st = 6;
        return r;
    }
    let mut item_count = 512u32;
    let info = 32u64 + nent as u64 * 48;
    if info + 18 <= 0x1000 {
        if smem_rd32(toc + info) == Some(0x4949_4953) {
            if let Some(w) = smem_rd32(toc + info + 16) {
                item_count = (w & 0xffff).max(1);
            }
        }
    }
    if want >= item_count {
        r.st = 8;
        return r;
    }
    let base = 0x8090_0000u64 + gp_off as u64;
    let Some(hm) = smem_rd32(base) else {
        r.st = 6;
        return r;
    };
    if hm != 0x5452_5024 {
        r.st = 5;
        return r;
    }
    let Some(hh) = smem_rd32(base + 4) else {
        r.st = 6;
        return r;
    };
    let h0 = hh & 0xffff;
    let h1 = hh >> 16;
    if h0 != 0xfffe && h1 != 0xfffe {
        r.st = 5;
        return r;
    }
    let Some(hsz) = smem_rd32(base + 8) else {
        r.st = 6;
        return r;
    };
    if hsz != gp_sz {
        r.st = 5;
        return r;
    }
    if !sfr_lock(freq) {
        r.st = 7;
        return r;
    }
    let mut walked = sfr_walk(base, gp_sz, gp_cache, want as u16);
    if wipe && (walked.st == 0 || walked.st == 1) {
        let n = walked.sz.min(256);
        if sfr_zero_item(walked.at as u64, n) {
            walked.st = if walked.st == 0 { 0x10 } else { 0x11 };
        } else {
            walked.st = 0x12;
        }
    }
    sfr_unlock();
    walked.gp = gp_off;
    walked
}

/// PAS 之前把 423 清成空串。返回值的 `w` 仍是清掉之前的字节。
fn sfr_wipe(freq: u32, want: u32) -> Sfr {
    unsafe {
        core::arch::asm!("msr daifclr, #8", "isb", options(nostack));
    }
    const SMEM_BASE: u64 = 0x8090_0000;
    dcache_ivac(SMEM_BASE, 0x100);
    let init = rd32((SMEM_BASE + 0xC0) as usize);
    let ver = rd32((SMEM_BASE + 0x5C) as usize);
    if init != 1 || ver >> 16 != 12 {
        return Sfr {
            st: 0xb,
            sz: 0,
            gp: 0,
            at: 0,
            c: 0,
            w: [0; 32],
        };
    }
    let toc = SMEM_BASE + 0x20_0000 - 0x1000;
    let index = ((0x80A0_0000u64 - 0x8000_0000) >> 21) as usize;
    let slot = unsafe { (core::ptr::addr_of_mut!(L2) as *mut u64).add(index) };
    let old = unsafe { slot.read_volatile() };
    l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
    let got = sfr_in_map(freq, toc, want, true);
    l2_swap(index, old, toc >> 12);
    got
}

fn sfr_hit(r: &mut Sfr, at: u64, entry_size: u32, cached: u32) -> bool {
    r.sz = entry_size;
    r.at = at as u32;
    r.c = cached;
    if entry_size == 0 || !smem_window(at, 1) {
        r.st = 6;
        return true;
    }
    let Some(b0) = smem_rd8(at) else {
        r.st = 6;
        return true;
    };
    r.st = if b0 == 0 { 1 } else { 0 };
    if !sfr_words(at, entry_size, &mut r.w) {
        r.st = 6;
    }
    true
}

/// 先未缓存链表，再缓存链表。对得上 `qcom_smem_get_private`。
fn sfr_walk(base: u64, part_sz: u32, cacheline: u32, want: u16) -> Sfr {
    let mut r = Sfr {
        st: 2,
        sz: 0,
        gp: 0,
        at: 0,
        c: 0,
        w: [0; 32],
    };
    let Some(free_u) = smem_rd32(base + 12) else {
        r.st = 6;
        return r;
    };
    let Some(free_c) = smem_rd32(base + 16) else {
        r.st = 6;
        return r;
    };
    if free_u > part_sz || free_c > part_sz || free_u > free_c || free_u < 32 {
        r.st = 4;
        return r;
    }
    let uncached_end = base + free_u as u64;
    let cached_end = base + free_c as u64;
    let p_end = base + part_sz as u64;
    let mut p = base + 32;
    let mut steps = 0u32;
    while p < uncached_end && p + 16 < uncached_end && steps < 512 {
        steps += 1;
        let Some(w0) = smem_rd32(p) else {
            r.st = 6;
            return r;
        };
        let Some(esz) = smem_rd32(p + 4) else {
            r.st = 6;
            return r;
        };
        let Some(pads) = smem_rd32(p + 8) else {
            r.st = 6;
            return r;
        };
        if (w0 as u16) != 0xa5a5 {
            r.st = 4;
            return r;
        }
        let item = (w0 >> 16) as u16;
        let pad_data = pads as u16;
        let pad_hdr = (pads >> 16) as u16;
        let Some(next) = p
            .checked_add(16)
            .and_then(|v| v.checked_add(pad_hdr as u64))
            .and_then(|v| v.checked_add(esz as u64))
        else {
            r.st = 4;
            return r;
        };
        if next <= p || next > uncached_end {
            r.st = 4;
            return r;
        }
        if item == want {
            if esz >= part_sz || (pad_data as u32) >= esz {
                r.st = 4;
                return r;
            }
            let entry_size = esz - pad_data as u32;
            let item_at = p + 16 + pad_hdr as u64;
            if item_at < p || item_at + entry_size as u64 > uncached_end {
                r.st = 4;
                return r;
            }
            sfr_hit(&mut r, item_at, entry_size, 0);
            return r;
        }
        p = next;
    }
    if steps >= 512 {
        r.st = 4;
        return r;
    }
    if cached_end == p_end {
        return r;
    }
    if cacheline == 0 || cacheline > 128 || cacheline & (cacheline - 1) != 0 {
        r.st = 4;
        return r;
    }
    let hdr_slot = (16u32 + cacheline - 1) & !(cacheline - 1);
    let Some(first) = p_end.checked_sub(hdr_slot as u64) else {
        r.st = 4;
        return r;
    };
    if first < cached_end || first + 16 > p_end {
        r.st = 4;
        return r;
    }
    p = first;
    steps = 0;
    while p > cached_end && steps < 512 {
        steps += 1;
        let Some(w0) = smem_rd32(p) else {
            r.st = 6;
            return r;
        };
        let Some(esz) = smem_rd32(p + 4) else {
            r.st = 6;
            return r;
        };
        let Some(pads) = smem_rd32(p + 8) else {
            r.st = 6;
            return r;
        };
        if (w0 as u16) != 0xa5a5 {
            r.st = 4;
            return r;
        }
        let item = (w0 >> 16) as u16;
        let pad_data = pads as u16;
        let Some(item_at) = p.checked_sub(esz as u64) else {
            r.st = 4;
            return r;
        };
        let Some(next) = item_at.checked_sub(hdr_slot as u64) else {
            r.st = 4;
            return r;
        };
        if next >= p {
            r.st = 4;
            return r;
        }
        if item == want {
            if esz >= part_sz || (pad_data as u32) >= esz {
                r.st = 4;
                return r;
            }
            let entry_size = esz - pad_data as u32;
            if item_at < cached_end || item_at + entry_size as u64 > p {
                r.st = 4;
                return r;
            }
            sfr_hit(&mut r, item_at, entry_size, 1);
            return r;
        }
        p = next;
    }
    if steps >= 512 {
        r.st = 4;
    }
    r
}

/// 先解开 SError，再读头两个字。区尾 TOC 整页用 NC 读：
/// 这一页的 WB 读会把核冻死。只找 host 0/1 的分区偏移，不走进分区。
fn smem_scan(ufs: &mut Ufs, do_glink: bool) {
    unsafe {
        core::arch::asm!("msr daifclr, #8", "isb", options(nostack));
    }
    const SMEM_BASE: u64 = 0x8090_0000;
    dcache_ivac(SMEM_BASE, 64);
    let init = rd32((SMEM_BASE + 0xC0) as usize);
    let ver = rd32((SMEM_BASE + 0x5C) as usize);
    let toc = SMEM_BASE + 0x20_0000 - 0x1000;
    let index = ((0x80A0_0000u64 - 0x8000_0000) >> 21) as usize;
    let slot = unsafe { (core::ptr::addr_of_mut!(L2) as *mut u64).add(index) };
    let old = unsafe { slot.read_volatile() };
    l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
    let magic = rd32(toc as usize);
    let mut nent = 0u32;
    let mut part = 0u32;
    let mut g = 0u32;
    if magic == 0x434f_5424 {
        nent = rd32((toc + 8) as usize);
        let n = nent.min(48);
        let mut e = toc + 32;
        for _ in 0..n {
            let off = rd32(e as usize);
            let sz = rd32((e + 4) as usize);
            let h0 = rd16a(e + 12) as u32;
            let h1 = rd16a(e + 14) as u32;
            e += 48;
            if off == 0 || sz < 32 || off >= 0x20_0000 || off + sz > 0x20_0000 {
                continue;
            }
            if (h0 == 0 && h1 == 1) || (h0 == 1 && h1 == 0) {
                part = off;
                g |= 1 << 7;
                break;
            }
        }
    } else {
        nent = magic;
    }
    // 分区头在前一个 WB 块。项可以跨进当前这块 NC。先扫完再把描述符换回去。
    let mut free_u = 0u32;
    if part != 0 && part + 64 < 0x10_0000 {
        let base = SMEM_BASE + part as u64;
        dcache_ivac(base, 64);
        if rd32(base as usize) == 0x5452_5024 {
            g |= 1 << 6;
            let psz = rd32((base + 8) as usize);
            free_u = rd32((base + 12) as usize);
            let room = 0x20_0000 - part;
            let end_off = free_u.min(psz).min(room);
            // 下一块 NC 只覆盖到 0x80C00000，再往前不读。
            let nc_end = 0x80C0_0000u64;
            let mut at435 = 0u64;
            let mut sz435 = 0u64;
            let mut at478 = 0u64;
            let mut sz478 = 0u64;
            let mut at479 = 0u64;
            let mut sz479 = 0u64;
            let mut at480 = 0u64;
            let mut sz480 = 0u64;
            let mut boff = 0u32;
            let mut b0 = 0u32;
            let mut b1 = 0u32;
            let mut blast = 0u32;
            if end_off > 32 {
                let wb_len = end_off.min(0x10_0000 - part);
                if wb_len > 32 {
                    dcache_ivac(base, wb_len as u64);
                }
                let end = base + end_off as u64;
                let mut p = base + 32;
                for _ in 0..128 {
                    if p + 16 > end || p + 16 > nc_end || (p < 0x80A0_0000 && p + 16 > 0x80A0_0000)
                    {
                        boff = (p - base) as u32;
                        break;
                    }
                    if rd16a(p) != 0xa5a5 {
                        g |= 1 << 5;
                        boff = (p - base) as u32;
                        b0 = rd32(p as usize);
                        b1 = rd32((p + 4) as usize);
                        break;
                    }
                    let item = rd16a(p + 2);
                    let isz = rd32((p + 4) as usize) as u64;
                    let pad = rd16a(p + 10) as u64;
                    let data = p + 16 + pad;
                    match item {
                        435 => {
                            g |= 1 << 0;
                            at435 = data;
                            sz435 = isz;
                        }
                        428 => g |= 1 << 1,
                        478 => {
                            g |= 1 << 2;
                            at478 = data;
                            sz478 = isz;
                        }
                        479 => {
                            g |= 1 << 3;
                            at479 = data;
                            sz479 = isz;
                        }
                        480 => {
                            g |= 1 << 4;
                            at480 = data;
                            sz480 = isz;
                        }
                        _ => {}
                    }
                    let next = p + 16 + pad + isz;
                    if next <= p || isz > 0x2_0000 {
                        g |= 1 << 5;
                        boff = (p - base) as u32;
                        b0 = rd32(p as usize);
                        b1 = isz as u32;
                        break;
                    }
                    blast = item as u32;
                    p = next;
                }
            }
            let (kcode, new_free, cached) = smp_publish(base, free_u, g, boff, b0, b1);
            if kcode == 1 {
                free_u = new_free;
                g |= 1 << 1;
            } else if unsafe { MPSS_PUB } == 0xffff_ffff {
                unsafe { MPSS_PUB = kcode };
            }
            let gcode = if do_glink {
                glink_version(
                    base, &mut free_u, &mut g, at478, sz478, at479, sz479, at480, sz480,
                )
            } else {
                0
            };
            let (mut slave, mut smpm, mut d0, mut d1, mut d2, mut d3, mut rx0, mut ev, mut n0) =
                smem_payloads(at435, sz435, at478, sz478, at480, sz480);
            if do_glink {
                let mut round = 0u32;
                loop {
                    pd_reply(at478, at479, sz479);
                    let before = unsafe { RF_N };
                    rf_reply(at478, at479, sz479);
                    tf_reply(ufs, at478, at479, sz479);
                    pm_reply(at478, at479, sz479);
                    sr_tick(at478, at479, sz479);
                    ipa_init_tick(at478, at479, sz479);
                    ipa_cmplt_tick(at478, at479, sz479);
                    ipa_cfg_tick(at478, at479, sz479);
                    ipa_ir_tick(at478, at479, sz479);
                    ipa_ind_tick(at478, at479, sz479);
                    round += 1;
                    // Linux：fs1/fs2/fsg/fsc 连续 OPEN 后再 ALLOC。
                    // 发送环不空就停；没答的留在队列里，接收 tail 不推过队头。
                    if unsafe { RF_N } == before || round >= 8 {
                        break;
                    }
                    let next = smem_payloads(at435, sz435, at478, sz478, at480, sz480);
                    slave = next.0;
                    smpm = next.1;
                    d0 = next.2;
                    d1 = next.3;
                    d2 = next.4;
                    d3 = next.5;
                    rx0 = next.6;
                    ev = next.7;
                    n0 = next.8;
                }
                glink_resume(at478, at479, sz479);
                glink_rx_reuse(at478, at479, sz479);
                glink_rx_advance(at478, unsafe { GLINK_RXLEN });
            }
            l2_swap(index, old, toc >> 12);
            unsafe {
                SMEM_SNAP = SmemSnap {
                    init,
                    ver,
                    nent,
                    part,
                    free: free_u,
                    g,
                    slave,
                    smpm,
                    d0,
                    d1,
                    d2,
                    d3,
                    rx0,
                    boff,
                    b0,
                    b1,
                    blast,
                    k: kcode,
                    cached,
                    ev,
                    n0,
                    gk: gcode,
                };
            }
            return;
        }
    }
    l2_swap(index, old, toc >> 12);
    unsafe {
        SMEM_SNAP = SmemSnap {
            init,
            ver,
            nent,
            part,
            free: free_u,
            g,
            slave: 0,
            smpm: 0,
            d0: 0,
            d1: 0,
            d2: 0,
            d3: 0,
            rx0: 0,
            boff: 0,
            b0: 0,
            b1: 0,
            blast: 0,
            k: 0,
            cached: 0,
            ev: 0,
            n0: 0,
            gk: 0,
        };
    }
}

/// 描述符 478 已在、发送 FIFO 479 不在时，按 glink_smem 分配 16KB FIFO，
/// 写入 VERSION（cmd 0，版本 1，特性 INTENT_REUSE），把头拨到 8，
/// 再踢 IPCC signal 0。返回 1 表示这次发出去了。
fn glink_version(
    base: u64,
    free_u: &mut u32,
    g: &mut u32,
    at478: u64,
    sz478: u64,
    at479: u64,
    sz479: u64,
    at480: u64,
    sz480: u64,
) -> u32 {
    if *g & 4 == 0 || sz478 < 16 || at478 % 4 != 0 {
        return 0;
    }
    let head = rd32((at478 + 4) as usize);
    if head != 0 {
        return glink_open_ack(at478, at479, sz479, at480, sz480);
    }
    let mut fifo = at479;
    if *g & 8 == 0 {
        let cached = rd32((base + 16) as usize);
        const ALLOC: u32 = 0x4010;
        if free_u.saturating_add(ALLOC) > cached {
            return 2;
        }
        const MUTEX: usize = 0x01f4_3000;
        let mut got = false;
        for _ in 0..256 {
            wr32(MUTEX, 1);
            unsafe {
                core::arch::asm!("dsb sy", options(nostack));
            }
            if rd32(MUTEX) == 1 {
                got = true;
                break;
            }
        }
        if !got {
            return 3;
        }
        let now = rd32((base + 12) as usize);
        if now != *free_u {
            wr32(MUTEX, 0);
            return 4;
        }
        let hdr = base + *free_u as u64;
        // item 479 = 0x1DF，长度 0x4000，padding 0。
        wr32(hdr as usize, 0x01df_a5a5);
        wr32((hdr + 4) as usize, 0x4000);
        wr32((hdr + 8) as usize, 0);
        wr32((hdr + 12) as usize, 0);
        fifo = hdr + 16;
        let new_free = *free_u + ALLOC;
        wr32((base + 12) as usize, new_free);
        dcache_civac(base, 64);
        wr32(MUTEX, 0);
        *free_u = new_free;
        *g |= 1 << 3;
    } else if sz479 < 8 || fifo % 4 != 0 {
        return 0;
    }
    if fifo < 0x8090_0000 || fifo + 8 > 0x80B0_0000 {
        return 0;
    }
    // cmd=0，param1=1，param2=INTENT_REUSE。
    wr32(fifo as usize, 0x0001_0000);
    wr32((fifo + 4) as usize, 1);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32((at478 + 4) as usize, 8);
    if at478 < 0x80A0_0000 {
        dcache_civac(at478, 16);
    } else {
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
    }
    wr32(0x4080_0c, 2 << 16);
    1
}

/// 对接收 FIFO 里每个 OPEN 回 OPEN_ACK。rcid 是 OPEN 的 param1。
fn glink_open_ack(at478: u64, at479: u64, sz479: u64, at480: u64, sz480: u64) -> u32 {
    if at479 == 0 || at480 == 0 || sz479 < 8 || sz480 < 8 {
        return 0;
    }
    if unsafe { GLINK_ACKED } {
        return glink_hello(at478, at479, sz479, at480, sz480);
    }
    let rx_tail = rd32((at478 + 8) as usize) as u64;
    let rx_head = rd32((at478 + 12) as usize) as u64;
    // 活体 rx_head 已经是 0x2b0。超过 0x80 就直接返回的话，OPEN_ACK 永远不会做。
    if rx_head <= rx_tail || rx_head > sz480 {
        return 0;
    }
    let mut rcid = [0u32; 4];
    let mut nack = 0usize;
    let mut off = rx_tail;
    while off + 8 <= rx_head && nack < 4 {
        let w0 = rd32((at480 + off) as usize);
        let w1 = rd32((at480 + off + 4) as usize);
        let cmd = w0 & 0xffff;
        let param1 = w0 >> 16;
        let param2 = w1 & 0xffff;
        let step = match cmd {
            0 | 1 => 8,
            2 => {
                if nack < 4 {
                    rcid[nack] = param1;
                    nack += 1;
                }
                (8 + param2 as u64 + 7) & !7
            }
            _ => break,
        };
        if step == 0 || off + step > rx_head {
            break;
        }
        off += step;
    }
    if nack == 0 {
        return 0;
    }
    let mut tx = rd32((at478 + 4) as usize) as u64;
    // 三条 OPEN：glink_ssr 24 字节，IPCRTR 16，SSM_RTR_MODEM_APPS 32。
    if tx + (nack as u64) * 8 + 72 > sz479 {
        return 0;
    }
    for id in rcid.iter().take(nack) {
        // OPEN_ACK = 4，param2 = 0。
        wr32((at479 + tx) as usize, 4 | (*id << 16));
        wr32((at479 + tx + 4) as usize, 0);
        tx += 8;
    }
    // 本地 cid 1/2/3。远端发起打开之后，主机还要自己再 OPEN 一次。
    tx = glink_put_open(at479, tx, 1, b"glink_ssr\0");
    tx = glink_put_open(at479, tx, 2, b"IPCRTR\0");
    tx = glink_put_open(at479, tx, 3, b"SSM_RTR_MODEM_APPS\0");
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32((at478 + 4) as usize, tx as u32);
    if at478 < 0x80A0_0000 {
        dcache_civac(at478, 16);
    } else {
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
    }
    if at479 < 0x80A0_0000 {
        dcache_civac(at479, tx);
    }
    wr32(0x4080_0c, 2 << 16);
    unsafe { GLINK_ACKED = true; }
    6
}

/// IPCRTR 上的 QRTR HELLO。要等对方先给这个通道一个够大的 intent。
fn glink_hello(at478: u64, at479: u64, sz479: u64, at480: u64, sz480: u64) -> u32 {
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    let rx_head = rd32((at478 + 12) as usize) as u64;
    if tx_head != tx_tail || rx_head <= 0x58 || rx_head > sz480 {
        return 0;
    }
    let mut off = 0x58u64;
    let mut iid = 0u32;
    let mut found = false;
    let mut guard = 0u32;
    while off + 8 <= rx_head && guard < 80 {
        guard += 1;
        let w0 = rd32((at480 + off) as usize);
        let cmd = w0 & 0xffff;
        let lcid = w0 >> 16;
        let step = if cmd == 0
            || cmd == 1
            || cmd == 4
            || cmd == 6
            || cmd == 7
            || cmd == 8
            || cmd == 13
            || cmd == 14
            || cmd == 15
        {
            8
        } else if cmd == 2 {
            let nlen = rd32((at480 + off + 4) as usize) & 0xffff;
            if nlen > 64 {
                break;
            }
            (8 + nlen as u64 + 7) & !7
        } else if cmd == 5 && off + 16 <= rx_head {
            let count = rd32((at480 + off + 4) as usize);
            let size = rd32((at480 + off + 8) as usize);
            let id = rd32((at480 + off + 12) as usize);
            if qrtr_lcid(lcid) && count >= 1 && size > unsafe { SEEN_ISZ } {
                unsafe { SEEN_ISZ = size; }
            }
            if qrtr_lcid(lcid) && count >= 1 && size >= 52 && (!found || size > unsafe { LAST_ISZ }) {
                iid = id;
                found = true;
                unsafe {
                    LAST_IID = id;
                    LAST_ISZ = size;
                    HAVE_IID = true;
                }
            }
            16
        } else if cmd == 9 {
            let chunk = rd32((at480 + off + 8) as usize) as u64;
            if chunk > 512 || off + 16 + chunk > rx_head + 16 {
                break;
            }
            (16 + chunk + 7) & !7
        } else {
            break;
        };
        if step < 8 {
            break;
        }
        off += step;
    }
    if !unsafe { GLINK_INTENTS } {
        if tx_head + 160 > sz479 {
            return 0;
        }
        let mut p = at479 + tx_head;
        let sizes = [0x800u32, 0x800, 0x800, 0x800, 0x800, 0x2000, 0x2000, 0x2000, 0x4400, 0x4400];
        for (i, sz) in sizes.iter().enumerate() {
            wr32(p as usize, 5 | (2 << 16));
            wr32((p + 4) as usize, 1);
            wr32((p + 8) as usize, *sz);
            wr32((p + 12) as usize, (i as u32) + 1);
            p += 16;
        }
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        wr32((at478 + 4) as usize, (tx_head + 160) as u32);
        if at478 < 0x80A0_0000 {
            dcache_civac(at478, 16);
        }
        wr32(0x4080_0c, 2 << 16);
        unsafe { GLINK_INTENTS = true; }
        return 8;
    }
    if unsafe { GLINK_HELLO } {
        if !found || tx_head + 72 > sz479 {
            return 0;
        }
        let use_id = iid;
        // LOOKUP 应答优先：对端已在找本机服务。
        let pend = unsafe { LOOK_PEND };
        if pend != 0 {
            let svc = pend & 0xffff;
            let inst_q = pend >> 16;
            if let Some((inst, port)) = host_svc_port(svc, inst_q) {
                glink_tx_qrtr(at478, at479, tx_head, use_id, 4, svc, inst, 1, port);
                unsafe { LOOK_PEND = 0; }
                return 14;
            }
            unsafe { LOOK_PEND = 0; }
        }
        if unsafe { HOST_ADV } < 5 {
            // qrtr.c：对方 HELLO 之后推本机服务。一次一包（一 intent）；0x35 打头。
            // 第五个是 0x31，实例字 0x101。TCP 已能连第二次。
            if unsafe { HELLO_RX } > 0 {
                glink_announce(at478, at479, tx_head, sz479, use_id);
            }
            return 13;
        }
        if unsafe { !GLINK_LOOKUP } {
            // icnss_qmi.c：0x45，版本 1，实例 0，线上实例字 1。
            glink_tx_qrtr(at478, at479, tx_head, use_id, 10, 0x45, 1, 0, 0);
            unsafe { GLINK_LOOKUP = true; }
            return 9;
        }
        if unsafe { !GLINK_LOC } {
            // service-locator.c：0x40，版本 1，实例 1，线上实例字 0x101。
            glink_tx_qrtr(at478, at479, tx_head, use_id, 10, 0x40, 0x101, 0, 0);
            unsafe { GLINK_LOC = true; }
            return 11;
        }
        return 0;
    }
    if !found || tx_head + 72 > sz479 {
        return 0;
    }
    let hello_at = at479 + tx_head;
    let mut i = 0u64;
    while i < 72 {
        wr32((hello_at + i) as usize, 0);
        i += 4;
    }
    // TX_DATA，lcid 2，intent id。
    wr32(hello_at as usize, 9 | (2 << 16));
    wr32((hello_at + 4) as usize, iid);
    wr32((hello_at + 8) as usize, 52);
    wr32((hello_at + 12) as usize, 0);
    // qrtr_hdr_v1：版本 1，HELLO，源节点 1，控制口，目标广播节点。
    wr32((hello_at + 16) as usize, 1);
    wr32((hello_at + 20) as usize, 2);
    wr32((hello_at + 24) as usize, 1);
    wr32((hello_at + 28) as usize, 0xffff_fffe);
    wr32((hello_at + 32) as usize, 0);
    wr32((hello_at + 36) as usize, 20);
    wr32((hello_at + 40) as usize, 0xffff_ffff);
    wr32((hello_at + 44) as usize, 0xffff_fffe);
    // qrtr_ctrl_pkt：cmd = HELLO，其余 0。
    wr32((hello_at + 48) as usize, 2);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32((at478 + 4) as usize, (tx_head + 72) as u32);
    if at478 < 0x80A0_0000 {
        dcache_civac(at478, 16);
    } else {
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
    }
    wr32(0x4080_0c, 2 << 16);
    unsafe { GLINK_HELLO = true; }
    7
}

/// pm-service 0x35 最先（对齐 Linux qrtr-lookup inst=0x1001 port=16407），
/// 再 rmtfs 14、tftp 4096、pd-mapper 64。一次一包。节点 1。
fn glink_announce(at478: u64, at479: u64, head: u64, sz479: u64, iid: u32) {
    let n = unsafe { HOST_ADV };
    if n >= 5 || head + 72 > sz479 {
        return;
    }
    let (svc, inst, port) = if n == 0 {
        // Linux：svc=0x35 inst=0x1001 at 1:16407（非旧探针 1/0x13）。
        (PM_SVC, PM_INST, PM_QRTR_PORT)
    } else if n == 1 {
        (14, 1, 0x10)
    } else if n == 2 {
        (4096, 1, 0x11)
    } else if n == 3 {
        (64, 257, 0x12)
    } else {
        // qmi_send_new_server：version | (instance << 8) = 0x101。
        // slot-b 在发 INIT 的同一时刻已经收到 INDICATION_REGISTER。
        (0x31, IPA_A5_INST, IPA_SVC_PORT)
    };
    glink_tx_qrtr(at478, at479, head, iid, 4, svc, inst, 1, port);
    unsafe { HOST_ADV = n + 1; }
}

/// 本机可公告的服务：LOOKUP 命中则立刻回 NEW_SERVER（对齐 qrtr ns）。
fn host_svc_port(svc: u32, inst: u32) -> Option<(u32, u32)> {
    // inst：对端 LOOKUP 常带精确实例字，或 0 表示通配。
    if svc == PM_SVC && (inst == 0 || inst == PM_INST || inst == 1) {
        Some((PM_INST, PM_QRTR_PORT))
    } else if svc == 14 && (inst == 0 || inst == 1) {
        Some((1, 0x10))
    } else if svc == 4096 && (inst == 0 || inst == 1) {
        Some((1, 0x11))
    } else if svc == 64 && (inst == 0 || inst == 257) {
        Some((257, 0x12))
    } else if svc == 0x31 && (inst == 0 || inst == 1 || inst == IPA_A5_INST) {
        Some((IPA_A5_INST, IPA_SVC_PORT))
    } else {
        None
    }
}

fn glink_tx_qrtr(
    at478: u64,
    at479: u64,
    tx_head: u64,
    iid: u32,
    typ: u32,
    svc: u32,
    inst: u32,
    node: u32,
    port: u32,
) {
    let p = at479 + tx_head;
    let mut i = 0u64;
    while i < 72 {
        wr32((p + i) as usize, 0);
        i += 4;
    }
    wr32(p as usize, 9 | (2 << 16));
    wr32((p + 4) as usize, iid);
    wr32((p + 8) as usize, 52);
    wr32((p + 12) as usize, 0);
    wr32((p + 16) as usize, 1);
    wr32((p + 20) as usize, typ);
    wr32((p + 24) as usize, 1);
    wr32((p + 28) as usize, 0xffff_fffe);
    wr32((p + 32) as usize, 0);
    wr32((p + 36) as usize, 20);
    wr32((p + 40) as usize, 0);
    wr32((p + 44) as usize, 0xffff_fffe);
    wr32((p + 48) as usize, typ);
    wr32((p + 52) as usize, svc);
    wr32((p + 56) as usize, inst);
    wr32((p + 60) as usize, node);
    wr32((p + 64) as usize, port);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32((at478 + 4) as usize, (tx_head + 72) as u32);
    if at478 < 0x80A0_0000 {
        dcache_civac(at478, 16);
    }
    wr32(0x4080_0c, 2 << 16);
}

/// qrtr_recvmsg：confirm_rx 置位就回 RESUME_TX。载荷是 client.node/port，即我们自己的地址。
fn glink_resume(at478: u64, at479: u64, sz479: u64) {
    if at478 == 0 || at479 == 0 || unsafe { !CF_PEND || !HAVE_IID } {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail || tx_head + 64 > sz479 {
        return;
    }
    let p = at479 + tx_head;
    let mut i = 0u64;
    while i < 64 {
        wr32((p + i) as usize, 0);
        i += 4;
    }
    wr32(p as usize, 9 | (2 << 16));
    wr32((p + 4) as usize, unsafe { LAST_IID });
    wr32((p + 8) as usize, 44);
    wr32((p + 16) as usize, 1);
    wr32((p + 20) as usize, 7);
    wr32((p + 24) as usize, unsafe { CF_DN });
    wr32((p + 28) as usize, unsafe { CF_DP });
    wr32((p + 36) as usize, 12);
    wr32((p + 40) as usize, unsafe { CF_SN });
    wr32((p + 44) as usize, unsafe { CF_SP });
    wr32((p + 48) as usize, 7);
    wr32((p + 52) as usize, unsafe { CF_DN });
    wr32((p + 56) as usize, unsafe { CF_DP });
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32((at478 + 4) as usize, (tx_head + 64) as u32);
    if at478 < 0x80A0_0000 {
        dcache_civac(at478, 16);
    }
    wr32(0x4080_0c, 2 << 16);
    unsafe {
        CF_PEND = false;
        CF_N = CF_N.wrapping_add(1);
    }
}

struct Dom<'a> {
    svc: &'a [u8],
    dom: &'a [u8],
    inst: u32,
}

/// 六个 jsn 按文件名序。服务名是 provider/service，域是 soc/domain/subdomain。
const DOMS: &[Dom<'static>] = &[
    Dom { svc: b"tms/servreg", dom: b"msm/adsp/root_pd", inst: 74 },
    Dom { svc: b"tms/servreg", dom: b"msm/adsp/sensor_pd", inst: 74 },
    Dom { svc: b"tms/servreg", dom: b"msm/adsp/audio_pd", inst: 74 },
    Dom { svc: b"avs/audio", dom: b"msm/adsp/audio_pd", inst: 74 },
    Dom { svc: b"tms/servreg", dom: b"msm/cdsp/root_pd", inst: 76 },
    Dom { svc: b"tms/servreg", dom: b"msm/modem/root_pd", inst: 180 },
    Dom { svc: b"gps/gps_service", dom: b"msm/modem/root_pd", inst: 180 },
    Dom { svc: b"kernel/elf_loader", dom: b"msm/modem/wlan_pd", inst: 180 },
    Dom { svc: b"tms/servreg", dom: b"msm/modem/wlan_pd", inst: 180 },
    Dom { svc: b"wlan/fw", dom: b"msm/modem/wlan_pd", inst: 180 },
];

fn put_u8(buf: &mut [u8], n: &mut usize, b: u8) {
    if *n < buf.len() {
        buf[*n] = b;
        *n += 1;
    }
}

fn put_u16(buf: &mut [u8], n: &mut usize, v: u16) {
    put_u8(buf, n, v as u8);
    put_u8(buf, n, (v >> 8) as u8);
}

fn put_u32(buf: &mut [u8], n: &mut usize, v: u32) {
    put_u16(buf, n, v as u16);
    put_u16(buf, n, (v >> 16) as u16);
}

/// 嵌套字符串：1 字节 strlen，后面是字符，不带 NUL。再加实例和 service_data。
fn put_dom(buf: &mut [u8], n: &mut usize, dom: &[u8], inst: u32) {
    put_u8(buf, n, dom.len() as u8);
    for &b in dom {
        put_u8(buf, n, b);
    }
    put_u32(buf, n, inst);
    put_u8(buf, n, 0);
    put_u32(buf, n, 0);
}

/// pd-mapper 的 GET_DOMAIN_LIST 应答。对得上的名字带域列表，对不上则域数 0。
/// storage.c 的 partition_table，下标就是 caller_id。
const RFS_PATHS: &[&[u8]] = &[
    b"/boot/modem_fs1",
    b"/boot/modem_fs2",
    b"/boot/modem_fsc",
    b"/boot/modem_fsg",
    b"/boot/modem_study",
    b"/boot/modem_tunning",
    b"/boot/modem_tng",
    b"/oem/nvbk/static",
    b"/oem/nvbk/dynamic",
    b"/oppo/oem_partion",
];

/// TX_DATA 用掉的 intent 按 qcom_glink_native.c 的 RX_DONE_W_REUSE 还回去。
fn glink_rx_reuse(at478: u64, at479: u64, sz479: u64) {
    if at478 == 0 || at479 == 0 || sz479 < 16 {
        return;
    }
    let mut n = 0u32;
    while n < 16 {
        let left = unsafe { RX_LIID_N };
        if left == 0 || sz479 < 16 {
            break;
        }
        let tx_head = rd32((at478 + 4) as usize) as u64;
        let tx_tail = rd32(at478 as usize) as u64;
        let avail = if tx_tail <= tx_head {
            sz479 - tx_head + tx_tail
        } else {
            tx_tail - tx_head
        };
        if avail < 16 || tx_head + 8 > sz479 {
            break;
        }
        let liid = unsafe { RX_LIID[0] };
        let mut i = 1u32;
        while i < left {
            unsafe { RX_LIID[(i - 1) as usize] = RX_LIID[i as usize] };
            i += 1;
        }
        unsafe { RX_LIID_N = left - 1; }
        wr32((at479 + tx_head) as usize, 14 | (2 << 16));
        wr32((at479 + tx_head + 4) as usize, liid);
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        wr32((at478 + 4) as usize, (tx_head + 8) as u32);
        if at478 < 0x80A0_0000 {
            dcache_civac(at478, 16);
        }
        wr32(0x4080_0c, 2 << 16);
        unsafe { RX_DONE_N = RX_DONE_N.wrapping_add(1) };
        n += 1;
    }
}

fn glink_tx_payload(
    at478: u64,
    at479: u64,
    tx_head: u64,
    sz479: u64,
    src_port: u32,
    dst_node: u32,
    dst_port: u32,
    q: &[u8],
) -> bool {
    let n = q.len();
    // qrtr_endpoint_post：len 必须 4 字节对齐，且等于 ALIGN(size, 4) + 头长。
    // size 仍是真实 QMI 长度。OPEN 应答 21 字节，不补就被丢掉。
    let padded = (n + 3) & !3;
    let chunk = 32 + padded;
    let total = (16 + chunk + 7) & !7;
    let isz = unsafe { LAST_ISZ } as usize;
    // 扩容刀：载荷上限 7684（TFTP 4 头+7680 块），整包 8192。
    // 真协议闸是 isz（对端批的 intent）与环界；这两个帽只是本方缓冲界。
    if !unsafe { HAVE_IID } || n > 7684 || chunk > isz || total > 8192 {
        return false;
    }
    // 环尾放不下时回卷。各发送者都守 head==tail 单飞纪律，到这必是整环已排空。
    let mut head = tx_head;
    if head + total as u64 > sz479 {
        let tail = rd32(at478 as usize) as u64;
        if head == tail {
            head = 0;
        }
        if head + total as u64 > sz479 {
            return false;
        }
    }
    let p = at479 + head;
    let mut i = 0usize;
    while i < total {
        wr32((p + i as u64) as usize, 0);
        i += 4;
    }
    wr32(p as usize, 9 | (2 << 16));
    wr32((p + 4) as usize, unsafe { LAST_IID });
    wr32((p + 8) as usize, chunk as u32);
    wr32((p + 12) as usize, 0);
    wr32((p + 16) as usize, 1);
    wr32((p + 20) as usize, 1);
    wr32((p + 24) as usize, 1);
    wr32((p + 28) as usize, src_port);
    wr32((p + 32) as usize, 0);
    wr32((p + 36) as usize, n as u32);
    wr32((p + 40) as usize, dst_node);
    wr32((p + 44) as usize, dst_port);
    let mut o = 0usize;
    while o + 4 <= n {
        let w = q[o] as u32
            | (q[o + 1] as u32) << 8
            | (q[o + 2] as u32) << 16
            | (q[o + 3] as u32) << 24;
        wr32((p + 48 + o as u64) as usize, w);
        o += 4;
    }
    if o < n {
        let mut w = 0u32;
        let mut k = 0;
        while o + k < n {
            w |= (q[o + k] as u32) << (8 * k);
            k += 1;
        }
        wr32((p + 48 + o as u64) as usize, w);
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32((at478 + 4) as usize, (head + total as u64) as u32);
    if at478 < 0x80A0_0000 {
        dcache_civac(at478, 16);
    }
    wr32(0x4080_0c, 2 << 16);
    true
}

fn qmi_tlv_u32(fifo: u64, off: u64, chunk: u64, want: u8) -> Option<u32> {
    let w1 = rd32((fifo + off + 52) as usize);
    let msglen = ((w1 >> 8) & 0xffff) as u64;
    if chunk < 32 || msglen < 3 || 7 + msglen > chunk - 32 || msglen > 256 {
        return None;
    }
    let base = fifo + off + 55;
    let mut p = 0u64;
    while p + 3 <= msglen {
        let t = rd8(base + p);
        let ln = rd8(base + p + 1) as u64 | ((rd8(base + p + 2) as u64) << 8);
        p += 3;
        if p + ln > msglen {
            return None;
        }
        if t == want && ln == 4 {
            let mut v = 0u32;
            let mut k = 0u64;
            while k < 4 {
                v |= (rd8(base + p + k) as u32) << (8 * k);
                k += 1;
            }
            return Some(v);
        }
        p += ln;
    }
    None
}

fn qmi_tlv_u16(fifo: u64, off: u64, chunk: u64, want: u8) -> Option<u16> {
    let w1 = rd32((fifo + off + 52) as usize);
    let msglen = ((w1 >> 8) & 0xffff) as u64;
    if chunk < 32 || msglen < 3 || 7 + msglen > chunk - 32 || msglen > 256 {
        return None;
    }
    let base = fifo + off + 55;
    let mut p = 0u64;
    while p + 3 <= msglen {
        let t = rd8(base + p);
        let ln = rd8(base + p + 1) as u64 | ((rd8(base + p + 2) as u64) << 8);
        p += 3;
        if p + ln > msglen {
            return None;
        }
        if t == want && ln == 2 {
            let v = rd8(base + p) as u16 | ((rd8(base + p + 1) as u16) << 8);
            return Some(v);
        }
        p += ln;
    }
    None
}

fn rfs_caller(name: &[u8]) -> Option<u32> {
    let mut i = 0usize;
    while i < RFS_PATHS.len() {
        if RFS_PATHS[i] == name {
            // Android rmt_storage 回的是 fd，不会是 0。
            return Some((i as u32) + 1);
        }
        i += 1;
    }
    None
}

fn qmi_finish(q: &mut [u8], n: usize, txn: u32, msgid: u32) {
    qmi_finish_typed(q, n, 2, txn, msgid);
}

/// qtype：0=REQ 2=RESP 4=IND。
fn qmi_finish_typed(q: &mut [u8], n: usize, qtype: u8, txn: u32, msgid: u32) {
    let msglen = (n - 7) as u16;
    q[0] = qtype;
    q[1] = txn as u8;
    q[2] = (txn >> 8) as u8;
    q[3] = msgid as u8;
    q[4] = (msgid >> 8) as u8;
    q[5] = msglen as u8;
    q[6] = (msglen >> 8) as u8;
}

fn put_result(q: &mut [u8], n: &mut usize, fail: bool) {
    put_u8(q, n, 2);
    put_u16(q, n, 4);
    put_u16(q, n, if fail { 1 } else { 0 });
    put_u16(q, n, if fail { 1 } else { 0 });
}

fn rf_queued(skey: u32) -> bool {
    unsafe {
        let mut i = 0u32;
        while i < RF_QN {
            let idx = ((RF_QH + i) % RF_Q_CAP as u32) as usize;
            let s = &RF_Q[idx];
            if ((s.port << 16) | (s.txn & 0xffff)) == skey {
                return true;
            }
            i += 1;
        }
    }
    false
}

fn rf_enqueue(
    fifo: u64,
    off: u64,
    chunk: u64,
    txn: u32,
    node: u32,
    port: u32,
    msgid: u32,
) -> bool {
    unsafe {
        if RF_QN >= RF_Q_CAP as u32 {
            return false;
        }
        let idx = ((RF_QH + RF_QN) % RF_Q_CAP as u32) as usize;
        let mut name = [0u8; 64];
        let mut nn = 0usize;
        if msgid == 1 && chunk >= 32 {
            let w1 = rd32((fifo + off + 52) as usize);
            let msglen = ((w1 >> 8) & 0xffff) as u64;
            if msglen >= 3 && 7 + msglen <= chunk - 32 && msglen <= 96 {
                let base = fifo + off + 55;
                let mut p = 0u64;
                while p + 3 <= msglen {
                    let t = rd8(base + p);
                    let ln = rd8(base + p + 1) as u64 | ((rd8(base + p + 2) as u64) << 8);
                    p += 3;
                    if p + ln > msglen {
                        break;
                    }
                    if t == 1 {
                        let mut k = 0u64;
                        while k < ln && nn < 64 {
                            let b = rd8(base + p + k);
                            if b == 0 {
                                break;
                            }
                            name[nn] = b;
                            nn += 1;
                            k += 1;
                        }
                        break;
                    }
                    p += ln;
                }
            }
        }
        RF_Q[idx].txn = txn;
        RF_Q[idx].node = node;
        RF_Q[idx].port = port;
        RF_Q[idx].msg = msgid;
        RF_Q[idx].off = off;
        RF_Q[idx].fifo = fifo;
        RF_Q[idx].path = name;
        RF_Q[idx].path_n = nn as u8;
        RF_HIT = RF_HIT.wrapping_add(1);
        let hslot = (RF_HIST_N % 8) as usize;
        RF_HIST[hslot] = (msgid & 0xff) | ((txn & 0xffff) << 8) | ((port & 0xff) << 24);
        RF_HIST_N = RF_HIST_N.wrapping_add(1);
        RF_QN += 1;
        RF_PEND = true;
    }
    true
}

fn rf_bind_head() {
    unsafe {
        if RF_QN == 0 {
            RF_PEND = false;
            return;
        }
        let idx = (RF_QH % RF_Q_CAP as u32) as usize;
        let slot = &RF_Q[idx];
        RF_TXN = slot.txn;
        RF_NODE = slot.node;
        RF_PORT = slot.port;
        RF_MSG = slot.msg;
        RF_OFF = slot.off;
        RF_FIFO = slot.fifo;
        RF_PATH = slot.path;
        RF_PATH_N = slot.path_n;
        RF_PEND = true;
    }
}

fn rf_pop() {
    unsafe {
        if RF_QN == 0 {
            RF_PEND = false;
            return;
        }
        RF_QH = (RF_QH + 1) % RF_Q_CAP as u32;
        RF_QN -= 1;
        RF_PEND = RF_QN != 0;
    }
}

/// rmtfs。OPEN 对得上 storage.c 的路径就回 caller_id。
/// ALLOC 的 buff_size 0 按 rmt_storage_alloc_buff_cb 回窗口物理地址，并改走偏移。
/// 只答队头，编码与单槽相同。发送环不空就留着。
fn rf_reply(at478: u64, at479: u64, sz479: u64) {
    if unsafe { RF_QN } == 0 || at478 == 0 || at479 == 0 {
        return;
    }
    rf_bind_head();
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        return;
    }
    let msg = unsafe { RF_MSG };
    let txn = unsafe { RF_TXN };
    let fifo = unsafe { RF_FIFO };
    let off = unsafe { RF_OFF };
    let chunk = rd32((fifo + off + 8) as usize) as u64;
    let mut q = [0u8; 32];
    let mut n = 7usize;
    let mut skip = false;
    if msg == 1 {
        let name_n = unsafe { RF_PATH_N } as usize;
        let mut name = [0u8; 64];
        unsafe {
            let mut i = 0;
            while i < name_n && i < 64 {
                name[i] = RF_PATH[i];
                i += 1;
            }
        }
        put_result(&mut q, &mut n, false);
        if let Some(id) = rfs_caller(&name[..name_n]) {
            unsafe { RF_ID = id };
            put_u8(&mut q, &mut n, 16);
            put_u16(&mut q, &mut n, 4);
            put_u32(&mut q, &mut n, id);
        } else {
            unsafe { RF_ID = 0xffff_ffff };
            q[7] = 2;
            // result 已经写成 0。改成失败。
            put_u16_at(&mut q, 10, 1);
            put_u16_at(&mut q, 12, 1);
        }
    } else if msg == 2 {
        let id = qmi_tlv_u32(fifo, off, chunk, 1).unwrap_or(0xffff_ffff);
        put_result(&mut q, &mut n, id >= 10);
    } else if msg == 3 {
        let id = qmi_tlv_u32(fifo, off, chunk, 1).unwrap_or(0xffff_ffff);
        let ok = rfs_iovec(fifo, off, chunk, id);
        unsafe {
            if ok {
                RF_IOK = RF_IOK.wrapping_add(1);
            } else {
                RF_IFAIL = RF_IFAIL.wrapping_add(1);
            }
        }
        put_result(&mut q, &mut n, !ok);
    } else if msg == 4 {
        let sz = qmi_tlv_u32(fifo, off, chunk, 2);
        if let Some(sz) = sz {
            unsafe { RF_SZ = sz };
        }
        if sz.is_none() || sz.unwrap() as u64 > RFS_SIZE || !rfs_assign() {
            put_result(&mut q, &mut n, true);
        } else {
            unsafe { RFS_IOFF = if sz.unwrap() == 0 { 1 } else { 0 } };
            put_result(&mut q, &mut n, false);
            put_u8(&mut q, &mut n, 16);
            put_u16(&mut q, &mut n, 8);
            put_u32(&mut q, &mut n, RFS_ADDR as u32);
            put_u32(&mut q, &mut n, (RFS_ADDR >> 32) as u32);
        }
    } else if msg == 5 {
        put_result(&mut q, &mut n, false);
        put_u8(&mut q, &mut n, 16);
        put_u16(&mut q, &mut n, 1);
        put_u8(&mut q, &mut n, 0);
    } else {
        skip = true;
    }
    if skip {
        unsafe {
            if RF_SEEN_N < 32 {
                RF_SEEN[RF_SEEN_N as usize] =
                    (RF_PORT << 16) | (txn & 0xffff);
                RF_SEEN_N += 1;
            }
            rf_pop();
        }
        return;
    }
    qmi_finish(&mut q, n, txn, msg);
    if msg == 1 {
        unsafe {
            let mut i = 0usize;
            while i < 12 && i < n {
                RF_RSP[i] = q[i];
                i += 1;
            }
            RF_RSP_N = i as u8;
        }
    }
    if glink_tx_payload(
        at478,
        at479,
        tx_head,
        sz479,
        0x10,
        unsafe { RF_NODE },
        unsafe { RF_PORT },
        &q[..n],
    ) {
        unsafe {
            if RF_SEEN_N < 32 {
                RF_SEEN[RF_SEEN_N as usize] =
                    (RF_PORT << 16) | (txn & 0xffff);
                RF_SEEN_N += 1;
            }
            rf_pop();
            RF_N += 1;
        }
    }
}

fn put_u16_at(q: &mut [u8], at: usize, v: u16) {
    if at + 1 < q.len() {
        q[at] = v as u8;
        q[at + 1] = (v >> 8) as u8;
    }
}

/// modem_a FAT16：4K LBA。已知 data 区簇 2 在 MODEM_LBA+17、每簇 4 块。
/// 读 BPB，算出 root / data；对不上已知 data 起点就拒（调用方有硬编码回退）。
fn modem_fat_geom(ufs: &mut Ufs) -> Result<(u32, u32, u32), u32> {
    ufs.read10(MODEM_LBA, 1, DATA).map_err(|_| 0x51u32)?;
    let b = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
    let bps = u16::from_le_bytes([b[11], b[12]]) as u32;
    let spc = b[13] as u32;
    let reserved = u16::from_le_bytes([b[14], b[15]]) as u32;
    let nfats = b[16] as u32;
    let root_ents = u16::from_le_bytes([b[17], b[18]]) as u32;
    let fat_sz = u16::from_le_bytes([b[22], b[23]]) as u32;
    if (bps != 512 && bps != 4096) || spc == 0 || nfats == 0 || fat_sz == 0 {
        return Err(0x52);
    }
    let root_bytes = root_ents.saturating_mul(32);
    let root_secs = (root_bytes + bps - 1) / bps;
    let data_sec = reserved + nfats * fat_sz + root_secs;
    let sec_per_4k = 4096 / bps;
    if sec_per_4k == 0 || data_sec % sec_per_4k != 0 {
        return Err(0x53);
    }
    let data_off = data_sec / sec_per_4k;
    let root_off = (reserved + nfats * fat_sz) / sec_per_4k;
    let spc_4k = (spc * bps) / 4096;
    if spc_4k == 0 || data_off != 17 || spc_4k != 4 {
        return Err(0x54 | (data_off << 8) | (spc_4k << 24));
    }
    let root_secs_4k = root_secs / sec_per_4k;
    if root_secs_4k == 0 {
        return Err(0x55);
    }
    Ok((MODEM_LBA + root_off, root_secs_4k, MODEM_LBA + data_off))
}

fn fat83_eq(ent: &[u8], name: &[u8], ext: &[u8]) -> bool {
    if ent.len() < 11 {
        return false;
    }
    let mut i = 0usize;
    while i < 8 {
        let a = ent[i];
        let b = if i < name.len() {
            let c = name[i];
            if c >= b'a' && c <= b'z' {
                c - 32
            } else {
                c
            }
        } else {
            b' '
        };
        let aa = if a >= b'a' && a <= b'z' { a - 32 } else { a };
        if aa != b {
            return false;
        }
        i += 1;
    }
    i = 0;
    while i < 3 {
        let a = ent[8 + i];
        let b = if i < ext.len() {
            let c = ext[i];
            if c >= b'a' && c <= b'z' {
                c - 32
            } else {
                c
            }
        } else {
            b' '
        };
        let aa = if a >= b'a' && a <= b'z' { a - 32 } else { a };
        if aa != b {
            return false;
        }
        i += 1;
    }
    true
}

fn fat_split83(comp: &[u8]) -> ([u8; 8], [u8; 3]) {
    // 只按「最后一个」点拆 8.3，避免 549_38_0.mbn.1.32 把 ext 弄成 "1.3"。
    let mut name = [b' '; 8];
    let mut ext = [b' '; 3];
    let mut dot = None;
    let mut i = 0usize;
    while i < comp.len() {
        if comp[i] == b'.' {
            dot = Some(i);
        }
        i += 1;
    }
    let (npart, epart) = match dot {
        Some(d) => (&comp[..d], &comp[d + 1..]),
        None => (comp, &comp[0..0]),
    };
    // 若 stem 仍含点（少见），8.3 截断到 8；LFN 路径另比全名。
    let mut i = 0usize;
    while i < npart.len() && i < 8 {
        let c = npart[i];
        name[i] = if c >= b'a' && c <= b'z' { c - 32 } else { c };
        i += 1;
    }
    i = 0;
    while i < epart.len() && i < 3 {
        let c = epart[i];
        // ext 遇点即停（.1.32 残留）
        if c == b'.' {
            break;
        }
        ext[i] = if c >= b'a' && c <= b'z' { c - 32 } else { c };
        i += 1;
    }
    (name, ext)
}

/// 去掉 TFTP 会话后缀 `.digits.digits`（如 .1.32）。
fn fat_strip_tftp_suffix(name: &[u8]) -> &[u8] {
    let mut end = name.len();
    // 两段：.NUM.NUM
    let mut dots = 0usize;
    let mut i = name.len();
    while i > 0 {
        i -= 1;
        let c = name[i];
        if c == b'.' {
            dots += 1;
            if dots == 2 {
                // 确认两段都是纯数字
                let mut ok = true;
                let mut j = i + 1;
                let mut seen_digit = false;
                let mut stage = 0u8;
                while j < name.len() {
                    let d = name[j];
                    if d == b'.' {
                        if stage != 0 || !seen_digit {
                            ok = false;
                            break;
                        }
                        stage = 1;
                        seen_digit = false;
                    } else if d >= b'0' && d <= b'9' {
                        seen_digit = true;
                    } else {
                        ok = false;
                        break;
                    }
                    j += 1;
                }
                if ok && seen_digit && stage == 1 {
                    end = i;
                }
                break;
            }
        } else if !(c >= b'0' && c <= b'9') && dots < 2 {
            // 非数字打断后缀扫描
            if dots == 0 {
                return name;
            }
        }
    }
    &name[..end]
}

fn fat_upc(b: u8) -> u8 {
    if b >= b'a' && b <= b'z' {
        b - 32
    } else {
        b
    }
}

/// 从 LFN 槽取 UCS-2 字符（只认 ASCII 低字节；高字节非 0 当 0xFF）。
fn lfn_chars(ent: &[u8], out: &mut [u8; 13]) -> usize {
    // offsets: 1..10 (5), 14..25 (6), 28..31 (2) — 13 UTF-16LE chars
    const OFF: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    let mut n = 0usize;
    for &o in &OFF {
        if o + 1 >= ent.len() {
            break;
        }
        let lo = ent[o];
        let hi = ent[o + 1];
        if lo == 0 && hi == 0 {
            break;
        }
        out[n] = if hi == 0 { lo } else { 0xFF };
        n += 1;
    }
    n
}

fn lfn_eq_comp(lfn: &[u8], lfn_n: usize, comp: &[u8]) -> bool {
    if lfn_n != comp.len() {
        return false;
    }
    let mut i = 0usize;
    while i < lfn_n {
        if fat_upc(lfn[i]) != fat_upc(comp[i]) {
            return false;
        }
        i += 1;
    }
    true
}

/// 在 root 或子目录簇链里找 8.3 或 LFN。返回 (cluster, size, is_dir)。
/// `comp` 是原始路径分量（用于 LFN 全名比对）；name/ext 是 8.3。
fn fat_dir_find(
    ufs: &mut Ufs,
    data_lba: u32,
    dir_cl: u32,
    root_lba: u32,
    root_secs: u32,
    name: &[u8],
    ext: &[u8],
    comp: &[u8],
) -> Result<(u32, u32, bool), u32> {
    let (name8, ext3) = {
        let mut n = [0u8; 8];
        let mut e = [0u8; 3];
        n.copy_from_slice(name);
        e.copy_from_slice(ext);
        (n, e)
    };
    // 累积 LFN（逆序槽）；最多 20 槽 × 13 = 260，够用。
    let mut lfn_buf = [0u8; 260];
    let mut lfn_n = 0usize;
    let mut lfn_expect: u8 = 0;
    let mut scan_block = |b: &[u8]| -> Option<Result<(u32, u32, bool), u32>> {
        let mut off = 0usize;
        while off + 32 <= b.len() {
            let ent = &b[off..off + 32];
            if ent[0] == 0 {
                return Some(Err(0x57));
            }
            if ent[0] == 0xe5 {
                lfn_n = 0;
                lfn_expect = 0;
                off += 32;
                continue;
            }
            let attr = ent[11];
            if attr & 0x0f == 0x0f {
                // LFN 槽。seq = bit0-4；bit6=last。
                let seq = ent[0];
                let ord = seq & 0x1f;
                if ord == 0 {
                    lfn_n = 0;
                    lfn_expect = 0;
                    off += 32;
                    continue;
                }
                let mut chars = [0u8; 13];
                let cn = lfn_chars(ent, &mut chars);
                if seq & 0x40 != 0 {
                    lfn_n = 0;
                    lfn_expect = ord;
                }
                if lfn_expect == 0 || ord != lfn_expect {
                    // 失序，丢弃
                    lfn_n = 0;
                    lfn_expect = 0;
                    off += 32;
                    continue;
                }
                // 槽按倒序出现：先 last(高序号) 后 1。拼到缓冲前端位置 (ord-1)*13。
                let base = (ord as usize - 1) * 13;
                if base + cn <= lfn_buf.len() {
                    let mut i = 0usize;
                    while i < cn {
                        lfn_buf[base + i] = chars[i];
                        i += 1;
                    }
                    if seq & 0x40 != 0 {
                        lfn_n = base + cn;
                    } else if base + cn > lfn_n {
                        lfn_n = base + cn;
                    }
                }
                lfn_expect = ord.wrapping_sub(1);
                off += 32;
                continue;
            }
            // 普通 8.3 / 目录项。先比 8.3，再比累积 LFN。
            let hit83 = fat83_eq(ent, &name8, &ext3);
            let hitlfn = lfn_n > 0 && lfn_expect == 0 && lfn_eq_comp(&lfn_buf[..lfn_n], lfn_n, comp);
            lfn_n = 0;
            lfn_expect = 0;
            if hit83 || hitlfn {
                let c2 = u16::from_le_bytes([ent[26], ent[27]]) as u32
                    | ((u16::from_le_bytes([ent[20], ent[21]]) as u32) << 16);
                // size 在 28..32；目录项常为 0，文件应为实长。
                let sz = u32::from_le_bytes([ent[28], ent[29], ent[30], ent[31]]);
                return Some(Ok((c2, sz, attr & 0x10 != 0)));
            }
            off += 32;
        }
        None
    };
    if dir_cl == 0 {
        let mut s = 0u32;
        while s < root_secs {
            ufs.read10(root_lba + s, 1, DATA).map_err(|_| 0x56u32)?;
            let b = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
            if let Some(r) = scan_block(b) {
                return r;
            }
            s += 1;
        }
        return Err(0x57);
    }
    let mut cl = dir_cl;
    let mut guard = 0u32;
    while cl >= 2 && cl < 0xfff8 && guard < 4096 {
        guard += 1;
        let lba = data_lba + (cl - 2) * 4;
        let mut s = 0u32;
        while s < 4 {
            ufs.read10(lba + s, 1, DATA).map_err(|_| 0x56u32)?;
            let b = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
            if let Some(r) = scan_block(b) {
                return r;
            }
            s += 1;
        }
        cl = fat_next(cl) as u32;
    }
    Err(0x57)
}

/// 相对 modem 根的路径，如 image/modem_pr/so/549_38_0.mbn。
/// 失败码：0x57 未找到；高字节 = 失败分量下标+1（tz 可辨）。
fn fat_path_open(ufs: &mut Ufs, rel: &[u8]) -> Result<(u32, u32), u32> {
    // 优先 BPB；对不上已知 data=+17/spc=4 时回退：reserved1 + 2×FAT6 + root4 = 17。
    let (root_lba, root_secs, data_lba) = match modem_fat_geom(ufs) {
        Ok(g) => g,
        Err(_) => (MODEM_LBA + 13, 4, MODEM_LBA + 17),
    };
    let mut dir_cl = 0u32;
    let mut start = 0usize;
    let mut i = 0usize;
    let mut depth = 0u32;
    while i <= rel.len() {
        if i == rel.len() || rel[i] == b'/' {
            if i > start {
                let raw = &rel[start..i];
                let last = i == rel.len();
                let comp = if last {
                    fat_strip_tftp_suffix(raw)
                } else {
                    raw
                };
                let (n8, e3) = fat_split83(comp);
                match fat_dir_find(ufs, data_lba, dir_cl, root_lba, root_secs, &n8, &e3, comp) {
                    Ok((cl, sz, is_dir)) => {
                        if last {
                            if is_dir {
                                return Err(0x58 | ((depth + 1) << 8));
                            }
                            return Ok((cl, sz));
                        }
                        if !is_dir {
                            return Err(0x58 | ((depth + 1) << 8));
                        }
                        dir_cl = cl;
                    }
                    Err(e) => {
                        return Err((e & 0xff) | ((depth + 1) << 8));
                    }
                }
                depth += 1;
            }
            start = i + 1;
        }
        i += 1;
    }
    Err(0x57)
}

/// 簇 2 起，每簇 4 个 4K 块。只读文件第一块的 ELF 头，不发给 modem。
fn fat_file_hdr(ufs: &mut Ufs, cl: u32) -> (u32, u32) {
    if cl < 2 {
        return (0, 0);
    }
    let lba = MODEM_LBA + 17 + (cl - 2) * 4;
    if ufs.read10(lba, 1, DATA).is_err() {
        return (0, 0);
    }
    let b = unsafe { core::slice::from_raw_parts(DATA as *const u8, 20) };
    let mag = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let et = u16::from_le_bytes([b[16], b[17]]) as u32;
    let em = u16::from_le_bytes([b[18], b[19]]) as u32;
    (mag, et | (em << 16))
}

/// 开机一次：查 modem_a 上 SO，写 tz= 和 th=。
/// 不进 smem_scan；不改 glink_tx 上限；RRQ 仍 ERROR（不 DATA 真喂）。
/// 候选路径：image/… 与根上 modem_pr/…（部分机把 modem_pr 放根）。
fn tf_boot_probe(ufs: &mut Ufs) {
    if unsafe { TZ_ST } != 0 {
        return;
    }
    const CANDS: &[&[u8]] = &[
        b"image/modem_pr/so/549_38_0.mbn",
        b"modem_pr/so/549_38_0.mbn",
        b"image/modem_pr/so/549_38_0.mbn.1.32",
    ];
    let mut last_err = 0x57u32;
    for &p in CANDS {
        match fat_path_open(ufs, p) {
            Ok((cl, sz)) => {
                let (mag, eh) = fat_file_hdr(ufs, cl);
                unsafe {
                    TZ_CL = cl;
                    TZ_SZ = sz;
                    TZ_MAG = mag;
                    TZ_EH = eh;
                    // st=1 命中；若 sz==0 仍标 1（上层看 sz 判空文件/错解析）
                    TZ_ST = 1;
                }
                return;
            }
            Err(e) => {
                last_err = e;
            }
        }
    }
    unsafe {
        TZ_SZ = 0;
        TZ_CL = last_err;
        let low = last_err & 0xff;
        TZ_ST = if low == 0x57 {
            4
        } else if low < 0x100 {
            low
        } else {
            5
        };
    }
}

fn tf_drop_pend() {
    unsafe {
        // 刀 G3：环形写。旧代码满 8 冻结，首个 8 偏移永久误杀环复用位上的新包。
        TF_SEEN[TF_SEEN_I as usize] = TF_OFF;
        TF_SEEN_I = (TF_SEEN_I + 1) & 7;
        if TF_SEEN_N < 8 {
            TF_SEEN_N += 1;
        }
        TF_PEND = false;
        TF_N += 1;
    }
}

fn tf_name_has(pat: &[u8]) -> bool {
    let n = unsafe { TF_NAME_N } as usize;
    if n < pat.len() {
        return false;
    }
    let start = n - pat.len();
    let mut i = 0usize;
    while i < pat.len() {
        if unsafe { TF_NAME[start + i] } != pat[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn tf_is_so() -> bool {
    tf_name_has(b"549_38_0.mbn")
}

fn tf_is_ruleset() -> bool {
    tf_name_has(b"ota_firewall/ruleset")
}

fn tf_new_port() -> u32 {
    unsafe {
        let p = TF_TID;
        TF_TID = TF_TID.wrapping_add(1);
        if TF_TID < 0x1000 {
            TF_TID = 0x1000;
        }
        p
    }
}

fn tf_put_dec(q: &mut [u8], n: &mut usize, mut v: u32) {
    let mut tmp = [0u8; 10];
    let mut k = 0usize;
    if v == 0 {
        if *n < q.len() {
            q[*n] = b'0';
            *n += 1;
        }
        return;
    }
    while v > 0 && k < tmp.len() {
        tmp[k] = b'0' + (v % 10) as u8;
        k += 1;
        v /= 10;
    }
    while k > 0 {
        k -= 1;
        if *n < q.len() {
            q[*n] = tmp[k];
            *n += 1;
        }
    }
}

fn tf_put_cstr(q: &mut [u8], n: &mut usize, s: &[u8]) {
    let mut i = 0usize;
    while i < s.len() && *n < q.len() {
        q[*n] = s[i];
        *n += 1;
        i += 1;
    }
    if *n < q.len() {
        q[*n] = 0;
        *n += 1;
    }
}

/// RRQ 载荷里有没有选项，有没有 rsize/seek 键，以及两键的值。
/// 键不存在时值为 0，和“键存在且值为 0”分开记。
/// 刀 G5：补 seek 解析——stock tqftpserv 的 OACK 会把 rsize/seek 原样回显，
/// 客户端段落读（rsize=52 读 ELF 头）靠 seek 定偏移，此前我们全忽略。
fn tf_rrq_rsize(fifo: u64, off: u64, chunk: u64) -> (bool, bool, u32, bool, u32) {
    let size = rd32((fifo + off + 36) as usize) as u64;
    if size < 4 || 48 + size > 16 + chunk {
        return (false, false, 0, false, 0);
    }
    let base = fifo + off + 48;
    let mut p = 2u64;
    let mut skipped = 0u32;
    while skipped < 2 && p < size {
        let b = rd8(base + p);
        p += 1;
        if b == 0 {
            skipped += 1;
        }
    }
    let has = p < size;
    let mut saw_rs = false;
    let mut saw_sk = false;
    let mut rsize = 0u32;
    let mut seek = 0u32;
    while p < size {
        let ks = p;
        while p < size && rd8(base + p) != 0 {
            p += 1;
        }
        let ke = p;
        if p < size {
            p += 1;
        }
        let vs = p;
        while p < size && rd8(base + p) != 0 {
            p += 1;
        }
        let ve = p;
        if p < size {
            p += 1;
        }
        let klen = ke - ks;
        let k0 = rd8(base + ks);
        let is_rs = klen == 5
            && k0 == b'r'
            && rd8(base + ks + 1) == b's'
            && rd8(base + ks + 2) == b'i'
            && rd8(base + ks + 3) == b'z'
            && rd8(base + ks + 4) == b'e';
        let is_sk = klen == 4
            && k0 == b's'
            && rd8(base + ks + 1) == b'e'
            && rd8(base + ks + 2) == b'e'
            && rd8(base + ks + 3) == b'k';
        if is_rs || is_sk {
            let mut v = 0u32;
            let mut i = vs;
            while i < ve {
                let c = rd8(base + i);
                if c < b'0' || c > b'9' {
                    break;
                }
                v = v.saturating_mul(10).saturating_add((c - b'0') as u32);
                i += 1;
            }
            if is_rs {
                saw_rs = true;
                rsize = v;
            } else {
                saw_sk = true;
                seek = v;
            }
        }
    }
    (has, saw_rs, rsize, saw_sk, seek)
}

static mut TF_OPTBUF: [u8; 192] = [0; 192];
static mut TF_OPTLEN: u8 = 0;

/// 刀 G11：rpl= 渲染对——qrtr 尺寸段 vs 抄本窗（tf_save_so 记账）。
static mut TF_RPL_SZ: u32 = 0;
static mut TF_RPL_RM: u32 = 0;

/// 刀 G5：RRQ/WRQ 滚动日志（6 槽）。任何目的口发到 node1 的 op1/op2 都入册——
/// 此前只看 0x11，客户端若换口发请求整段盲。fl bit0=rsize 键在、bit1=seek 键在。
static mut TF_RL_P: [u32; 6] = [0; 6];
static mut TF_RL_S: [u32; 6] = [0; 6];
static mut TF_RL_O: [u32; 6] = [0; 6];
static mut TF_RL_R: [u32; 6] = [0; 6];
static mut TF_RL_K: [u32; 6] = [0; 6];
static mut TF_RL_F: [u32; 6] = [0; 6];
static mut TF_RL_N: u32 = 0;
static mut TF_RL_VIS: u64 = 0;

/// 只把 SO 的 RRQ 载荷抄进 BSS。选项等到第一次 sm 之后再解析。
fn tf_save_so(fifo: u64, off: u64, chunk: u64) {
    if !tf_is_so() {
        return;
    }
    let size = rd32((fifo + off + 36) as usize) as usize;
    // 刀 G11：物理载荷＝chunk-32（fragment＝16B glink 头＋chunk，qrtr 头
    // 32B 落 +16..+48）。旧 chunk-48 少抄 16B——G10 转录里 "tsize" 键
    // 死在这道裁切上（TF_OPTLEN=88=旧 room，size≥88）。
    let room = if chunk > 32 { chunk as usize - 32 } else { 0 };
    let n = size.min(room).min(192);
    unsafe {
        TF_RPL_SZ = size as u32;
        TF_RPL_RM = room as u32;
        let mut i = 0usize;
        while i < n {
            TF_OPTBUF[i] = rd8(fifo + off + 48 + i as u64);
            i += 1;
        }
        TF_OPTLEN = n as u8;
    }
}

fn tf_apply_saved() {
    unsafe {
        TF_REQ_BLK = 512;
        TF_REQ_WSZ = 0;
        TF_REQ_TMO = 1000;
        TF_HAS_TS = 0;
        // 刀 G5：rsize/seek 也是逐请求选项，缺省清零重置。
        TF_HAS_RS = 0;
        TF_RSIZE = 0;
        TF_HAS_SK = 0;
        TF_SEEK = 0;
    }
    let n = unsafe { TF_OPTLEN } as usize;
    if n < 4 {
        return;
    }
    let mut p = 2usize;
    let mut skipped = 0u32;
    while skipped < 2 && p < n {
        let b = unsafe { TF_OPTBUF[p] };
        p += 1;
        if b == 0 {
            skipped += 1;
        }
    }
    while p < n {
        let ks = p;
        while p < n && unsafe { TF_OPTBUF[p] } != 0 {
            p += 1;
        }
        let ke = p;
        if p < n {
            p += 1;
        }
        let vs = p;
        while p < n && unsafe { TF_OPTBUF[p] } != 0 {
            p += 1;
        }
        let ve = p;
        if p < n {
            p += 1;
        }
        let mut v = 0u32;
        let mut i = vs;
        let mut digits = ve > vs;
        while i < ve {
            let c = unsafe { TF_OPTBUF[i] };
            if c < b'0' || c > b'9' {
                digits = false;
                break;
            }
            v = v.saturating_mul(10).saturating_add((c - b'0') as u32);
            i += 1;
        }
        if !digits {
            continue;
        }
        let klen = ke - ks;
        let k0 = unsafe { TF_OPTBUF[ks] };
        unsafe {
            if klen == 7 && k0 == b'b' {
                // 原样收。降议刀已死（客户端对值≠请求的 OACK 整体停车，连 DATA 都不 ACK），
                // 唯一活路=按请求的 7680 发，glink TX 侧扩容承接。
                TF_REQ_BLK = v;
            } else if klen == 5 && k0 == b'w' {
                TF_REQ_WSZ = v;
            } else if klen == 9 && k0 == b't' {
                TF_REQ_TMO = v;
            } else if klen == 5 && k0 == b't' {
                TF_HAS_TS = 1;
            } else if klen == 5 && k0 == b'r' {
                // 刀 G5：stock tqftpserv 对齐——rsize/seek 从保存的 RRQ 重放，
                // OACK 原样回显，DATA 偏移从 seek 起算。
                TF_HAS_RS = 1;
                TF_RSIZE = v;
            } else if klen == 4 && k0 == b's' {
                TF_HAS_SK = 1;
                TF_SEEK = v;
            }
        }
    }
}

fn tf_note_ack(off: u64, fifo: u64) {
    if unsafe { TF_ACK_OFF } == off as u32 {
        return;
    }
    let op = ((rd8(fifo + off + 48) as u32) << 8) | (rd8(fifo + off + 49) as u32);
    if op == 4 {
        let blk = ((rd8(fifo + off + 50) as u32) << 8) | (rd8(fifo + off + 51) as u32);
        unsafe {
            TF_ACK_OFF = off as u32;
            TF_ACK_BLK = blk as u16;
            TF_ACK_PEND = true;
        }
    } else if op == 5 {
        let code = ((rd8(fifo + off + 50) as u32) << 8) | (rd8(fifo + off + 51) as u32);
        unsafe {
            TF_ACK_OFF = off as u32;
            // 9 = End of Transfer。tqftpserv 日志里 stat（rsize=0）的 OACK 之后就是它。
            TF_ST = if code == 9 && TF_ST == 1 { 4 } else { 9 };
            TF_ACK_PEND = false;
        }
    }
}

/// 从 SO 的簇链读 `n` 字节到 `TF_BLKBUF[4..]`。簇 16KiB，FAT 已在 `MODEM_FAT`。
fn so_read(ufs: &mut Ufs, off: u32, n: usize) -> usize {
    let sz = unsafe { TZ_SZ };
    if n == 0 || off >= sz {
        return 0;
    }
    let n = n.min((sz - off) as usize).min(8192);
    let mut cl = unsafe { TZ_CL };
    let mut skip = off / 16384;
    while skip > 0 {
        let nxt = fat_next(cl) as u32;
        if nxt < 2 || nxt >= 0xfff8 {
            return 0;
        }
        cl = nxt;
        skip -= 1;
    }
    let within = (off % 16384) as usize;
    let sec = (within / 4096) as u32;
    let bo = within % 4096;
    let lba = MODEM_LBA + 17 + (cl - 2) * 4 + sec;
    if ufs.read10(lba, 1, DATA).is_err() {
        return 0;
    }
    let avail = 4096 - bo;
    let take = n.min(avail);
    unsafe {
        core::ptr::copy_nonoverlapping(
            (DATA as *const u8).add(bo),
            TF_BLKBUF.as_mut_ptr().add(4),
            take,
        );
    }
    if take == n {
        return n;
    }
    let rest = n - take;
    let lba2 = if sec + 1 < 4 {
        lba + 1
    } else {
        let nxt = fat_next(cl) as u32;
        if nxt < 2 || nxt >= 0xfff8 {
            return take;
        }
        MODEM_LBA + 17 + (nxt - 2) * 4
    };
    if ufs.read10(lba2, 1, DATA).is_err() {
        return take;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(DATA as *const u8, TF_BLKBUF.as_mut_ptr().add(4 + take), rest);
    }
    take + rest
}

/// tftp。WRQ 仍在监听口回 ACK block 0。
/// SO 的 RRQ 在 II_ST=2（IPA INIT 完成）后立即发 OACK（刀 G4：不再等第一次 sm）。
/// rsize 键存在且值为 0 时不发 DATA，等 code 9。
fn tf_reply(ufs: &mut Ufs, at478: u64, at479: u64, sz479: u64) {
    if at478 == 0 || at479 == 0 {
        return;
    }
    // 刀 G12：入口计数（pq=call 首格）。dc 在环闸里已是 dv 第四格，
    // call = dc + noarm + nost + nopend + dv0 全分割。
    unsafe { TF_PQ_CALL = TF_PQ_CALL.wrapping_add(1) };
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    unsafe {
        TF_TXH = tx_head as u32;
        TF_TXT = tx_tail as u32;
        TF_TXSZ = sz479 as u32;
    }
    // 刀 G4：环非空不再一刀切回。G3 实测：一笔不被消费的包（CTRL 副本）落在环里，
    // head≠tail 门把后续 grace/DATA 全部冻死（tt=a00.960 永冻、tb 停在 1）。改
    // 「免回卷容量闸」：head≠tail 时只要最大包（整包 8192）不回卷也放得下就继续；
    // 回卷纪律（非空不许绕回 0）与 glink_tx_payload 内闸保持一致。
    if tx_head != tx_tail && tx_head + 8192 > sz479 {
        unsafe { TF_DC = TF_DC.wrapping_add(1) };
        return;
    }
    // 刀 G4：武装改 II_ST=2——G3 实测 rd=19f3(6.6s) 对 tm=3e8(1s)：SM_ARMED 等
    // 第一次主机 sm 纯属观察纪律，客户端早超时闭口。本靴 +0.7s 首样已 ii=2、
    // rf=3：rmtfs 回复早已在 glink TX 上流动，无主机也活。
    let armed = unsafe { II_ST } == 2;
    let st = unsafe { TF_ST };
    // 宽限刀（假设 E）：OACK 已发、ACK-0 未到、块 0 未动，满 2s 合成 ack=0，
    // 落进下面的正常分支直发 DATA 块 1。若期间真 ACK-0 已到则此门不进。
    if armed
        && st == 1
        && !unsafe { TF_ACK_PEND }
        && unsafe { TF_BLKNO } == 0
        && unsafe { TF_OACK_AT } != 0
    {
        let freq = time::freq_or(FREQ_FALLBACK as u32) as u64;
        if time::cntvct().wrapping_sub(unsafe { TF_OACK_AT }) > freq * 2 {
            unsafe {
                TF_ACK_PEND = true;
                TF_ACK_BLK = 0;
            }
        }
    }
    // 刀 G12：pump 门四分位插桩（宽限 synth 之后计数——synth 能救的归
    // dv0，不误记 nopend）。
    if !armed {
        unsafe { TF_PQ_NOARM = TF_PQ_NOARM.wrapping_add(1) };
    } else if st != 1 && st != 2 {
        unsafe { TF_PQ_NOST = TF_PQ_NOST.wrapping_add(1) };
    } else if !unsafe { TF_ACK_PEND } {
        unsafe { TF_PQ_NOPEND = TF_PQ_NOPEND.wrapping_add(1) };
    }
    if armed && (st == 1 || st == 2) && unsafe { TF_ACK_PEND } {
        unsafe { TF_DV = TF_DV.wrapping_add(1) };
        let ack = unsafe { TF_ACK_BLK };
        let expect = unsafe { TF_BLKNO };
        if ack != expect {
            unsafe { TF_ACK_PEND = false };
            return;
        }
        // stat：rsize 键在，值为 0。停在状态 1，等 opcode 5 code 9。不发 DATA。
        if unsafe { TF_HAS_RS } != 0 && unsafe { TF_RSIZE } == 0 {
            unsafe { TF_ACK_PEND = false };
            return;
        }
        // 对方要的块超过本方缓冲/环上限（8192）才拒。7680 是本机实测请求值，
        // TX 扩容刀后 glink 按 isz(8320 intent) 与环界双闸放行。
        if unsafe { TF_REQ_BLK } > 8192 {
            unsafe {
                TF_ST = 7;
                TF_ACK_PEND = false;
            }
            return;
        }
        let blk_sz = unsafe { TF_REQ_BLK } as usize;
        // 刀 G5：DATA 从 seek 起算（stock tftp_send_data 同款：文件偏移 =
        // seek + ack*blksize，块号在窗口内从 1 数）。此前 seek 全忽略，段落读
        // 永远从文件头回数据。
        let off = unsafe { TF_SEEK }
            .saturating_add((ack as u32).saturating_mul(blk_sz as u32));
        let limit = {
            let sz = unsafe { TZ_SZ };
            let rs = unsafe { TF_RSIZE };
            // rsize 是「从 seek 起要多少」，窗口尾 = seek+rsize，夹在文件尾内。
            if rs > 0 {
                unsafe { TF_SEEK }.saturating_add(rs).min(sz)
            } else {
                sz
            }
        };
        if off >= limit {
            unsafe {
                TF_ST = 3;
                TF_ACK_PEND = false;
            }
            return;
        }
        let mut want = blk_sz;
        if off + blk_sz as u32 > limit {
            want = (limit - off) as usize;
        }
        let n = so_read(ufs, off, want);
        if n == 0 {
            unsafe {
                TF_DZ = TF_DZ.wrapping_add(1);
                TF_ST = 8;
                TF_ACK_PEND = false;
            }
            return;
        }
        let blk = ack.wrapping_add(1);
        unsafe {
            TF_BLKBUF[0] = 0;
            TF_BLKBUF[1] = 3;
            TF_BLKBUF[2] = (blk >> 8) as u8;
            TF_BLKBUF[3] = blk as u8;
        }
        let pkt = unsafe { &TF_BLKBUF[..4 + n] };
        if glink_tx_payload(
            at478,
            at479,
            tx_head,
            sz479,
            unsafe { TF_XFER_PORT },
            unsafe { TF_NODE },
            unsafe { TF_PORT },
            pkt,
        ) {
            unsafe {
                TF_BLKNO = blk;
                TF_ST = if n < blk_sz || off + n as u32 >= limit {
                    3
                } else {
                    2
                };
                TF_ACK_PEND = false;
                TF_N += 1;
            }
        } else {
            unsafe { TF_DX = TF_DX.wrapping_add(1) };
        }
        return;
    }
    if unsafe { !TF_PEND } {
        return;
    }
    let op = unsafe { TF_OP };
    if op == 2 {
        let mut q = [0u8; 4];
        q[0] = 0;
        q[1] = 4;
        if glink_tx_payload(
            at478,
            at479,
            tx_head,
            sz479,
            0x11,
            unsafe { TF_NODE },
            unsafe { TF_PORT },
            &q,
        ) {
            tf_drop_pend();
        }
        return;
    }
    // tqftpserv：请求一到就换一个新口回 ERROR 1。不等 sm。
    // INIT 完成前不发：早发 ERROR 会把发送环卡住。
    if op == 1 && tf_is_ruleset() {
        if unsafe { II_ST } != 2 {
            return;
        }
        let sport = tf_new_port();
        let mut q = [0u8; 20];
        let msg = b"file not found";
        q[0] = 0;
        q[1] = 5;
        q[2] = 0;
        q[3] = 1;
        let mut n = 4usize;
        let mut i = 0usize;
        while i < msg.len() && n < q.len() {
            q[n] = msg[i];
            n += 1;
            i += 1;
        }
        if n < q.len() {
            q[n] = 0;
            n += 1;
        }
        if glink_tx_payload(
            at478,
            at479,
            tx_head,
            sz479,
            sport,
            unsafe { TF_NODE },
            unsafe { TF_PORT },
            &q[..n],
        ) {
            unsafe {
                TF_REJ = TF_REJ.wrapping_add(1);
                TF_REJ_PORT = sport;
            }
            tf_drop_pend();
        }
        return;
    }
    // 状态 1 且还没送出 DATA：旧端口的 OACK 没人 ACK。新 RRQ 换端口再发一次。
    // 状态 4：stat 的 End of Transfer 已经收到，下一笔 SO 重新 OACK。
    let st_now = unsafe { TF_ST };
    let retarget = st_now == 1 && unsafe { TF_BLKNO } == 0;
    let fresh = st_now == 0 || st_now == 4;
    // 刀 G9c：st=2/3（DATA 已发/送完）新到 pend 不再饿死——只回 OACK
    // 应答（不重置游标、不 synth DATA、~100B/笔）。G9 满服务版三靴全黑
    // （G8b 对照定罪，收据 7d8d282）；节流版答到每个重试口又不打环。
    let retry = st_now == 2 || st_now == 3;
    // 游标不回头纪律保持：fresh（st 0/4）之外照旧丢。armed 已是 II_ST=2（刀 G4）。
    if op == 1 && !armed && tf_is_so() && unsafe { TZ_ST } == 1 && fresh {
        return;
    }
    if op != 1
        || !armed
        || (!retarget && !fresh && !retry)
        || !tf_is_so()
        || unsafe { TZ_ST } != 1
    {
        tf_drop_pend();
        return;
    }
    tf_apply_saved();
    let mut q = [0u8; 96];
    let mut n = 0usize;
    q[0] = 0;
    q[1] = 6;
    n = 2;
    // 顺序跟 tqftpserv tftp_send_oack：blksize、timeoutms、tsize、wsize。
    tf_put_cstr(&mut q, &mut n, b"blksize");
    tf_put_dec(&mut q, &mut n, unsafe { TF_REQ_BLK });
    if n < q.len() {
        q[n] = 0;
        n += 1;
    }
    tf_put_cstr(&mut q, &mut n, b"timeoutms");
    tf_put_dec(&mut q, &mut n, unsafe { TF_REQ_TMO });
    if n < q.len() {
        q[n] = 0;
        n += 1;
    }
    if unsafe { TF_HAS_TS } != 0 {
        tf_put_cstr(&mut q, &mut n, b"tsize");
        tf_put_dec(&mut q, &mut n, unsafe { TZ_SZ });
        if n < q.len() {
            q[n] = 0;
            n += 1;
        }
    }
    // 刀 G6：wsize 恒回显（stock handle_rrq：wsize 缺省 1，`wsize ? &wsize : NULL`
    // 恒真——请求没带也回 wsize=1）。G5 窗实测客户端只带 blksize+timeoutms，
    // 我们每笔 OACK 都缺 wsize，客户端收 OACK 即 EOT 重试 16 笔后弃。
    {
        let wsv = if unsafe { TF_REQ_WSZ } > 0 {
            unsafe { TF_REQ_WSZ }
        } else {
            1
        };
        tf_put_cstr(&mut q, &mut n, b"wsize");
        tf_put_dec(&mut q, &mut n, wsv);
        if n < q.len() {
            q[n] = 0;
            n += 1;
        }
    }
    // 刀 G5→G6：rsize/seek 回显改「值非零」门（stock `rsize ? &rsize : NULL`
    // ——值为 0 不回显：stat 的 rsize=0 键在也不回；段落读 rsize=52 才回）。
    if unsafe { TF_HAS_RS } != 0 && unsafe { TF_RSIZE } != 0 {
        tf_put_cstr(&mut q, &mut n, b"rsize");
        tf_put_dec(&mut q, &mut n, unsafe { TF_RSIZE });
        if n < q.len() {
            q[n] = 0;
            n += 1;
        }
    }
    if unsafe { TF_HAS_SK } != 0 && unsafe { TF_SEEK } != 0 {
        tf_put_cstr(&mut q, &mut n, b"seek");
        tf_put_dec(&mut q, &mut n, unsafe { TF_SEEK });
        if n < q.len() {
            q[n] = 0;
            n += 1;
        }
    }
    // 刀 G13（二分回退）：G12 镜像两窗复现「纯 ping 20s 活、首查 sm 即死」
    // （+8s ping 亡且不回靴）——行为增量只有临时口。本刀 sport 复位 0x11，
    // pq=/ox= 四分位计数全留（一刀两用：若 sm 复活，pq= 直接实测 dv=0 矛盾）。
    let sport = TF_SVC_PORT;
    if glink_tx_payload(
        at478,
        at479,
        tx_head,
        sz479,
        sport,
        unsafe { TF_NODE },
        unsafe { TF_PORT },
        &q[..n],
    ) {
        unsafe { TF_PQ_TXOK = TF_PQ_TXOK.wrapping_add(1) };
        // 刀 G9c：retry 服务只投 OACK，不碰游标/synth——原会话状态原样保留，
        // 重试口的任何回包（ACK/EOT）由 rx 滚动备忘捕获即达判读目的。
        if !retry {
            unsafe {
                TF_XFER_PORT = sport;
                TF_ST = 1;
                TF_BLKNO = 0;
                TF_ACK_PEND = false;
                TF_ACK_OFF = 0xffff_ffff;
                TF_OACK_AT = time::cntvct();
                // 刀 G7：OACK 落环即合成 ACK-0（2s 宽限退役为兜底）。G6 窗2 实测：
                // 客户端 1s 超时窗（tm=3e8）内既不 ACK-0 也不 EOT——18 笔重试全在
                // ping 前打完，2s 后才发的 DATA 块 1 永远扑空（tb=2.1.11 冻结、
                // rx=0 全靴零回包）。若客户端真是 ACK-0 型，真 ACK-0 稍后到也不碍
                // 事：TF_ACK_OFF 去重，且 ack≠expect 的迟到 ACK 直接丢。stat 舞不受
                // 影响——DATA 分支的 rsize=0 门照旧拦住不发。
                TF_ACK_PEND = true;
                TF_ACK_BLK = 0;
            }
        } else {
            unsafe { TF_DR = TF_DR.wrapping_add(1) };
        }
        // 刀 G4：CTRL 副本探针删除。G3 实测它一笔不被消费即冻环（tt 卡 a00.960、
        // grace/DATA 连带冻死）；且其落位公式与 glink_tx_payload 内部回卷纪律
        // 不一致，疑自我写坏。G2 问题已答（cp=1 无反应），不再需要。
        tf_drop_pend();
    } else {
        // 刀 G12：OACK TX 失败计数（环满/回卷闸）。pend 不丢——下圈原样重试
        // 服务，ox=ok.no 与 pq= 合读即知服务环是否在空转。
        unsafe { TF_PQ_TXNO = TF_PQ_TXNO.wrapping_add(1) };
    }
}

/// pm-service / SSREQ（QRTR 0x35）：对账 vendor pm-service 内嵌 IDL。
/// msgid 0x20=SYSTEM_SHUTDOWN / 0x21=SYSTEM_RESTART / 0x22=PERIPHERAL_RESTART
/// （REQ/RESP/IND 同号；max_sz=7）。Linux：RESP result 成功；SHUTDOWN/RESTART
/// 再发 IND status=SERVICED(1)；PERIPHERAL_RESTART 只 RESP。不在此 reboot。
fn pm_reply(at478: u64, at479: u64, sz479: u64) {
    if unsafe { !PM_PEND } || at478 == 0 || at479 == 0 {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        return;
    }
    let msg = unsafe { PM_MSG };
    let txn = unsafe { PM_TXN };
    let node = unsafe { PM_NODE };
    let port = unsafe { PM_PORT };
    // 未知 msgid：失败 RESP，不发 IND（对齐 CSI 拒识）。
    let known = msg >= 0x20 && msg <= 0x22;
    let mut q = [0u8; 16];
    let mut n = 7usize;
    put_result(&mut q, &mut n, !known);
    qmi_finish(&mut q, n, txn, msg);
    if !glink_tx_payload(at478, at479, tx_head, sz479, PM_QRTR_PORT, node, port, &q[..n]) {
        return;
    }
    unsafe {
        PM_PEND = false;
        PM_N = PM_N.wrapping_add(1);
    }
    // SHUTDOWN/RESTART：再发 IND（同 msgid，qtype=4，txn=0xffff，TLV01 status=1）。
    if !known || msg == 0x22 {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let mut ind = [0u8; 16];
    let mut in_ = 7usize;
    put_u8(&mut ind, &mut in_, 1);
    put_u16(&mut ind, &mut in_, 4);
    put_u32(&mut ind, &mut in_, 1);
    qmi_finish_typed(&mut ind, in_, 4, 0xffff, msg);
    let _ = glink_tx_payload(at478, at479, tx_head, sz479, PM_QRTR_PORT, node, port, &ind[..in_]);
}

/// SERVREG_NOTIF PDR：REGISTER_LISTENER(0x20) → IND ACK → 仅 DOWN 时 RESTART_PD。
/// 对齐 Linux service-notifier；不改 TX intent pick/parse/BUSY/RR。
/// 门控：HOST_ADV>=4 且 RFS IOVEC 已绿（RF_IOK>=4 ≡ sm ik=4.x），避免
/// node-0 放行后过早 REGISTER/RESTART 堵 early path（206821a 硬挂）。
/// UNINIT(0x7fffffff) 是真状态：只听 IND，不对 UNINIT 发 RESTART。
fn sr_tick(at478: u64, at479: u64, sz479: u64) {
    if at478 == 0 || at479 == 0 || unsafe { HOST_ADV } < 4 || !unsafe { HAVE_IID } {
        return;
    }
    // RFS 绿后再发 PDR；ik= 左栏即 RF_IOK。
    if unsafe { RF_IOK } < 4 {
        return;
    }
    // modem 可在 node 0；有 port 即可发 REGISTER。
    if !unsafe { SR_HAVE } || unsafe { SR_PORT } == 0 {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        return;
    }
    // 一笔 REGISTER。
    if unsafe { SR_HIT } == 0 && !unsafe { SR_PEND } {
        let txn = unsafe { SR_TXN };
        let mut q = [0u8; 64];
        let mut n = 7usize;
        put_u8(&mut q, &mut n, 1);
        put_u16(&mut q, &mut n, 1);
        put_u8(&mut q, &mut n, 1);
        put_u8(&mut q, &mut n, 2);
        put_u16(&mut q, &mut n, SR_NAME.len() as u16);
        for &b in SR_NAME {
            put_u8(&mut q, &mut n, b);
        }
        qmi_finish_typed(&mut q, n, 0, txn, 0x20);
        if glink_tx_payload(
            at478,
            at479,
            tx_head,
            sz479,
            SR_QRTR_PORT,
            unsafe { SR_NODE },
            unsafe { SR_PORT },
            &q[..n],
        ) {
            unsafe {
                SR_HIT = 1;
                SR_PEND = true;
                SR_TXN = SR_TXN.wrapping_add(1);
            }
        }
        return;
    }
    // STATE_UPDATED IND → SET_ACK（msgid 0x23），对齐 Linux。
    if unsafe { SR_ACK_PEND } && !unsafe { SR_PEND } && !unsafe { SR_RP_PEND } {
        let txn = unsafe { SR_TXN };
        let ack_tid = unsafe { SR_ACK_TXN };
        let mut q = [0u8; 80];
        let mut n = 7usize;
        put_u8(&mut q, &mut n, 1);
        put_u16(&mut q, &mut n, SR_NAME.len() as u16);
        for &b in SR_NAME {
            put_u8(&mut q, &mut n, b);
        }
        put_u8(&mut q, &mut n, 2);
        put_u16(&mut q, &mut n, 2);
        put_u16(&mut q, &mut n, ack_tid);
        qmi_finish_typed(&mut q, n, 0, txn, 0x23);
        if glink_tx_payload(
            at478,
            at479,
            tx_head,
            sz479,
            SR_QRTR_PORT,
            unsafe { SR_NODE },
            unsafe { SR_PORT },
            &q[..n],
        ) {
            unsafe {
                SR_ACK_PEND = false;
                SR_TXN = SR_TXN.wrapping_add(1);
            }
        }
        return;
    }
    // 仅 DOWN 才 RESTART_PD（Linux pd_restart 路径）；不对 UNINIT。
    if unsafe { SR_NEED_RP } && unsafe { SR_RP_N } == 0 && !unsafe { SR_RP_PEND } && !unsafe { SR_PEND }
    {
        let txn = unsafe { SR_TXN };
        let mut q = [0u8; 64];
        let mut n = 7usize;
        put_u8(&mut q, &mut n, 1);
        put_u16(&mut q, &mut n, SR_NAME.len() as u16);
        for &b in SR_NAME {
            put_u8(&mut q, &mut n, b);
        }
        qmi_finish_typed(&mut q, n, 0, txn, 0x24);
        if glink_tx_payload(
            at478,
            at479,
            tx_head,
            sz479,
            SR_QRTR_PORT,
            unsafe { SR_NODE },
            unsafe { SR_PORT },
            &q[..n],
        ) {
            unsafe {
                SR_RP_N = 1;
                SR_RP_PEND = true;
                SR_NEED_RP = false;
                SR_TXN = SR_TXN.wrapping_add(1);
            }
        }
    }
}

/// IPA QMI INIT_MODEM_DRIVER（msgid 0x0021）— 在 iq 闩 + GSI CH7 STARTED 后单发。
/// TLV 对齐 vendor ipa3_qmi_init_modem_send_sync_msg + ipa_4_7_mem_part；R=IG_R（活体 0）。
/// 冷启省略 is_ssr_bootup；modem_comp_decomp=0 省略 zip。hashable（lito 无 not-hashable）。
/// sm ii=st.txn.res。不改 CDSP/TFTP/TX/glink intent。
fn ipa_init_tick(at478: u64, at479: u64, sz479: u64) {
    if at478 == 0 || at479 == 0 || !unsafe { HAVE_IID } {
        unsafe { II_WHY = 1 };
        return;
    }
    // 已发或已回：不再发。
    if unsafe { II_SENT } != 0 || unsafe { II_PEND } || unsafe { II_ST } == 2 {
        unsafe { II_WHY = 2 };
        return;
    }
    // 前置：iq 0x31/0x201 + port；GSI CH7 STARTED；ig 已采（拿 R）。
    if unsafe { IQ_SEEN } == 0 || unsafe { IQ_PORT } == 0 {
        unsafe { II_WHY = 3 };
        return;
    }
    // stock qrtr：0x31 inst 0x201；无精确实例则等偏置闩。
    if unsafe { IQ_INST } != 0x201 {
        unsafe { II_WHY = 4 };
        return;
    }
    if unsafe { GC_ST } != 1 || unsafe { IG_ST } != 1 || unsafe { IG_R } == 0xffff_ffff {
        unsafe { II_WHY = 5 };
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        unsafe {
            II_WHY = 6;
            II_HX = tx_head as u32;
            II_TL = tx_tail as u32;
            // 0 cmd, 1 intent, 2 chunk, 7 src_port, 9 size, 10 dst_node, 11 dst_port, 12 qmi
            let pick = [0u64, 1, 2, 7, 9, 10, 11, 12];
            let mut k = 0usize;
            while k < 8 {
                let off = tx_tail + pick[k] * 4;
                II_PK[k] = if at479 != 0 && off + 4 <= sz479 {
                    rd32((at479 + off) as usize)
                } else {
                    0
                };
                k += 1;
            }
        }
        return;
    }
    let r = unsafe { IG_R };
    let txn = 1u32;
    // 体 ≤162；头 7 → 共 ~154。缓冲 192。
    let mut q = [0u8; 192];
    let mut n = 7usize;
    // 0x10 platform_type = 3 (MSM_ANDROID)
    put_u8(&mut q, &mut n, 0x10);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 3);
    // 0x11 hdr_tbl_info — modem_hdr 0x688 / size 0x240 → end 0x8c7
    put_u8(&mut q, &mut n, 0x11);
    put_u16(&mut q, &mut n, 8);
    put_u32(&mut q, &mut n, 0x688 + r);
    put_u32(&mut q, &mut n, 0x688 + r + 0x240 - 1);
    // 0x12 v4_route_tbl (nhash) — 0x508, indices=7
    put_u8(&mut q, &mut n, 0x12);
    put_u16(&mut q, &mut n, 8);
    put_u32(&mut q, &mut n, 0x508 + r);
    put_u32(&mut q, &mut n, 7);
    // 0x13 v6_route_tbl — 0x608, indices=7
    put_u8(&mut q, &mut n, 0x13);
    put_u16(&mut q, &mut n, 8);
    put_u32(&mut q, &mut n, 0x608 + r);
    put_u32(&mut q, &mut n, 7);
    // 0x14 v4_filter nhash — 0x308
    put_u8(&mut q, &mut n, 0x14);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 0x308 + r);
    // 0x15 v6_filter nhash — 0x408
    put_u8(&mut q, &mut n, 0x15);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 0x408 + r);
    // 0x16 modem_mem — 0x27d8 / 0x800
    put_u8(&mut q, &mut n, 0x16);
    put_u16(&mut q, &mut n, 8);
    put_u32(&mut q, &mut n, 0x27d8 + r);
    put_u32(&mut q, &mut n, 0x800);
    // 0x17 去掉后端点 16 后，ping 一通就掉，没读到 sm。先加回。
    put_u8(&mut q, &mut n, 0x17);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 16);
    // 0x18 is_ssr_bootup — 冷启省略（stock valid=false）
    // 0x19 hdr_proc_ctx — 0xad0 / 0xac0 → end 0x158f
    put_u8(&mut q, &mut n, 0x19);
    put_u16(&mut q, &mut n, 8);
    put_u32(&mut q, &mut n, 0xad0 + r);
    put_u32(&mut q, &mut n, 0xad0 + r + 0xac0 - 1);
    // 0x1A zip — size 0 省略
    // 0x1B..0x1E hash tables（lito 无 ipa-fltrt-not-hashable）
    put_u8(&mut q, &mut n, 0x1b);
    put_u16(&mut q, &mut n, 8);
    put_u32(&mut q, &mut n, 0x488 + r);
    put_u32(&mut q, &mut n, 7);
    put_u8(&mut q, &mut n, 0x1c);
    put_u16(&mut q, &mut n, 8);
    put_u32(&mut q, &mut n, 0x588 + r);
    put_u32(&mut q, &mut n, 7);
    put_u8(&mut q, &mut n, 0x1d);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 0x288 + r);
    put_u8(&mut q, &mut n, 0x1e);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 0x388 + r);
    // 0x1F/0x20 hw_stats_quota — base 0x2500 size 0x30
    put_u8(&mut q, &mut n, 0x1f);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 0x2500 + r);
    put_u8(&mut q, &mut n, 0x20);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 0x30);
    // 0x21/0x22 hw_drop_stats — base 0x27b0 size 0x20
    put_u8(&mut q, &mut n, 0x21);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 0x27b0 + r);
    put_u8(&mut q, &mut n, 0x22);
    put_u16(&mut q, &mut n, 4);
    put_u32(&mut q, &mut n, 0x20);

    qmi_finish_typed(&mut q, n, 0, txn, IPA_INIT_MSGID);
    if glink_tx_payload(
        at478,
        at479,
        tx_head,
        sz479,
        IPA_QRTR_PORT,
        unsafe { IQ_NODE },
        unsafe { IQ_PORT },
        &q[..n],
    ) {
        unsafe {
            II_WHY = 0;
            II_TXN = txn;
            II_PEND = true;
            II_SENT = 1;
            II_ST = 1;
            II_RES = 0xffff_ffff;
        }
    } else {
        unsafe {
            II_WHY = 7;
            II_ST = 3;
            II_SENT = 1;
        }
    }
}

/// 回 INIT_MODEM_DRIVER_CMPLT_RESP：只有 TLV 0x02 result 成功。
fn ipa_cmplt_tick(at478: u64, at479: u64, sz479: u64) {
    if !unsafe { IC_PEND } || at478 == 0 || at479 == 0 || !unsafe { HAVE_IID } {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        return;
    }
    let txn = unsafe { IC_TXN };
    let mut q = [0u8; 32];
    let mut n = 7usize;
    put_result(&mut q, &mut n, false);
    qmi_finish_typed(&mut q, n, 2, txn, 0x35);
    if glink_tx_payload(
        at478,
        at479,
        tx_head,
        sz479,
        IPA_QRTR_PORT,
        unsafe { IC_NODE },
        unsafe { IC_PORT },
        &q[..n],
    ) {
        unsafe {
            IC_PEND = false;
            IC_SENT = IC_SENT.wrapping_add(1);
        }
    }
}

/// QMI_IPA_CONFIG_RESP：只有 TLV 0x02 result 成功。从服务端口 0x16 回。
fn ipa_cfg_tick(at478: u64, at479: u64, sz479: u64) {
    if unsafe { CFG_ARM } == 0 || !unsafe { CFG_PEND } || at478 == 0 || at479 == 0 || !unsafe { HAVE_IID }
    {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        return;
    }
    let txn = unsafe { CFG_TXN };
    let mut q = [0u8; 32];
    let mut n = 7usize;
    put_result(&mut q, &mut n, false);
    qmi_finish_typed(&mut q, n, 2, txn, 0x27);
    if glink_tx_payload(
        at478,
        at479,
        tx_head,
        sz479,
        IPA_SVC_PORT,
        unsafe { CFG_NODE },
        unsafe { CFG_PORT },
        &q[..n],
    ) {
        unsafe {
            CFG_DONE_TXN = txn;
            CFG_DONE_PORT = CFG_PORT;
            CFG_PEND = false;
            CFG_SENT = CFG_SENT.wrapping_add(1);
        }
    }
}

/// INDICATION_REGISTER_RESP：TLV 0x02 成功。
fn ipa_ir_tick(at478: u64, at479: u64, sz479: u64) {
    if !unsafe { IR_PEND } || at478 == 0 || at479 == 0 || !unsafe { HAVE_IID } {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        return;
    }
    let txn = unsafe { IR_TXN };
    let mut q = [0u8; 32];
    let mut n = 7usize;
    put_result(&mut q, &mut n, false);
    qmi_finish_typed(&mut q, n, 2, txn, 0x20);
    if glink_tx_payload(
        at478,
        at479,
        tx_head,
        sz479,
        IPA_SVC_PORT,
        unsafe { IR_NODE },
        unsafe { IR_PORT },
        &q[..n],
    ) {
        unsafe {
            IR_PEND = false;
            IR_DONE = 1;
        }
    }
}

/// INIT 已完成且 0x20 已回：MASTER_DRIVER_INIT_COMPLETE 指示，qtype 4，msgid 0x22。
fn ipa_ind_tick(at478: u64, at479: u64, sz479: u64) {
    if unsafe { IR_DONE } == 0 || unsafe { IND_SENT } != 0 || unsafe { II_ST } != 2 {
        return;
    }
    if at478 == 0 || at479 == 0 || !unsafe { HAVE_IID } {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        return;
    }
    let mut q = [0u8; 32];
    let mut n = 7usize;
    put_result(&mut q, &mut n, false);
    qmi_finish_typed(&mut q, n, 4, 0, 0x22);
    if glink_tx_payload(
        at478,
        at479,
        tx_head,
        sz479,
        IPA_SVC_PORT,
        unsafe { IR_NODE },
        unsafe { IR_PORT },
        &q[..n],
    ) {
        unsafe { IND_SENT = 1; }
    }
}

fn pd_reply(at478: u64, at479: u64, sz479: u64) {
    if unsafe { !PD_PEND || !HAVE_IID } || at478 == 0 || at479 == 0 {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        return;
    }
    let name_n = unsafe { PD_NAME_N } as usize;
    let mut name = [0u8; 64];
    unsafe {
        let mut i = 0;
        while i < name_n && i < 64 {
            name[i] = PD_NAME[i];
            i += 1;
        }
    }
    let mut hits: [&Dom; 8] = [&DOMS[0]; 8];
    let mut nh = 0usize;
    for d in DOMS {
        if d.svc.len() == name_n && d.svc == &name[..name_n] && nh < 8 {
            hits[nh] = d;
            nh += 1;
        }
    }
    let mut body = 7 + 5 + 5;
    if nh > 0 {
        body += 4;
        for d in hits.iter().take(nh) {
            body += 1 + d.dom.len() + 9;
        }
    }
    let need = 7 + body;
    if need > 256 {
        return;
    }
    unsafe {
        PD_HITS = nh as u8;
    }
    let mut q = [0u8; 256];
    let mut n = 7usize;
    put_u8(&mut q, &mut n, 2);
    put_u16(&mut q, &mut n, 4);
    put_u16(&mut q, &mut n, 0);
    put_u16(&mut q, &mut n, 0);
    put_u8(&mut q, &mut n, 16);
    put_u16(&mut q, &mut n, 2);
    put_u16(&mut q, &mut n, nh as u16);
    put_u8(&mut q, &mut n, 17);
    put_u16(&mut q, &mut n, 2);
    put_u16(&mut q, &mut n, 1);
    if nh > 0 {
        let mut list = 1usize;
        for d in hits.iter().take(nh) {
            list += 1 + d.dom.len() + 4 + 1 + 4;
        }
        put_u8(&mut q, &mut n, 18);
        put_u16(&mut q, &mut n, list as u16);
        put_u8(&mut q, &mut n, nh as u8);
        for d in hits.iter().take(nh) {
            put_dom(&mut q, &mut n, d.dom, d.inst);
        }
    }
    if n != need {
        return;
    }
    let msglen = (n - 7) as u16;
    unsafe {
        PD_QLEN = msglen;
    }
    qmi_finish(&mut q, n, unsafe { PD_TXN }, 33);
    let _ = msglen;
    if glink_tx_payload(
        at478,
        at479,
        tx_head,
        sz479,
        0x12,
        unsafe { PD_NODE },
        unsafe { PD_PORT },
        &q[..n],
    ) {
        unsafe {
            if PD_SEEN_N < 8 {
                PD_SEEN[PD_SEEN_N as usize] = PD_TXN;
                PD_SEEN_N += 1;
            }
            PD_PEND = false;
            PD_N += 1;
        }
    }
}

fn glink_put_open(fifo: u64, tx: u64, lcid: u32, name: &[u8]) -> u64 {
    let nlen = name.len() as u32;
    let total = (8 + nlen as u64 + 7) & !7;
    let mut i = 0u64;
    while i < total {
        wr32((fifo + tx + i) as usize, 0);
        i += 4;
    }
    wr32((fifo + tx) as usize, 2 | (lcid << 16));
    wr32((fifo + tx + 4) as usize, nlen);
    let mut o = 0usize;
    while o < name.len() {
        let mut b = [0u8; 4];
        let mut k = 0;
        while k < 4 && o + k < name.len() {
            b[k] = name[o + k];
            k += 1;
        }
        wr32((fifo + tx + 8 + o as u64) as usize, u32::from_le_bytes(b));
        o += 4;
    }
    tx + total
}

/// 428 不在、自由指针处是空位时，按 smem + smp2p 的顺序分配出向项并踢 IPCC。
/// 返回 (k, 新的自由偏移, offset_free_cached)。k：0 已有，1 写过并踢了，
/// 2 塞不进 cached 区，3 没拿到 mutex，4 锁里发现自由指针变了。
fn smp_publish(base: u64, free_u: u32, g: u32, boff: u32, b0: u32, b1: u32) -> (u32, u32, u32) {
    let cached = rd32((base + 16) as usize);
    if g & 2 != 0 {
        if unsafe { MPSS_PUB } == 0xffff_ffff {
            unsafe { MPSS_PUB = 0 };
        }
        return (0, free_u, cached);
    }
    if boff != free_u || b0 != 0 || b1 != 0 {
        return (0, free_u, cached);
    }
    // 16 字节头 + 344（340 按 8 对齐）。
    const ALLOC: u32 = 0x168;
    if free_u.saturating_add(ALLOC) > cached {
        return (2, free_u, cached);
    }
    const MUTEX: usize = 0x01f4_3000;
    let mut got = false;
    for _ in 0..256 {
        wr32(MUTEX, 1);
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        if rd32(MUTEX) == 1 {
            got = true;
            break;
        }
    }
    if !got {
        return (3, free_u, cached);
    }
    let now = rd32((base + 12) as usize);
    if now != free_u {
        wr32(MUTEX, 0);
        return (4, now, cached);
    }
    let hdr = base + free_u as u64;
    // canary 0xA5A5，item 428 = 0x1AC。
    wr32(hdr as usize, 0x01ac_a5a5);
    wr32((hdr + 4) as usize, 0x158);
    wr32((hdr + 8) as usize, 4);
    wr32((hdr + 12) as usize, 0);
    let data = hdr + 16;
    for i in 0..86u64 {
        wr32((data + i * 4) as usize, 0);
    }
    wr32(data as usize, 0x504d_5324);
    wr32((data + 4) as usize, 1 << 8);
    wr32((data + 8) as usize, 1 << 16);
    // Linux lito-smp2p：出向 master-kernel + ipa（valid=2）；入向另有
    // slave-kernel / ipa / wlan（modem 侧建）。缺 ipa 出向时 modem 可能
    // 不走内存内自拉 wlan_pd，掉进 elf_loader+TFTP。
    wr32((data + 12) as usize, 16 | (2 << 16));
    // entries[0] "master-kernel"
    wr32((data + 20) as usize, 0x7473_616d);
    wr32((data + 24) as usize, 0x6b2d_7265);
    wr32((data + 28) as usize, 0x656e_7265);
    wr32((data + 32) as usize, 0x6c);
    // entries[1] "ipa" @ +40；value @ +56 = 0（已在上面清零）
    wr32((data + 40) as usize, 0x0061_7069);
    unsafe { IPA_SMP_VAL = data + 56; }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    unsafe { (data as *mut u8).add(4).write_volatile(1) };
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    let new_free = free_u + ALLOC;
    wr32((base + 12) as usize, new_free);
    dcache_civac(base, 64);
    wr32(MUTEX, 0);
    // MPSS client 2，SMP2P signal 2。
    wr32(0x4080_0c, (2 << 16) | 2);
    unsafe {
        MPSS_PUB = 1;
        MPSS_VOUT = 2;
    }
    (1, new_free, cached)
}

/// modem 建了 ipa 入向之后，按 ipa3_smp2p_modem_clk_query_isr 回 bit0|bit1。
fn ipa_smp_clk() {
    let addr = unsafe { IPA_SMP_VAL };
    if addr == 0 || unsafe { IPA_SMP_SENT } != 0 || unsafe { MPSS_IN } & 1 == 0 {
        return;
    }
    wr32(addr as usize, 3);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    dcache_civac(addr, 4);
    wr32(0x4080_0c, (2 << 16) | 2);
    unsafe { IPA_SMP_SENT = 1; }
}

/// slave-kernel 的值里，bit1 是 ONLINE。高位 0x80000000 是「还没读到条目」。
/// `proxy_unvote_intr_handler`：bit2 到了就把 lcx/lmx 的代理票撤成 0。
fn adsp_proxy_unvote(freq: u32) {
    if unsafe { ADSP_UV } != 0xffff_ffff {
        return;
    }
    let lcx = unsafe { ADSP_P[3] };
    let lmx = unsafe { ADSP_P[4] };
    let scx = if lcx != 0 { rpmh_cmd(lcx, 0, freq) } else { 0 };
    let smx = if lmx != 0 { rpmh_cmd(lmx, 0, freq) } else { 0 };
    unsafe { ADSP_UV = ((scx & 0xffff) << 16) | (smx & 0xffff) };
}

fn adsp_online(v: u32) -> bool {
    v & 0x8000_0000 == 0 && v & 2 != 0
}

/// 读一项 SMP2P 里的 slave-kernel。443 不在返回 0x80000000，魔数不对返回
/// 0x80000001，名字还没出现返回 0x80000004。这三个都没有 bit1。
fn lpass_slave(data: u64) -> u32 {
    let Some(magic) = smem_rd32(data) else {
        return 0x8000_0001;
    };
    if magic != 0x504d_5324 {
        return 0x8000_0001;
    }
    let Some(hdr) = smem_rd32(data + 12) else {
        return 0x8000_0001;
    };
    let n = ((hdr >> 16) & 0xffff).min(16);
    let mut i = 0u32;
    while i < n {
        let ent = data + 20 + i as u64 * 20;
        let Some(w0) = smem_rd32(ent) else {
            return 0x8000_0001;
        };
        let Some(w1) = smem_rd32(ent + 4) else {
            return 0x8000_0001;
        };
        let Some(w2) = smem_rd32(ent + 8) else {
            return 0x8000_0001;
        };
        let Some(w3) = smem_rd32(ent + 12) else {
            return 0x8000_0001;
        };
        if w0 == 0x7661_6c73 && w1 == 0x656b_2d65 && w2 == 0x6c65_6e72 && w3 == 0 {
            return smem_rd32(ent + 16).unwrap_or(0x8000_0001);
        }
        i += 1;
    }
    0x8000_0004
}

/// 入向项出现之后按 smp2p.c 做一次协商。版本都是 1 时，特性取交集。
/// flags bit0 是 RESTART_DONE，出向 flags bit1 回 RESTART_ACK，然后踢 IPCC。
fn lpass_negotiate(out_item: u64, in_item: u64) {
    if unsafe { LPASS_ACKED } != 0 {
        return;
    }
    let Some(iw4) = smem_rd32(in_item + 4) else {
        unsafe { ADSP_ACK = 5 };
        return;
    };
    let Some(iw12) = smem_rd32(in_item + 12) else {
        unsafe { ADSP_ACK = 5 };
        return;
    };
    let Some(iw16) = smem_rd32(in_item + 16) else {
        unsafe { ADSP_ACK = 5 };
        return;
    };
    unsafe {
        ADSP_IN[0] = iw4;
        ADSP_IN[1] = iw12;
        ADSP_IN[2] = iw16;
    }
    if (iw4 & 0xff) == 0 {
        unsafe { ADSP_ACK = 2 };
        return;
    }
    if out_item == 0 {
        unsafe {
            ADSP_ACK = 3;
            LPASS_ACKED = 1;
        }
        return;
    }
    if (iw4 & 0xff) != 1 {
        unsafe {
            ADSP_ACK = 2;
            LPASS_ACKED = 1;
        }
        return;
    }
    let Some(ow4) = smem_rd32(out_item + 4) else {
        unsafe { ADSP_ACK = 5 };
        return;
    };
    let Some(ow16) = smem_rd32(out_item + 16) else {
        unsafe { ADSP_ACK = 5 };
        return;
    };
    let neg = (iw4 >> 8) & (ow4 >> 8);
    let mut changed = false;
    if neg != (ow4 >> 8) {
        let addr = out_item + 4;
        wr32(addr as usize, (neg << 8) | 1);
        if addr < 0x80A0_0000 {
            dcache_civac(addr, 4);
        }
        changed = true;
    }
    if iw16 & 1 != 0 && ow16 & 2 == 0 {
        let addr = out_item + 16;
        wr32(addr as usize, ow16 | 2);
        if addr < 0x80A0_0000 {
            dcache_civac(addr, 4);
        }
        changed = true;
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    if changed {
        wr32(0x4080_0c, (3 << 16) | 2);
        unsafe { ADSP_ACK = 1 };
    } else {
        unsafe { ADSP_ACK = 4 };
    }
    unsafe { LPASS_ACKED = 1 };
}

/// 在已经换成 NC 的 TOC 窗口里做 LPASS 边。
/// `qcom,smp2p-adsp`：分区 host 0/2，入向 443，出向 429，remote-pid 2。
/// pub_out 时按 smp2p.c 写下 master-kernel，force-stop 保持 0，再踢
/// IPCC client LPASS(3) signal SMP2P(2)。
/// 返回 (代码, slave 值, 分区偏移)。代码：0 出向项已在，1 写过并踢了，
/// 2 塞不进 cached 区，3 没锁，4 自由指针变了，5 没有这块分区，
/// 6 分区魔数不对，7 TOC 不对，8 链没落到自由指针，9 项碰到 TOC 保护区。
fn lpass_edge_in(pub_out: bool, smem_base: u64, toc: u64) -> (u32, u32, u32) {
    let magic = rd32(toc as usize);
    if magic != 0x434f_5424 {
        return (7, 0x8000_0000, 0);
    }
    let nent = rd32((toc + 8) as usize).min(48);
    let mut part = 0u32;
    let mut e = toc + 32;
    for _ in 0..nent {
        let off = rd32(e as usize);
        let sz = rd32((e + 4) as usize);
        let h0 = rd16a(e + 12) as u32;
        let h1 = rd16a(e + 14) as u32;
        e += 48;
        if off == 0 || sz < 32 || off >= 0x20_0000 || off + sz > 0x20_0000 {
            continue;
        }
        if (h0 == 0 && h1 == 2) || (h0 == 2 && h1 == 0) {
            part = off;
            break;
        }
    }
    if part == 0 {
        return (5, 0x8000_0000, 0);
    }
    let base = smem_base + part as u64;
    let Some(pm) = smem_rd32(base) else {
        return (6, 0x8000_0000, part);
    };
    if pm != 0x5452_5024 {
        return (6, 0x8000_0000, part);
    }
    let Some(psz) = smem_rd32(base + 8) else {
        return (6, 0x8000_0000, part);
    };
    let Some(free_u) = smem_rd32(base + 12) else {
        return (6, 0x8000_0000, part);
    };
    let Some(cached) = smem_rd32(base + 16) else {
        return (6, 0x8000_0000, part);
    };
    let room = 0x20_0000 - part;
    let end_off = free_u.min(psz).min(room);
    let mut at429 = 0u64;
    let mut at443 = 0u64;
    let mut boff = 0u32;
    let mut slot_ok = false;
    if end_off >= 32 {
        let end = base + end_off as u64;
        let mut p = base + 32;
        for _ in 0..128 {
            if p + 16 > end || p + 16 > 0x80B0_0000 || (p < 0x80A0_0000 && p + 16 > 0x80A0_0000) {
                boff = (p - base) as u32;
                slot_ok = boff == free_u;
                break;
            }
            let Some(can) = smem_rd32(p) else {
                return (8, 0x8000_0000, part);
            };
            if (can as u16) != 0xa5a5 {
                boff = (p - base) as u32;
                slot_ok = boff == free_u && can == 0 && smem_rd32(p + 4) == Some(0);
                break;
            }
            let item = (can >> 16) as u16;
            let Some(isz_w) = smem_rd32(p + 4) else {
                return (8, 0x8000_0000, part);
            };
            let Some(pads) = smem_rd32(p + 8) else {
                return (8, 0x8000_0000, part);
            };
            let isz = isz_w as u64;
            let pad = (pads >> 16) as u64;
            let data = p + 16 + pad;
            if item == 429 {
                at429 = data;
            }
            if item == 443 {
                at443 = data;
            }
            let next = p + 16 + pad + isz;
            if next <= p || isz > 0x2_0000 {
                return (8, 0x8000_0000, part);
            }
            p = next;
        }
    }
    let slave = if at443 != 0 {
        lpass_slave(at443)
    } else {
        0x8000_0000
    };
    if !pub_out && at443 != 0 {
        lpass_negotiate(at429, at443);
    }
    if !pub_out || at429 != 0 {
        let code = if pub_out {
            0
        } else if at443 == 0 {
            0x10
        } else if slave == 0x8000_0004 {
            0x12
        } else if slave & 0x8000_0000 != 0 {
            0x13
        } else {
            0x11
        };
        return (code, slave, part);
    }
    if !slot_ok {
        return (8, slave, part);
    }
    const ALLOC: u32 = 0x168;
    if free_u.saturating_add(ALLOC) > cached {
        return (2, slave, part);
    }
    let hdr = base + free_u as u64;
    let item_end = hdr + ALLOC as u64;
    unsafe { LPASS_HDR = hdr; }
    // 0x80A00000 以上此刻是 NC。先在 WB 下把这一项的缓存行作废，再进来写。
    // 0x80AFE000 起到 TOC（0x80AFF000）不碰。横跨 0x80A00000 的项也不写。
    let upper = hdr >= 0x80A0_0000;
    if item_end > 0x80AFE000
        || base >= 0x80AFE000
        || (!upper && item_end > 0x80A0_0000)
    {
        return (9, slave, part);
    }
    if upper && unsafe { LPASS_FLUSHED } != hdr {
        return (9, slave, part);
    }
    const MUTEX: usize = 0x01f4_3000;
    let mut got = false;
    for _ in 0..256 {
        wr32(MUTEX, 1);
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        if rd32(MUTEX) == 1 {
            got = true;
            break;
        }
    }
    if !got {
        return (3, slave, part);
    }
    let Some(now) = smem_rd32(base + 12) else {
        wr32(MUTEX, 0);
        return (6, slave, part);
    };
    if now != free_u {
        wr32(MUTEX, 0);
        return (4, slave, part);
    }
    // canary 0xA5A5，item 429 = 0x1AD。size 0x158，padding_data 4。
    wr32(hdr as usize, 0x01ad_a5a5);
    wr32((hdr + 4) as usize, 0x158);
    wr32((hdr + 8) as usize, 4);
    wr32((hdr + 12) as usize, 0);
    let data = hdr + 16;
    for i in 0..86u64 {
        wr32((data + i * 4) as usize, 0);
    }
    wr32(data as usize, 0x504d_5324);
    // features = SSR_ACK，version 先留 0。local pid 0，remote pid 2。
    wr32((data + 4) as usize, 1 << 8);
    wr32((data + 8) as usize, 2 << 16);
    wr32((data + 12) as usize, 16 | (1 << 16));
    wr32((data + 20) as usize, 0x7473_616d);
    wr32((data + 24) as usize, 0x6b2d_7265);
    wr32((data + 28) as usize, 0x656e_7265);
    wr32((data + 32) as usize, 0x6c);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    unsafe { (data as *mut u8).add(4).write_volatile(1) };
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32((base + 12) as usize, free_u + ALLOC);
    if upper {
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        if base < 0x80A0_0000 {
            dcache_civac(base, 64);
        }
    } else {
        dcache_civac(hdr, ALLOC as u64);
        dcache_civac(base, 64);
    }
    wr32(MUTEX, 0);
    // LPASS client 3，SMP2P signal 2。
    wr32(0x4080_0c, (3 << 16) | 2);
    (1, slave, part)
}

/// LPASS GLINK。`glink_adsp` 是 remote-pid 2，描述符 478，发送 FIFO 479，
/// 接收 FIFO 480 由 ADSP 分配。IPCC 是 client 3、signal 0。
static mut LG_ST: u32 = 0;
static mut LG_PHASE: u8 = 0;
static mut LG_AT: u64 = 0;
static mut LG_LEN: u64 = 0;
static mut LG_FLUSH: u64 = 0;
static mut LG_D: u64 = 0;
static mut LG_TX: u64 = 0;
static mut LG_TXN: u64 = 0;
static mut LG_RX: u64 = 0;
static mut LG_RXN: u64 = 0;
static mut LG_IID: u32 = 0;
static mut LG_ISZ: u32 = 0;
static mut LG_HELLO: u8 = 0;
static mut LG_HN: u32 = 0;
static mut LG_HVIS: u64 = 0;
/// bit0..3 看见对方 OPEN，4..7 我们的 OPEN 已发，8..11 对方 ACK，
/// 12..15 intent 发完，16 已回 VERSION_ACK。通道序：
/// IPCRTR、glink_ssr、apr_audio_svc、fastrpcglink-apps-dsp。
static mut LG_BITS: u32 = 0;
static mut LG_NEED: u8 = 0;
static mut LG_ISENT: [u8; 4] = [0; 4];
/// soak 转过一圈之后才允许发 fastrpc 的 OPEN。提前发会把 USB 弄没。
static mut LG_ARM: u8 = 0;
/// fastrpc 通道上第一包 TX_DATA 的前 32 字节。n 是包数。
/// 对方发包时 param1 是他们的 lcid（我们的 rcid），不是我们的 lcid 4。
static mut LG_FP: [u32; 8] = [0; 8];
static mut LG_FPN: u32 = 0;
static mut FP_RXP: u32 = 0xffff_ffff;
/// USB 用的 SMMU CB。0xffffffff 还没装上。
static mut USB_CB: u32 = 0xffff_ffff;
/// fastrpc 对方的 rcid、给我们的 intent。0xffffffff 还没有。
static mut FP_RCID: u32 = 0xffff_ffff;
static mut FP_IID: u32 = 0;
static mut FP_ISZ: u32 = 0;
/// 见到过的 intent，每条 lcid、id、size。OPEN 可能比 intent 晚到。
static mut FP_ILOG: [u32; 24] = [0; 24];
static mut FP_IN: u32 = 0;
/// 0 还没发。1 已发 INIT_ATTACH_SENSORS。2 SMMU 没装上。3 页不在映射里。
/// 4 已发 remotectl_open。6 attach 的 retval 不是 0。
static mut FP_SENT: u8 = 0;
/// 对方还了我们用掉的 intent。1 可再用，2 被删掉了。
static mut FP_REUSE: u8 = 0;
/// 上一包 TX_DATA 用的 intent。RX_DONE 对这个 id。
static mut FP_LAST: u32 = 0xffff_ffff;
/// ADSP GLINK 发送环。CH/CT 是现在的 head/tail。SH/ST 是 init2 提交时的。
static mut FP_CH: u32 = 0xffff_ffff;
static mut FP_CT: u32 = 0xffff_ffff;
static mut FP_SH: u32 = 0xffff_ffff;
static mut FP_ST: u32 = 0xffff_ffff;
/// fastrpc 这条 rcid 上全部 RX_DONE，不限 intent id。
static mut FP_DN: u32 = 0;
static mut FP_RDP: u32 = 0xffff_ffff;
/// intent 被删掉之后：1 新 intent 已经到了，2 正在要。
static mut FP_INEW: u8 = 0;
/// init2 前多交的那一个 RX intent（cmd5）。0 未交，1 已交。
static mut FP_IXTRA: u8 = 0;
/// 已向 TX 环塞过 READ_NOTIF（cmd13）。内核 sent_read_notify 同形。
static mut FP_RNOTIF: u8 = 0;
/// 已经对对方的应答回过 RX_DONE_W_REUSE。第二包另计。
static mut FP_RDONE: u8 = 0;
static mut FP_ODONE: u8 = 0;
/// 对方那包 TX_DATA 用的 liid。0 还没见到。
static mut FP_RLIID: u32 = 0;
static mut FP_OLIID: u32 = 0;
/// adsp_default_listener_register 的应答。
static mut FP_REGN: u8 = 0;
static mut FP_REG: [u32; 4] = [0; 4];
static mut FP_GLIID: u32 = 0;
static mut FP_GDONE: u8 = 0;
/// remotectl_close 的应答，以及页内 dlret。
static mut FP_CLSN: u8 = 0;
static mut FP_CLS: [u32; 4] = [0; 4];
static mut FP_CLIID: u32 = 0;
static mut FP_CDONE: u8 = 0;
static mut FP_CDL: u32 = 0xffff_ffff;
/// adsp_listener_init2 的应答。
static mut FP_ININ: u8 = 0;
static mut FP_INI: [u32; 4] = [0; 4];
static mut FP_ILIID: u32 = 0;
static mut FP_IDONE: u8 = 0;
/// 第一次 adsp_listener_next2 的应答，以及输出缓冲里的四个字。
static mut FP_NXN: u8 = 0;
static mut FP_NX: [u32; 4] = [0; 4];
static mut FP_NOUT: [u32; 4] = [0xffff_ffff; 4];
/// reverse tunnel：下一包 next2 应答的 intent / 嵌套调用。
static mut FP_NLIID: u32 = 0;
static mut FP_NDONE: u8 = 0;
/// 0 闲，1 有未处理嵌套，2 已编码待发续 next2。
static mut FP_REV: u8 = 0;
static mut FP_RVN: u32 = 0; // 已处理的 reverse 次数
static mut FP_RVH: u32 = 0xffff_ffff; // 上次 open 返回的 handle
static mut FP_RVSC: u32 = 0; // 上次嵌套 sc
static mut FP_RVRES: u32 = 0xffff_ffff;
static mut FP_RVOUTN: u32 = 0;
static mut FP_RVOUT: [u8; 1024] = [0; 1024];
static mut FP_RVINN: u32 = 0;
static mut FP_RVIN: [u8; 256] = [0; 256];
static mut FP_RVNAME: [u8; 32] = [0; 32];
static mut FP_RVNAMEN: u8 = 0;
/// apps_std 最小虚拟文件：fd 3..6，空文件（size=0），供 stat/fopen/fread 走通。
static mut AST_FD_OPEN: [u8; 4] = [0; 4];
static mut AST_FD_POS: [u32; 4] = [0; 4];
static mut AST_FD_SIZE: [u32; 4] = [0; 4];
static mut AST_METHOD: u32 = 0xffff_ffff; // 上次 apps_std 方法号（含扩展）
/// remotectl_open 的应答：ctx、retval。0 还没有。
static mut FP_OPN: u8 = 0;
static mut FP_OPEN: [u32; 4] = [0; 4];
/// 输出缓冲里的 handle、dlret。
static mut FP_HDL: u32 = 0xffff_ffff;
static mut FP_DL: u32 = 0xffff_ffff;
/// 已经向 fastrpc 要过一次 intent。
static mut FP_REQ: u8 = 0;
static mut FP_SMR: u32 = 0xffff_ffff;
/// 选中槽改写前的 SMR、S2CR。没槽时是 nsmr、valid 位为空的槽数。
static mut FP_PREV: [u32; 2] = [0xffff_ffff; 2];
/// INIT_ATTACH_SENSORS 的参数页。链接基在 0xA7E80000，落在 USB CB 的 1GB 恒等映射里。
#[repr(C, align(4096))]
struct FpPage {
    b: [u8; 4096],
}
static mut FP_PAGE: FpPage = FpPage { b: [0; 4096] };

fn lpass_store(addr: u64, val: u32) {
    wr32(addr as usize, val);
    if addr < 0x80A0_0000 {
        dcache_civac(addr, 4);
    }
}

/// 在 NC 窗口里。commit 为假时只算出要作废的范围。
fn lpass_glink_in(commit: bool) -> u32 {
    const SMEM_BASE: u64 = 0x8090_0000;
    let toc = SMEM_BASE + 0x20_0000 - 0x1000;
    if rd32(toc as usize) != 0x434f_5424 {
        return 7;
    }
    let nent = rd32((toc + 8) as usize).min(48);
    let mut part = 0u32;
    let mut e = toc + 32;
    for _ in 0..nent {
        let off = rd32(e as usize);
        let sz = rd32((e + 4) as usize);
        let h0 = rd16a(e + 12) as u32;
        let h1 = rd16a(e + 14) as u32;
        e += 48;
        if off == 0 || sz < 32 || off >= 0x20_0000 || off + sz > 0x20_0000 {
            continue;
        }
        if (h0 == 0 && h1 == 2) || (h0 == 2 && h1 == 0) {
            part = off;
            break;
        }
    }
    if part == 0 {
        return 5;
    }
    let base = SMEM_BASE + part as u64;
    let Some(pm) = smem_rd32(base) else {
        return 6;
    };
    if pm != 0x5452_5024 {
        return 6;
    }
    let Some(psz) = smem_rd32(base + 8) else {
        return 6;
    };
    let Some(mut free_u) = smem_rd32(base + 12) else {
        return 6;
    };
    let Some(cached) = smem_rd32(base + 16) else {
        return 6;
    };
    let end_off = free_u.min(psz).min(0x20_0000 - part);
    let mut at478 = 0u64;
    let mut at479 = 0u64;
    let mut sz479 = 0u64;
    let mut at480 = 0u64;
    let mut sz480 = 0u64;
    if end_off >= 32 {
        let end = base + end_off as u64;
        let mut p = base + 32;
        for _ in 0..128 {
            if p + 16 > end || p + 16 > 0x80B0_0000 || (p < 0x80A0_0000 && p + 16 > 0x80A0_0000) {
                break;
            }
            let Some(can) = smem_rd32(p) else {
                return 8;
            };
            if (can as u16) != 0xa5a5 {
                break;
            }
            let item = (can >> 16) as u16;
            let Some(isz_w) = smem_rd32(p + 4) else {
                return 8;
            };
            let Some(pads) = smem_rd32(p + 8) else {
                return 8;
            };
            let isz = isz_w as u64;
            let pad = (pads >> 16) as u64;
            let data = p + 16 + pad;
            if item == 478 {
                at478 = data;
            } else if item == 479 {
                at479 = data;
                sz479 = isz - (pads as u16) as u64;
            } else if item == 480 {
                at480 = data;
                sz480 = isz - (pads as u16) as u64;
            }
            let next = p + 16 + pad + isz;
            if next <= p || isz > 0x2_0000 {
                return 8;
            }
            p = next;
        }
    }
    if at478 != 0 && at479 != 0 && at480 != 0 {
        unsafe {
            LG_D = at478;
            LG_TX = at479;
            LG_TXN = sz479;
            LG_RX = at480;
            LG_RXN = sz480;
        }
    }
    if unsafe { LG_PHASE } == 0 {
        let need478 = at478 == 0;
        let need479 = at479 == 0;
        let mut bytes = 0u32;
        if need478 {
            bytes = bytes.saturating_add(0x30);
        }
        if need479 {
            bytes = bytes.saturating_add(0x4010);
        }
        if bytes > 0 && free_u.saturating_add(bytes) > cached {
            return 2;
        }
        let hdr = base + free_u as u64;
        let item_end = hdr + bytes as u64;
        if bytes > 0 && (item_end > 0x80AFE000 || base >= 0x80AFE000) {
            return 9;
        }
        if bytes > 0 && hdr >= 0x80A0_0000 && unsafe { LG_FLUSH } != hdr {
            unsafe {
                LG_AT = hdr;
                LG_LEN = bytes as u64;
            }
            return 0x20;
        }
        if !commit && bytes > 0 && hdr >= 0x80A0_0000 {
            return 0x20;
        }
        if bytes > 0 {
            const MUTEX: usize = 0x01f4_3000;
            let mut got = false;
            for _ in 0..256 {
                wr32(MUTEX, 1);
                unsafe {
                    core::arch::asm!("dsb sy", options(nostack));
                }
                if rd32(MUTEX) == 1 {
                    got = true;
                    break;
                }
            }
            if !got {
                return 3;
            }
            let Some(now) = smem_rd32(base + 12) else {
                wr32(MUTEX, 0);
                return 6;
            };
            if now != free_u {
                wr32(MUTEX, 0);
                return 4;
            }
            if need478 {
                let h = base + free_u as u64;
                lpass_store(h, 0x01de_a5a5);
                lpass_store(h + 4, 0x20);
                lpass_store(h + 8, 0);
                lpass_store(h + 12, 0);
                at478 = h + 16;
                free_u += 0x30;
            }
            if need479 {
                let h = base + free_u as u64;
                lpass_store(h, 0x01df_a5a5);
                lpass_store(h + 4, 0x4000);
                lpass_store(h + 8, 0);
                lpass_store(h + 12, 0);
                at479 = h + 16;
                sz479 = 0x4000;
                free_u += 0x4010;
            }
            lpass_store(base + 12, free_u);
            wr32(MUTEX, 0);
        }
        if at478 == 0 || at479 == 0 || sz479 < 8 {
            return 2;
        }
        lpass_store(at478, 0);
        lpass_store(at478 + 8, 0);
        lpass_store(at478 + 12, 0);
        lpass_store(at479, 0x0001_0000);
        lpass_store(at479 + 4, 1);
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        lpass_store(at478 + 4, 8);
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        wr32(0x4080_0c, 3 << 16);
        unsafe { LG_PHASE = 1 };
        return 1;
    }
    if at478 == 0 || at479 == 0 || at480 == 0 || sz479 < 16 || sz480 < 8 {
        return 1;
    }
    // 接收 FIFO 到了就交给 lpass_glink_work。qcom_glink_native.c 是一直收的，
    // 不是只处理第一拍。create_remote 要等对方先 OPEN。
    unsafe { LG_PHASE = 2 };
    0x12
}

fn lpass_ch_name(i: u32) -> &'static [u8] {
    match i {
        0 => b"IPCRTR\0",
        1 => b"glink_ssr\0",
        2 => b"apr_audio_svc\0",
        _ => b"fastrpcglink-apps-dsp\0",
    }
}

fn lpass_name_is(rxf: u64, off: u64, nlen: u32, name: &[u8]) -> bool {
    if nlen as usize != name.len() {
        return false;
    }
    let mut i = 0u32;
    while i < nlen {
        let Some(w) = smem_rd32(rxf + off + 8 + (i & !3) as u64) else {
            return false;
        };
        let b = (w >> ((i & 3) * 8)) as u8;
        if b != name[i as usize] {
            return false;
        }
        i += 1;
    }
    true
}

fn lpass_ch(rxf: u64, off: u64, nlen: u32) -> Option<u32> {
    let mut i = 0u32;
    while i < 4 {
        if lpass_name_is(rxf, off, nlen, lpass_ch_name(i)) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn lpass_intent_n(ch: u32) -> u8 {
    match ch {
        0 => 10,
        1 => 5,
        2 => 20,
        _ => 64,
    }
}

fn lpass_intent_sz(ch: u32, n: u8) -> u32 {
    match ch {
        0 => {
            if n < 5 {
                0x800
            } else if n < 8 {
                0x2000
            } else {
                0x4400
            }
        }
        1 => 0x400,
        2 => 0x200,
        _ => 0x64,
    }
}

fn lpass_tx_idle(d: u64, txn: u64, need: u64) -> Option<u64> {
    let head = smem_rd32(d + 4).unwrap_or(0) as u64;
    let tail = smem_rd32(d).unwrap_or(0) as u64;
    if head != tail || head.saturating_add(need) > txn {
        return None;
    }
    Some(head)
}

fn lpass_tx_commit(d: u64, txf: u64, head: u64, n: u64) {
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    lpass_store(d + 4, (head + n) as u32);
    if txf < 0x80A0_0000 {
        dcache_civac(txf + head, n);
    }
    wr32(0x4080_0c, 3 << 16);
}

fn lpass_rx_advance(d: u64, step: u64, rxn: u64) {
    let tail = smem_rd32(d + 8).unwrap_or(0) as u64;
    let mut n = tail.saturating_add(step);
    if rxn != 0 && n >= rxn {
        n %= rxn;
    }
    lpass_store(d + 8, n as u32);
}

/// SID 0x1403 指到 USB 已经装好的 CB。
/// `arm_smmu_find_sme`：空槽只看 SMR.VALID（bit31）是 0，不看 S2CR。
/// 上一张还要求 S2CR 为 0，槽 4 以外的空槽全被跳过。
/// `arm_smmu_write_sme`：先 S2CR 再 SMR。不碰槽 4。
fn fastrpc_smmu() -> bool {
    if unsafe { FP_SMR } != 0xffff_ffff {
        return unsafe { FP_SMR } < 128;
    }
    let cb = unsafe { USB_CB };
    if cb == 0xffff_ffff || cb > 0xff {
        return false;
    }
    let base = SMMU_BASE;
    let nsmr = (rd32(base + 0x20) & 0xff) as usize;
    if nsmr == 0 || nsmr > 128 {
        unsafe {
            FP_PREV[0] = nsmr as u32;
            FP_PREV[1] = 0;
            FP_SMR = 0xffff_fffe;
        }
        return false;
    }
    let sid = 0x1403u32;
    let mut free: Option<usize> = None;
    let mut nfree = 0u32;
    let mut i = 0usize;
    while i < nsmr {
        let smr = rd32(base + 0x800 + i * 4);
        if smr & (1 << 31) == 0 {
            nfree += 1;
            if free.is_none() && i != 4 {
                free = Some(i);
            }
            i += 1;
            continue;
        }
        let id = smr & 0x7fff;
        let mask = (smr >> 16) & 0x7fff;
        // 已有条目盖住这条 SID 时，内核返回 -EINVAL，不再占新槽。
        if ((sid ^ id) & !mask) == 0 {
            let s2 = rd32(base + 0xc00 + i * 4);
            unsafe {
                FP_PREV[0] = smr;
                FP_PREV[1] = s2;
            }
            if mask == 0 && id == sid && (s2 >> 16) & 3 == 0 && (s2 & 0xff) == cb {
                unsafe { FP_SMR = i as u32 };
                return true;
            }
            unsafe { FP_SMR = 0xffff_fffc };
            return false;
        }
        i += 1;
    }
    let Some(slot) = free else {
        unsafe {
            FP_PREV[0] = nsmr as u32;
            FP_PREV[1] = nfree;
            FP_SMR = 0xffff_fffe;
        }
        return false;
    };
    let smr_before = rd32(base + 0x800 + slot * 4);
    let s2_before = rd32(base + 0xc00 + slot * 4);
    unsafe {
        FP_PREV[0] = smr_before;
        FP_PREV[1] = s2_before;
    }
    // TYPE=translation，CBNDX=USB 的 CB，SHCFG=NSH（arm_smmu_write_s2cr）。
    let s2_val = cb | (3 << 8);
    wr32(base + 0xc00 + slot * 4, s2_val);
    wr32(base + 0x800 + slot * 4, (1 << 31) | sid);
    let sb = rd32(base + 0xc00 + slot * 4);
    let sm = rd32(base + 0x800 + slot * 4);
    if (sb >> 16) & 3 == 0
        && (sb & 0xff) == cb
        && sm & (1 << 31) != 0
        && (sm & 0x7fff) == sid
    {
        unsafe { FP_SMR = slot as u32 };
        return true;
    }
    unsafe { FP_SMR = 0xffff_fffd };
    false
}

/// hexagonrpc `remotectl_open("adsp_default_listener")`。
/// handle 0，method 0，in 2 / out 2。attach 的 retval 是 0、并且对方还了 intent 之后才发。
fn fastrpc_remotectl(d: u64, txf: u64, txn: u64, at: u64) -> bool {
    if unsafe { LG_FPN } == 0 {
        return false;
    }
    let reply_ctx = unsafe { LG_FP[0] };
    let reply_ret = unsafe { LG_FP[2] };
    if reply_ctx != 0x1002 || reply_ret != 0 {
        unsafe { FP_SENT = 6 };
        return false;
    }
    // __qcom_glink_rx_done：先把对方应答占用的 intent 还回去。一拍只发这一包。
    if unsafe { FP_RDONE } == 0 {
        let liid = unsafe { FP_RLIID };
        if liid == 0 {
            unsafe { FP_RDONE = 1 };
            return false;
        }
        if at + 8 > txn {
            return false;
        }
        lpass_store(txf + at, 14 | (4 << 16));
        lpass_store(txf + at + 4, liid);
        lpass_tx_commit(d, txf, at, 8);
        unsafe { FP_RDONE = 1 };
        return true;
    }
    let reuse = unsafe { FP_REUSE };
    if reuse == 0 {
        return false;
    }
    let rcid = unsafe { FP_RCID } as usize;
    if rcid >= 8 {
        return false;
    }
    if reuse == 2 && unsafe { FP_INEW } != 1 {
        if unsafe { FP_INEW } == 0 && at + 8 <= txn {
            lpass_store(txf + at, 7 | (4 << 16));
            lpass_store(txf + at + 4, 64);
            lpass_tx_commit(d, txf, at, 8);
            unsafe { FP_INEW = 2 };
            return true;
        }
        return false;
    }
    let iid = unsafe { FP_ILOG[rcid * 2] };
    let isz = unsafe { FP_ILOG[rcid * 2 + 1] };
    if isz < 40 {
        return false;
    }
    let page = core::ptr::addr_of!(FP_PAGE) as u64;
    if page < 0x8000_0000 || page + 4096 > 0xC000_0000 {
        unsafe { FP_SENT = 3 };
        return false;
    }
    let total = 16 + 40;
    if at + total as u64 > txn {
        return false;
    }
    let tagged = (3u64 << 32) | page;
    let mut i = 0u64;
    while i < 4096 {
        wr32((page + i) as usize, 0);
        i += 4;
    }
    // get_args：4 个 remote_arg64，list 从 64 起，phy page 从 96 起。
    // metalen 548，每个缓冲按 128 对齐：640、768、896、1024。
    let offs = [640u64, 768, 896, 1024];
    let lens = [8u64, 22, 8, 256];
    let mut n = 0usize;
    while n < 4 {
        let pv = tagged + offs[n];
        wr32((page + n as u64 * 16) as usize, pv as u32);
        wr32((page + n as u64 * 16 + 4) as usize, (pv >> 32) as u32);
        wr32((page + n as u64 * 16 + 8) as usize, lens[n] as u32);
        wr32((page + 64 + n as u64 * 8) as usize, 1);
        wr32((page + 64 + n as u64 * 8 + 4) as usize, n as u32);
        wr32((page + 96 + n as u64 * 16) as usize, tagged as u32);
        wr32((page + 96 + n as u64 * 16 + 4) as usize, (tagged >> 32) as u32);
        wr32((page + 96 + n as u64 * 16 + 8) as usize, 4096);
        n += 1;
    }
    wr32((page + 640) as usize, 22);
    wr32((page + 644) as usize, 256);
    let name = b"adsp_default_listener\0";
    let mut o = 0usize;
    while o < name.len() {
        let mut b = [0u8; 4];
        let mut k = 0usize;
        while k < 4 && o + k < name.len() {
            b[k] = name[o + k];
            k += 1;
        }
        wr32((page + 768 + o as u64) as usize, u32::from_le_bytes(b));
        o += 4;
    }
    dcache_civac(page, 4096);
    // 用户态 invoke：pid 是 attach 时的 tgid。ctx 是下一号 (2<<12)|pd2。
    let sc = 0x0002_0200u32;
    let ctx = 0x2002u32;
    let msg = [
        1u32,
        1,
        ctx,
        0,
        0,
        sc,
        tagged as u32,
        (tagged >> 32) as u32,
        4096,
        0,
    ];
    lpass_store(txf + at, 9 | (4 << 16));
    lpass_store(txf + at + 4, iid);
    lpass_store(txf + at + 8, 40);
    lpass_store(txf + at + 12, 0);
    let mut w = 0usize;
    while w < 10 {
        lpass_store(txf + at + 16 + w as u64 * 4, msg[w]);
        w += 1;
    }
    lpass_tx_commit(d, txf, at, total as u64);
    unsafe {
        FP_SENT = 4;
        FP_LAST = iid;
        // 这个 intent 又被用掉了，等下一次 RX_DONE。
        FP_REUSE = 0;
    }
    true
}

/// `adsp_default_listener_register`：method 0，没有参数。handle 是 open 返回的。
fn fastrpc_register(d: u64, txf: u64, txn: u64, at: u64) -> bool {
    if unsafe { FP_OPN } == 0 {
        return false;
    }
    if unsafe { FP_OPEN[0] } != 0x2002 || unsafe { FP_OPEN[2] } != 0 {
        unsafe { FP_SENT = 7 };
        return false;
    }
    let page = core::ptr::addr_of!(FP_PAGE) as u64;
    dcache_ivac(page + 896, 64);
    let hdl = rd32((page + 896) as usize);
    let dl = rd32((page + 900) as usize);
    unsafe {
        FP_HDL = hdl;
        FP_DL = dl;
    }
    if dl != 0 {
        unsafe { FP_SENT = 7 };
        return false;
    }
    if unsafe { FP_ODONE } == 0 {
        let liid = unsafe { FP_OLIID };
        if liid == 0 {
            unsafe { FP_ODONE = 1 };
            return false;
        }
        if at + 8 > txn {
            return false;
        }
        lpass_store(txf + at, 14 | (4 << 16));
        lpass_store(txf + at + 4, liid);
        lpass_tx_commit(d, txf, at, 8);
        unsafe { FP_ODONE = 1 };
        return true;
    }
    if unsafe { FP_REUSE } == 0 {
        return false;
    }
    let rcid = unsafe { FP_RCID } as usize;
    if rcid >= 8 {
        return false;
    }
    let iid = unsafe { FP_ILOG[rcid * 2] };
    let isz = unsafe { FP_ILOG[rcid * 2 + 1] };
    if isz < 40 || at + 56 > txn {
        return false;
    }
    if page < 0x8000_0000 || page + 4096 > 0xC000_0000 {
        return false;
    }
    let tagged = (3u64 << 32) | page;
    let mut i = 0u64;
    while i < 4096 {
        wr32((page + i) as usize, 0);
        i += 4;
    }
    dcache_civac(page, 4096);
    // sc=0：没有输入也没有输出。ctx 是第三号 (3<<12)|pd2。
    let ctx = 0x3002u32;
    let msg = [
        1u32,
        1,
        ctx,
        0,
        hdl,
        0,
        tagged as u32,
        (tagged >> 32) as u32,
        4096,
        0,
    ];
    lpass_store(txf + at, 9 | (4 << 16));
    lpass_store(txf + at + 4, iid);
    lpass_store(txf + at + 8, 40);
    lpass_store(txf + at + 12, 0);
    let mut w = 0usize;
    while w < 10 {
        lpass_store(txf + at + 16 + w as u64 * 4, msg[w]);
        w += 1;
    }
    lpass_tx_commit(d, txf, at, 56);
    unsafe {
        FP_SENT = 5;
        FP_LAST = iid;
        FP_REUSE = 0;
    }
    true
}

/// `remotectl_close`：handle 0，method 1。输入是 open 拿到的对象句柄。
fn fastrpc_close(d: u64, txf: u64, txn: u64, at: u64) -> bool {
    if unsafe { FP_REGN } == 0 {
        return false;
    }
    if unsafe { FP_REG[0] } != 0x3002 || unsafe { FP_REG[2] } != 0 {
        unsafe { FP_SENT = 11 };
        return false;
    }
    if unsafe { FP_GDONE } == 0 {
        let liid = unsafe { FP_GLIID };
        if liid == 0 {
            unsafe { FP_GDONE = 1 };
            return false;
        }
        if at + 8 > txn {
            return false;
        }
        lpass_store(txf + at, 14 | (4 << 16));
        lpass_store(txf + at + 4, liid);
        lpass_tx_commit(d, txf, at, 8);
        unsafe { FP_GDONE = 1 };
        return true;
    }
    if unsafe { FP_REUSE } == 0 {
        return false;
    }
    let rcid = unsafe { FP_RCID } as usize;
    if rcid >= 8 {
        return false;
    }
    let iid = unsafe { FP_ILOG[rcid * 2] };
    let isz = unsafe { FP_ILOG[rcid * 2 + 1] };
    if isz < 40 || at + 56 > txn {
        return false;
    }
    let page = core::ptr::addr_of!(FP_PAGE) as u64;
    if page < 0x8000_0000 || page + 4096 > 0xC000_0000 {
        return false;
    }
    let hdl = unsafe { FP_HDL };
    let tagged = (3u64 << 32) | page;
    let mut i = 0u64;
    while i < 4096 {
        wr32((page + i) as usize, 0);
        i += 4;
    }
    // 3 个 remote_arg64。list 从 48 起，phy page 从 72 起。
    // metalen 508，128 对齐后是 512、640、768。
    let offs = [512u64, 640, 768];
    let lens = [8u64, 4, 256];
    let mut n = 0usize;
    while n < 3 {
        let pv = tagged + offs[n];
        wr32((page + n as u64 * 16) as usize, pv as u32);
        wr32((page + n as u64 * 16 + 4) as usize, (pv >> 32) as u32);
        wr32((page + n as u64 * 16 + 8) as usize, lens[n] as u32);
        wr32((page + 48 + n as u64 * 8) as usize, 1);
        wr32((page + 48 + n as u64 * 8 + 4) as usize, n as u32);
        wr32((page + 72 + n as u64 * 16) as usize, tagged as u32);
        wr32((page + 72 + n as u64 * 16 + 4) as usize, (tagged >> 32) as u32);
        wr32((page + 72 + n as u64 * 16 + 8) as usize, 4096);
        n += 1;
    }
    wr32((page + 512) as usize, hdl);
    wr32((page + 516) as usize, 256);
    dcache_civac(page, 4096);
    // method 1，1 个输入 2 个输出。ctx 是第四号。
    let sc = 0x0101_0200u32;
    let ctx = 0x4002u32;
    let msg = [
        1u32,
        1,
        ctx,
        0,
        0,
        sc,
        tagged as u32,
        (tagged >> 32) as u32,
        4096,
        0,
    ];
    lpass_store(txf + at, 9 | (4 << 16));
    lpass_store(txf + at + 4, iid);
    lpass_store(txf + at + 8, 40);
    lpass_store(txf + at + 12, 0);
    let mut w = 0usize;
    while w < 10 {
        lpass_store(txf + at + 16 + w as u64 * 4, msg[w]);
        w += 1;
    }
    lpass_tx_commit(d, txf, at, 56);
    unsafe {
        FP_SENT = 8;
        FP_LAST = iid;
        FP_REUSE = 0;
    }
    true
}

/// `adsp_listener_init2`：handle 3，method 3，没有参数。
/// `remotectl_close` 占住 intent 之后没有应答，这里不再发 close。
fn fastrpc_init2(d: u64, txf: u64, txn: u64, at: u64) -> bool {
    if unsafe { FP_REGN } == 0 || unsafe { FP_REG[0] } != 0x3002 || unsafe { FP_REG[2] } != 0 {
        return false;
    }
    // 单变量刀：跳过 register 应答的 RX_DONE_W_REUSE。
    // 证据：tail 停在 0x3d0=RX_DONE 之后；intent/init2/READ_NOTIF 放在
    // 同一偏移均不消费 → 疑 RX_DONE(liid=FP_GLIID) 楔死 DSP 读端。
    if unsafe { FP_GDONE } == 0 {
        unsafe { FP_GDONE = 2 }; // 2 = 故意未发
    }
    // 等 register 那包的 RX_DONE（对方还我们的 TX intent）。
    if unsafe { FP_REUSE } == 0 {
        return false;
    }
    // FP_IXTRA / READ_NOTIF 均已证伪（卡在 RX_DONE 之后的偏移）。
    let page = core::ptr::addr_of!(FP_PAGE) as u64;
    let rcid = unsafe { FP_RCID } as usize;
    if rcid >= 8 {
        return false;
    }
    let iid = unsafe { FP_ILOG[rcid * 2] };
    let isz = unsafe { FP_ILOG[rcid * 2 + 1] };
    if isz < 40 || at + 56 > txn {
        return false;
    }
    if page < 0x8000_0000 || page + 4096 > 0xC000_0000 {
        return false;
    }
    let tagged = (3u64 << 32) | page;
    let mut i = 0u64;
    while i < 4096 {
        wr32((page + i) as usize, 0);
        i += 4;
    }
    dcache_civac(page, 4096);
    let ctx = 0x5002u32;
    let msg = [
        1u32,
        1,
        ctx,
        0,
        3u32,
        0x0300_0000,
        tagged as u32,
        (tagged >> 32) as u32,
        4096,
        0,
    ];
    lpass_store(txf + at, 9 | (4 << 16));
    lpass_store(txf + at + 4, iid);
    lpass_store(txf + at + 8, 40);
    lpass_store(txf + at + 12, 0);
    let mut w = 0usize;
    while w < 10 {
        lpass_store(txf + at + 16 + w as u64 * 4, msg[w]);
        w += 1;
    }
    lpass_tx_commit(d, txf, at, 56);
    unsafe {
        FP_SENT = 9;
        FP_LAST = iid;
        FP_REUSE = 0;
        FP_INEW = 0;
        FP_SH = smem_rd32(d + 4).unwrap_or(0xffff_ffff);
        FP_ST = smem_rd32(d).unwrap_or(0xffff_ffff);
    }
    true
}

/// 第一次 `adsp_listener_next2`。handle 3，method 4。
/// 上一轮结果是 rctx 0、result 0xffffffff，输出缓冲是空的。
fn fastrpc_next2(d: u64, txf: u64, txn: u64, at: u64) -> bool {
    if unsafe { FP_ININ } == 0 {
        return false;
    }
    if unsafe { FP_INI[0] } != 0x5002 || unsafe { FP_INI[2] } != 0 {
        unsafe { FP_SENT = 13 };
        return false;
    }
    if unsafe { FP_IDONE } == 0 {
        let liid = unsafe { FP_ILIID };
        if liid == 0 {
            unsafe { FP_IDONE = 1 };
            return false;
        }
        if at + 8 > txn {
            return false;
        }
        lpass_store(txf + at, 14 | (4 << 16));
        lpass_store(txf + at + 4, liid);
        lpass_tx_commit(d, txf, at, 8);
        unsafe { FP_IDONE = 1 };
        return true;
    }
    if unsafe { FP_REUSE } == 0 && unsafe { FP_INEW } != 1 {
        if unsafe { FP_INEW } == 0 && at + 8 <= txn {
            lpass_store(txf + at, 7 | (4 << 16));
            lpass_store(txf + at + 4, 64);
            lpass_tx_commit(d, txf, at, 8);
            unsafe { FP_INEW = 2 };
            return true;
        }
        return false;
    }
    let rcid = unsafe { FP_RCID } as usize;
    if rcid >= 8 {
        return false;
    }
    let iid = unsafe { FP_ILOG[rcid * 2] };
    let isz = unsafe { FP_ILOG[rcid * 2 + 1] };
    // 参数页比 40 字节大，intent 至少要盖住 smq_msg。
    if isz < 40 || at + 56 > txn {
        return false;
    }
    let page = core::ptr::addr_of!(FP_PAGE) as u64;
    if page < 0x8000_0000 || page + 4096 > 0xC000_0000 {
        return false;
    }
    let tagged = (3u64 << 32) | page;
    let mut i = 0u64;
    while i < 4096 {
        wr32((page + i) as usize, 0);
        i += 4;
    }
    // hexagonrpc / 线表：REMOTE_SCALARS_MAKE(4, 2, 2) → 2 in + 2 out。
    // dump: argument 00000000FFFFFFFF0000000000000000 + 空 inbuf；
    // prim out 16B + nested_inbufs 256B。旧 sc=0x04020300（out=3）会让
    // DSP 多走一个 remote_arg → frpck_1_1。
    // pra[0] prim in 16B @640；pra[1] 空 in；pra[2] prim out @768；
    // pra[3] nested inbufs @896。
    let offs = [640u64, 0, 768, 896];
    let lens = [16u64, 0, 16, 256];
    let mut n = 0usize;
    while n < 4 {
        let off = offs[n];
        let len = lens[n];
        if len != 0 {
            let pv = tagged + off;
            wr32((page + n as u64 * 16) as usize, pv as u32);
            wr32((page + n as u64 * 16 + 4) as usize, (pv >> 32) as u32);
        }
        wr32((page + n as u64 * 16 + 8) as usize, len as u32);
        wr32((page + 64 + n as u64 * 8) as usize, if len != 0 { 1 } else { 0 });
        wr32((page + 64 + n as u64 * 8 + 4) as usize, n as u32);
        wr32((page + 96 + n as u64 * 16) as usize, tagged as u32);
        wr32((page + 96 + n as u64 * 16 + 4) as usize, (tagged >> 32) as u32);
        wr32((page + 96 + n as u64 * 16 + 8) as usize, 4096);
        n += 1;
    }
    // prev_ctx=0, prev_result=0xffffffff, outbufs_len=0, inbufs_size=0
    wr32((page + 640) as usize, 0);
    wr32((page + 644) as usize, 0xffff_ffff);
    wr32((page + 648) as usize, 0);
    wr32((page + 652) as usize, 0);
    dcache_civac(page, 4096);
    let ctx = 0x6002u32;
    let msg = [
        1u32,
        1,
        ctx,
        0,
        3u32,
        0x0402_0200, // MAKE(4, 2, 2)
        tagged as u32,
        (tagged >> 32) as u32,
        4096,
        0,
    ];
    lpass_store(txf + at, 9 | (4 << 16));
    lpass_store(txf + at + 4, iid);
    lpass_store(txf + at + 8, 40);
    lpass_store(txf + at + 12, 0);
    let mut w = 0usize;
    while w < 10 {
        lpass_store(txf + at + 16 + w as u64 * 4, msg[w]);
        w += 1;
    }
    lpass_tx_commit(d, txf, at, 56);
    unsafe {
        FP_SENT = 10;
        FP_LAST = iid;
        FP_REUSE = 0;
        FP_INEW = 0;
    }
    true
}

/// 解开 next2 嵌套 inbufs 线格式（与 hexagonrpcd/iobuffer.c 同形）。
fn fp_rev_take_buf(src: &[u8], mut off: usize, align: &mut usize) -> Option<(usize, usize)> {
    if off + 4 > src.len() {
        return None;
    }
    let sz = u32::from_le_bytes([src[off], src[off + 1], src[off + 2], src[off + 3]]) as usize;
    off += 4;
    *align = (*align + 4) & 7;
    if *align != 0 {
        let pad = 8 - *align;
        if off + pad > src.len() {
            return None;
        }
        off += pad;
        *align = 0;
    }
    if off + sz > src.len() {
        return None;
    }
    let start = off;
    off += sz;
    *align = (*align + sz) & 7;
    Some((start, sz))
}

/// handle0 / method0 = remotectl_open：按名返回 apps_std=1 / apps_mem=2。
fn fp_rev_remotectl_open() -> u32 {
    let n = unsafe { FP_RVINN } as usize;
    let src = unsafe { &FP_RVIN[..n] };
    let mut align = 0usize;
    let mut off = 0usize;
    let Some((o0, l0)) = fp_rev_take_buf(src, off, &mut align) else {
        unsafe { FP_RVRES = 14 }; // AEE_EBADPARM
        return 14;
    };
    off = o0 + l0;
    if l0 < 8 {
        unsafe { FP_RVRES = 14 };
        return 14;
    }
    let inlen = u32::from_le_bytes([src[o0], src[o0 + 1], src[o0 + 2], src[o0 + 3]]);
    let outlen = u32::from_le_bytes([src[o0 + 4], src[o0 + 5], src[o0 + 6], src[o0 + 7]]);
    let Some((o1, l1)) = fp_rev_take_buf(src, off, &mut align) else {
        unsafe { FP_RVRES = 14 };
        return 14;
    };
    if l1 == 0 || l1 > 31 || (inlen as usize != 0 && inlen as usize != l1) {
        // inlen 偶发与实际长度一致；放宽
    }
    if src[o1 + l1 - 1] != 0 {
        unsafe { FP_RVRES = 14 };
        return 14;
    }
    let namelen = l1.min(31);
    unsafe {
        FP_RVNAMEN = namelen as u8;
        let mut i = 0usize;
        while i < namelen {
            FP_RVNAME[i] = src[o1 + i];
            i += 1;
        }
        while i < 32 {
            FP_RVNAME[i] = 0;
            i += 1;
        }
    }
    let name = &src[o1..o1 + l1];
    let hdl = if name.starts_with(b"apps_std") {
        1u32
    } else if name.starts_with(b"apps_mem") {
        2u32
    } else if name.starts_with(b"remotectl") {
        0u32
    } else {
        // 未知接口：Android 同形回 -5
        let err = (-5i32) as u32;
        let ol = outlen.min(256);
        // encode outbufs: {0, err} + zero errstr
        let mut enc = [0u8; 320];
        let mut eo = 0usize;
        // out0 size 8
        enc[eo..eo + 4].copy_from_slice(&8u32.to_le_bytes());
        eo += 4;
        eo += 4; // pad
        enc[eo..eo + 4].copy_from_slice(&0u32.to_le_bytes());
        enc[eo + 4..eo + 8].copy_from_slice(&err.to_le_bytes());
        eo += 8;
        // out1
        enc[eo..eo + 4].copy_from_slice(&ol.to_le_bytes());
        eo += 4;
        eo += 4; // pad
        eo += ol as usize;
        unsafe {
            FP_RVOUTN = eo as u32;
            FP_RVOUT[..eo].copy_from_slice(&enc[..eo]);
            FP_RVH = 0;
            FP_RVRES = err;
        }
        return err;
    };
    let ol = outlen.min(256);
    let mut enc = [0u8; 320];
    let mut eo = 0usize;
    enc[eo..eo + 4].copy_from_slice(&8u32.to_le_bytes());
    eo += 4;
    eo += 4;
    enc[eo..eo + 4].copy_from_slice(&hdl.to_le_bytes());
    enc[eo + 4..eo + 8].copy_from_slice(&0u32.to_le_bytes());
    eo += 8;
    enc[eo..eo + 4].copy_from_slice(&ol.to_le_bytes());
    eo += 4;
    eo += 4;
    // zeros already
    eo += ol as usize;
    unsafe {
        FP_RVOUTN = eo as u32;
        FP_RVOUT[..eo].copy_from_slice(&enc[..eo]);
        FP_RVH = hdl;
        FP_RVRES = 0;
        FP_RVN = FP_RVN.wrapping_add(1);
    }
    0
}

/// 处理嵌套 reverse invoke；成功则 FP_REV=2（待发续 next2）。
fn fp_rev_encode_empty_outs(sc: u32, fill4: bool) {
    let n_out = ((sc >> 8) & 0xff) as usize;
    let mut enc = [0u8; 1024];
    let mut eo = 0usize;
    let mut i = 0usize;
    while i < n_out && eo + 16 < enc.len() {
        let sz: u32 = if fill4 { 4 } else { 0 };
        enc[eo..eo + 4].copy_from_slice(&sz.to_le_bytes());
        eo += 4;
        eo += 4; // pad to 8
        if fill4 {
            eo += 4;
        }
        i += 1;
    }
    unsafe {
        FP_RVOUTN = eo as u32;
        FP_RVOUT[..eo].copy_from_slice(&enc[..eo]);
    }
}

/// 编码若干 outbufs（与 hexagonrpcd/iobuffer.c outbufs_encode 同形）。
fn fp_rev_encode_outs(bufs: &[&[u8]]) {
    let mut enc = [0u8; 1024];
    let mut eo = 0usize;
    let mut align = 0usize;
    let mut bi = 0usize;
    while bi < bufs.len() {
        let b = bufs[bi];
        if eo + 4 > enc.len() {
            break;
        }
        enc[eo..eo + 4].copy_from_slice(&(b.len() as u32).to_le_bytes());
        eo += 4;
        align = (align + 4) & 7;
        if !b.is_empty() {
            if align != 0 {
                let pad = 8 - align;
                if eo + pad > enc.len() {
                    break;
                }
                eo += pad;
                align = 0;
            }
            if eo + b.len() > enc.len() {
                break;
            }
            enc[eo..eo + b.len()].copy_from_slice(b);
            eo += b.len();
            align = (align + b.len()) & 7;
        }
        bi += 1;
    }
    unsafe {
        FP_RVOUTN = eo as u32;
        FP_RVOUT[..eo].copy_from_slice(&enc[..eo]);
    }
}

fn fp_rev_set_path_name(src: &[u8]) {
    let mut n = 0usize;
    while n < src.len() && n < 31 && src[n] != 0 {
        n += 1;
    }
    unsafe {
        FP_RVNAMEN = n as u8;
        let mut i = 0usize;
        while i < n {
            FP_RVNAME[i] = src[i];
            i += 1;
        }
        while i < 32 {
            FP_RVNAME[i] = 0;
            i += 1;
        }
    }
}

fn fp_rev_parse_inbufs(src: &[u8], outs: &mut [(usize, usize); 8]) -> usize {
    let mut align = 0usize;
    let mut off = 0usize;
    let mut n = 0usize;
    while n < 8 {
        let Some((start, sz)) = fp_rev_take_buf(src, off, &mut align) else {
            break;
        };
        outs[n] = (start, sz);
        off = start + sz;
        n += 1;
    }
    n
}

fn fp_ast_alloc_fd(size: u32) -> Option<u32> {
    let mut i = 0usize;
    while i < 4 {
        if unsafe { AST_FD_OPEN[i] } == 0 {
            unsafe {
                AST_FD_OPEN[i] = 1;
                AST_FD_POS[i] = 0;
                AST_FD_SIZE[i] = size;
            }
            return Some((i as u32) + 3); // fd 3..6
        }
        i += 1;
    }
    None
}

fn fp_ast_fd_idx(fd: u32) -> Option<usize> {
    if fd >= 3 && fd <= 6 {
        let i = (fd - 3) as usize;
        if unsafe { AST_FD_OPEN[i] } != 0 {
            return Some(i);
        }
    }
    None
}

/// apps_std_stat 应答：96 字节 packed（对账 hexagonrpcd/apps_std.c）。
fn fp_ast_stat_bytes(size: u64, mode: u32) -> [u8; 96] {
    let mut o = [0u8; 96];
    // tsz=0, dev=1, ino=1
    o[8..16].copy_from_slice(&1u64.to_le_bytes());
    o[16..24].copy_from_slice(&1u64.to_le_bytes());
    o[24..28].copy_from_slice(&mode.to_le_bytes());
    o[28..32].copy_from_slice(&1u32.to_le_bytes()); // nlink
    // rdev=0
    o[40..48].copy_from_slice(&size.to_le_bytes());
    o
}

fn fp_rev_apps_std(method: u32, sc: u32) -> u32 {
    unsafe { AST_METHOD = method };
    let n = unsafe { FP_RVINN } as usize;
    let src = unsafe { &FP_RVIN[..n] };
    let mut bufs = [(0usize, 0usize); 8];
    let nb = fp_rev_parse_inbufs(src, &mut bufs);

    // AEE_EFAILED=1, EBADPARM=14, EUNSUPPORTED=20, ERESOURCENOTFOUND=22
    match method {
        // fflush(2) / fclose(3) / fseek(9)：无 outbufs
        2 => {
            fp_rev_encode_outs(&[]);
            0
        }
        3 => {
            // fclose: prim in = fd
            if nb >= 1 && bufs[0].1 >= 4 {
                let (s, _) = bufs[0];
                let fd = u32::from_le_bytes([src[s], src[s + 1], src[s + 2], src[s + 3]]);
                if let Some(i) = fp_ast_fd_idx(fd) {
                    unsafe { AST_FD_OPEN[i] = 0 };
                }
            }
            fp_rev_encode_outs(&[]);
            0
        }
        9 => {
            // fseek: fd, pos, whence
            if nb >= 1 && bufs[0].1 >= 12 {
                let (s, _) = bufs[0];
                let fd = u32::from_le_bytes([src[s], src[s + 1], src[s + 2], src[s + 3]]);
                let pos = u32::from_le_bytes([src[s + 4], src[s + 5], src[s + 6], src[s + 7]]);
                let whence = u32::from_le_bytes([src[s + 8], src[s + 9], src[s + 10], src[s + 11]]);
                if let Some(i) = fp_ast_fd_idx(fd) {
                    let size = unsafe { AST_FD_SIZE[i] };
                    let cur = unsafe { AST_FD_POS[i] };
                    let np = match whence {
                        0 => pos, // SEEK_SET
                        1 => cur.wrapping_add(pos),
                        2 => size.wrapping_add(pos), // SEEK_END, pos usually 0
                        _ => cur,
                    };
                    unsafe { AST_FD_POS[i] = np };
                }
            }
            fp_rev_encode_outs(&[]);
            0
        }
        // fread(4): out0 = {written,u32 is_eof,u32}, out1 = data
        4 => {
            // fread：从虚拟 fd 吐零直到 size；prim 里 fd+buf_size（容错 4/8B）
            let mut want = 0u32;
            let mut fd = 0u32;
            if nb >= 1 && bufs[0].1 >= 4 {
                let (s, l) = bufs[0];
                fd = u32::from_le_bytes([src[s], src[s + 1], src[s + 2], src[s + 3]]);
                if l >= 8 {
                    want = u32::from_le_bytes([src[s + 4], src[s + 5], src[s + 6], src[s + 7]]);
                }
            }
            // OUT_BLOB_SEQ 长度：从 inbufs[0] 描述符末尾尽力读；否则用 want
            let mut seq = want;
            if nb >= 1 && bufs[0].1 >= 12 {
                let (s, l) = bufs[0];
                // 扩展/普通：跳过 method? fread msg_id=4 <31，无 method 字。
                // 布局约：fd, buf_size, out_seq_count…
                if l >= 12 {
                    seq = u32::from_le_bytes([src[s + 8], src[s + 9], src[s + 10], src[s + 11]]);
                }
            }
            if seq == 0 {
                seq = want;
            }
            if seq > 256 {
                seq = 256; // 编码缓冲上限
            }
            let mut written = 0u32;
            let mut is_eof = 1u32;
            if let Some(i) = fp_ast_fd_idx(fd) {
                let size = unsafe { AST_FD_SIZE[i] };
                let pos = unsafe { AST_FD_POS[i] };
                let rem = if pos < size { size - pos } else { 0 };
                written = if rem < seq { rem } else { seq };
                unsafe { AST_FD_POS[i] = pos.wrapping_add(written) };
                is_eof = if unsafe { AST_FD_POS[i] } >= size { 1 } else { 0 };
            }
            let meta = {
                let mut m = [0u8; 8];
                m[0..4].copy_from_slice(&written.to_le_bytes());
                m[4..8].copy_from_slice(&is_eof.to_le_bytes());
                m
            };
            // 零页作 data
            let z = [0u8; 256];
            let data = &z[..written as usize];
            let n_out = ((sc >> 8) & 0xff) as usize;
            if n_out >= 2 {
                fp_rev_encode_outs(&[&meta, data]);
            } else if n_out == 1 {
                fp_rev_encode_outs(&[&meta]);
            } else {
                fp_rev_encode_outs(&[]);
            }
            0
        }
        // fopen_with_env(19): inbufs[1]=env, [3]=path, [4]=mode；out=fd
        19 => {
            if nb < 5 {
                fp_rev_encode_empty_outs(sc, true);
                return 14;
            }
            let (e0, el) = bufs[1];
            let (p0, pl) = bufs[3];
            let (m0, ml) = bufs[4];
            if pl == 0 || src[p0 + pl - 1] != 0 || el == 0 || src[e0 + el - 1] != 0 {
                fp_rev_encode_empty_outs(sc, true);
                return 14;
            }
            if ml == 0 || src[m0 + ml - 1] != 0 {
                fp_rev_encode_empty_outs(sc, true);
                return 14;
            }
            let mode0 = src[m0];
            if mode0 == b'w' || mode0 == b'a' {
                fp_rev_encode_empty_outs(sc, true);
                return 20; // EUNSUPPORTED
            }
            fp_rev_set_path_name(&src[p0..p0 + pl]);
            // 最小：任意只读 open → 64B 零文件 fd（与 stat size 对齐）
            match fp_ast_alloc_fd(64) {
                Some(fd) => {
                    let fb = fd.to_le_bytes();
                    fp_rev_encode_outs(&[&fb]);
                    0
                }
                None => {
                    fp_rev_encode_empty_outs(sc, true);
                    2 // ENOMEMORY
                }
            }
        }
        // opendir(26): out = u64 dirfd；空目录给假 fd
        26 => {
            if nb >= 2 {
                let (p0, pl) = bufs[1];
                if pl > 0 {
                    fp_rev_set_path_name(&src[p0..p0 + pl.min(31)]);
                }
            }
            match fp_ast_alloc_fd(0) {
                Some(fd) => {
                    let mut d = [0u8; 8];
                    d[0..4].copy_from_slice(&fd.to_le_bytes());
                    fp_rev_encode_outs(&[&d]);
                    0
                }
                None => {
                    fp_rev_encode_empty_outs(sc, true);
                    2
                }
            }
        }
        27 => {
            // closedir
            if nb >= 1 && bufs[0].1 >= 4 {
                let (s, _) = bufs[0];
                let fd = u32::from_le_bytes([src[s], src[s + 1], src[s + 2], src[s + 3]]);
                if let Some(i) = fp_ast_fd_idx(fd) {
                    unsafe { AST_FD_OPEN[i] = 0 };
                }
            }
            fp_rev_encode_outs(&[]);
            0
        }
        // readdir(28): 立刻 EOF（nout first u32 = 0）
        28 => {
            let zero4 = [0u8; 4];
            let name = [0u8; 256];
            let n_out = ((sc >> 8) & 0xff) as usize;
            if n_out >= 3 {
                fp_rev_encode_outs(&[&zero4, &name, &zero4]);
            } else {
                fp_rev_encode_empty_outs(sc, true);
            }
            0
        }
        // stat(31)：扩展号，pathname 在 inbufs[1]
        31 => {
            if nb < 2 {
                // 仍回 96 字节零，避免 DSP 按成功布局解包踩坑
                let zb = [0u8; 96];
                fp_rev_encode_outs(&[&zb]);
                return 14;
            }
            let (p0, pl) = bufs[1];
            if pl == 0 || src[p0 + pl - 1] != 0 {
                let zb = [0u8; 96];
                fp_rev_encode_outs(&[&zb]);
                return 14;
            }
            fp_rev_set_path_name(&src[p0..p0 + pl]);
            // 策略：对任意路径回「64B 普通文件」成功，逼 DSP 走 fopen/fread，
            // 以便 sm 打出路径名；内容仍是零（缺真实 .so / registry）。
            // mode: S_IFREG|0644 = 0x8000|0x1a4 = 0x81a4
            let st = fp_ast_stat_bytes(64, 0x81a4);
            fp_rev_encode_outs(&[&st]);
            0
        }
        _ => {
            fp_rev_encode_empty_outs(sc, true);
            20
        }
    }
}

fn fp_rev_dispatch() {
    let sc = unsafe { FP_NOUT[2] };
    let handle = unsafe { FP_NOUT[1] };
    unsafe { FP_RVSC = sc };
    let mut method = (sc >> 24) & 0x1f;
    // method==31：扩展号在 inbufs[0] 首字（hexagonrpcd listener.c）
    if method == 31 {
        let n = unsafe { FP_RVINN } as usize;
        if n >= 4 {
            let mut align = 0usize;
            if let Some((o0, l0)) = fp_rev_take_buf(unsafe { &FP_RVIN[..n] }, 0, &mut align) {
                if l0 >= 4 {
                    method = u32::from_le_bytes([
                        unsafe { FP_RVIN[o0] },
                        unsafe { FP_RVIN[o0 + 1] },
                        unsafe { FP_RVIN[o0 + 2] },
                        unsafe { FP_RVIN[o0 + 3] },
                    ]);
                }
            }
        }
    }
    let result = if handle == 0 && method == 0 {
        fp_rev_remotectl_open()
    } else if handle == 1 {
        let r = fp_rev_apps_std(method, sc);
        unsafe {
            FP_RVRES = r;
            FP_RVN = FP_RVN.wrapping_add(1);
        }
        r
    } else if handle == 2 {
        // apps_mem：暂不支持
        let err = 20u32;
        fp_rev_encode_empty_outs(sc, true);
        unsafe {
            FP_RVRES = err;
            FP_RVN = FP_RVN.wrapping_add(1);
        }
        err
    } else {
        let err = 20u32;
        fp_rev_encode_empty_outs(sc, true);
        unsafe {
            FP_RVRES = err;
            FP_RVN = FP_RVN.wrapping_add(1);
        }
        err
    };
    let _ = result;
    unsafe { FP_REV = 2 };
}

/// 发续 next2：带上上一轮 reverse 的 result + 编码 outbufs。
fn fastrpc_next2_cont(d: u64, txf: u64, txn: u64, at: u64) -> bool {
    if unsafe { FP_REUSE } == 0 && unsafe { FP_INEW } != 1 {
        if unsafe { FP_INEW } == 0 && at + 8 <= txn {
            lpass_store(txf + at, 7 | (4 << 16));
            lpass_store(txf + at + 4, 64);
            lpass_tx_commit(d, txf, at, 8);
            unsafe { FP_INEW = 2 };
            return true;
        }
        return false;
    }
    let rcid = unsafe { FP_RCID } as usize;
    if rcid >= 8 {
        return false;
    }
    let iid = unsafe { FP_ILOG[rcid * 2] };
    let isz = unsafe { FP_ILOG[rcid * 2 + 1] };
    let out_n = unsafe { FP_RVOUTN } as u64;
    // prim 16 + optional outbufs (aligned into page) + need 56 for glink hdr
    let total_glink = 56u64;
    if isz < 40 || at + total_glink > txn {
        return false;
    }
    let page = core::ptr::addr_of!(FP_PAGE) as u64;
    if page < 0x8000_0000 || page + 4096 > 0xC000_0000 {
        return false;
    }
    if out_n > 0 && 1024 + out_n > 4096 {
        return false;
    }
    let tagged = (3u64 << 32) | page;
    let mut i = 0u64;
    while i < 4096 {
        wr32((page + i) as usize, 0);
        i += 4;
    }
    // 拷贝编码后的 outbufs 到页内 1024
    if out_n != 0 {
        let mut j = 0u32;
        while j < out_n as u32 {
            unsafe {
                *((page + 1024 + j as u64) as *mut u8) = FP_RVOUT[j as usize];
            }
            j += 1;
        }
    }
    let outs_len = out_n;
    // pra: prim in @640, nested out @1024 (len outs_len), prim out @768, nested in @896
    let offs = [640u64, 1024u64, 768u64, 896u64];
    let lens = [16u64, outs_len, 16u64, 256u64];
    let mut n = 0usize;
    while n < 4 {
        let off = offs[n];
        let len = lens[n];
        if len != 0 {
            let pv = tagged + off;
            wr32((page + n as u64 * 16) as usize, pv as u32);
            wr32((page + n as u64 * 16 + 4) as usize, (pv >> 32) as u32);
        }
        wr32((page + n as u64 * 16 + 8) as usize, len as u32);
        wr32((page + 64 + n as u64 * 8) as usize, if len != 0 { 1 } else { 0 });
        wr32((page + 64 + n as u64 * 8 + 4) as usize, n as u32);
        wr32((page + 96 + n as u64 * 16) as usize, tagged as u32);
        wr32((page + 96 + n as u64 * 16 + 4) as usize, (tagged >> 32) as u32);
        wr32((page + 96 + n as u64 * 16 + 8) as usize, 4096);
        n += 1;
    }
    let rctx = unsafe { FP_NOUT[0] };
    let result = unsafe { FP_RVRES };
    wr32((page + 640) as usize, rctx);
    wr32((page + 644) as usize, result);
    wr32((page + 648) as usize, outs_len as u32);
    wr32((page + 652) as usize, 256);
    dcache_civac(page, 4096);
    let ctx = 0x6002u32;
    // 续调用仍用同一 ctx 槽，便于对账；DSP 不依赖此值匹配
    let msg = [
        1u32,
        1,
        ctx,
        0,
        3u32,
        0x0402_0200,
        tagged as u32,
        (tagged >> 32) as u32,
        4096,
        0,
    ];
    lpass_store(txf + at, 9 | (4 << 16));
    lpass_store(txf + at + 4, iid);
    lpass_store(txf + at + 8, 40);
    lpass_store(txf + at + 12, 0);
    let mut w = 0usize;
    while w < 10 {
        lpass_store(txf + at + 16 + w as u64 * 4, msg[w]);
        w += 1;
    }
    lpass_tx_commit(d, txf, at, 56);
    unsafe {
        FP_SENT = 10;
        FP_LAST = iid;
        FP_REUSE = 0;
        FP_INEW = 0;
        FP_REV = 0;
        FP_NXN = 0;
        FP_NDONE = 0;
    }
    true
}

fn fastrpc_rev_work(d: u64, txf: u64, txn: u64, at: u64) -> bool {
    if unsafe { FP_SENT } != 10 {
        return false;
    }
    // register→init2：RX_DONE 会楔死读端（已证）。但 reverse 循环必须
    // 归还 RX intent，否则 DSP 发完有限 intent 后无法再推嵌套 invoke
    //（实证 RVN 停在 ~5、TX 空闲、无新 6002）。此处对 NLIID 发
    // RX_DONE_W_REUSE，与 register 的 GLIID 路径分开。
    if unsafe { FP_NXN } != 0 && unsafe { FP_NDONE } == 0 {
        let liid = unsafe { FP_NLIID };
        if liid != 0 && at + 8 <= txn {
            lpass_store(txf + at, 14 | (4 << 16));
            lpass_store(txf + at + 4, liid);
            lpass_tx_commit(d, txf, at, 8);
            unsafe { FP_NDONE = 1 }; // 1 = 已发 reuse
            return true; // 下一拍再 dispatch / cont（TX 可能需等 idle）
        }
        unsafe { FP_NDONE = 2 };
    }
    if unsafe { FP_REV } == 1 {
        fp_rev_dispatch();
    }
    if unsafe { FP_REV } == 2 {
        return fastrpc_next2_cont(d, txf, txn, at);
    }
    false
}

/// `fastrpc_init_process` 的 FASTRPC_INIT_ATTACH_SENSORS。
/// handle 1，method 0，一个 4 字节 tgid，pd=2。一拍只发一包，接收 FIFO 必须是空的。
fn fastrpc_attach(d: u64, txf: u64, txn: u64, at: u64) -> bool {
    if unsafe { LG_ARM } == 0 {
        return false;
    }
    let sent = unsafe { FP_SENT };
    if sent == 1 {
        return fastrpc_remotectl(d, txf, txn, at);
    }
    if sent == 4 {
        return fastrpc_register(d, txf, txn, at);
    }
    if sent == 5 {
        return fastrpc_init2(d, txf, txn, at);
    }
    // init2 已提交后补发 register 的 RX_DONE（跳过楔死位，延迟到此）。
    // 内核对账：归还对方用过的 intent，DSP 才能再 TX 回 init2 应答。
    if sent == 9 && unsafe { FP_GDONE } == 2 {
        let liid = unsafe { FP_GLIID };
        if liid != 0 && at + 8 <= txn {
            lpass_store(txf + at, 14 | (4 << 16));
            lpass_store(txf + at + 4, liid);
            lpass_tx_commit(d, txf, at, 8);
            unsafe { FP_GDONE = 3 }; // 3 = 延迟已发
            return true;
        }
        unsafe { FP_GDONE = 3 };
    }
    if sent == 9 {
        return fastrpc_next2(d, txf, txn, at);
    }
    if sent == 10 {
        return fastrpc_rev_work(d, txf, txn, at);
    }
    if sent != 0 {
        return false;
    }
    if unsafe { LG_BITS } & (1 << 11) == 0 {
        return false;
    }
    let rcid = unsafe { FP_RCID };
    if rcid == 0xffff_ffff {
        return false;
    }
    if unsafe { FP_ISZ } == 0 && rcid < 8 && unsafe { FP_IN } & (1 << rcid) != 0 {
        unsafe {
            FP_IID = FP_ILOG[rcid as usize * 2];
            FP_ISZ = FP_ILOG[rcid as usize * 2 + 1];
        }
    }
    let iid = unsafe { FP_IID };
    let isz = unsafe { FP_ISZ };
    if isz < 40 {
        // qcom_glink_request_intent：没有 intent 就先要一块 64 字节的。
        if unsafe { FP_REQ } == 0 && at + 8 <= txn {
            lpass_store(txf + at, 7 | (4 << 16));
            lpass_store(txf + at + 4, 64);
            lpass_tx_commit(d, txf, at, 8);
            unsafe { FP_REQ = 1 };
            return true;
        }
        return false;
    }
    if !fastrpc_smmu() {
        unsafe { FP_SENT = 2 };
        return false;
    }
    let page = core::ptr::addr_of!(FP_PAGE) as u64;
    if page < 0x8000_0000 || page + 4096 > 0xC000_0000 {
        unsafe { FP_SENT = 3 };
        return false;
    }
    let iova = page;
    let tagged = (3u64 << 32) | iova;
    let mut i = 0u64;
    while i < 4096 {
        wr32((page + i) as usize, 0);
        i += 4;
    }
    // remote_arg64：pv 指向页内 tgid，len=4。invoke buf：1 块，pgidx 0。
    // phy page：这一页，4KB。tgid 放在 128 字节对齐的 512 处。
    let data = tagged + 512;
    wr32((page + 0) as usize, data as u32);
    wr32((page + 4) as usize, (data >> 32) as u32);
    wr32((page + 8) as usize, 4);
    wr32((page + 12) as usize, 0);
    wr32((page + 16) as usize, 1);
    wr32((page + 20) as usize, 0);
    wr32((page + 24) as usize, tagged as u32);
    wr32((page + 28) as usize, (tagged >> 32) as u32);
    wr32((page + 32) as usize, 4096);
    wr32((page + 36) as usize, 0);
    wr32((page + 512) as usize, 1);
    dcache_civac(page, 4096);
    // smq_msg：pid 0，tid 1，ctx=(1<<12)|pd2，handle 1，sc=method0/1 in。
    let sc = (1u32) << 16;
    let ctx = (1u32 << 12) | 2;
    let msg = [
        0u32,
        1,
        ctx,
        0,
        1,
        sc,
        tagged as u32,
        (tagged >> 32) as u32,
        4096,
        0,
    ];
    let total = 16 + 40;
    if at + total as u64 > txn {
        return false;
    }
    // __qcom_glink_send：param1 是自己的 lcid。对方按 rcid 查通道，
    // 再按 param2 查自己的 liid。写成对方的 rcid 会落到别的通道，
    // DSP 在 glink_core_intent.c:777 对 lc_intent 断言。
    lpass_store(txf + at, 9 | (4 << 16));
    lpass_store(txf + at + 4, iid);
    lpass_store(txf + at + 8, 40);
    lpass_store(txf + at + 12, 0);
    let mut w = 0usize;
    while w < 10 {
        lpass_store(txf + at + 16 + w as u64 * 4, msg[w]);
        w += 1;
    }
    lpass_tx_commit(d, txf, at, total as u64);
    unsafe {
        FP_SENT = 1;
        FP_LAST = iid;
        FP_REUSE = 0;
    }
    true
}

/// `qcom_glink_native.c` 的接收循环，一拍只发一包。
/// 对方 OPEN 之后：OPEN_ACK，下一拍再 OPEN 同名通道，ACK 之后按 DT 发 intent。
fn lpass_glink_work() {
    let d = unsafe { LG_D };
    let txf = unsafe { LG_TX };
    let txn = unsafe { LG_TXN };
    let rxf = unsafe { LG_RX };
    let rxn = unsafe { LG_RXN };
    if d == 0 || txf == 0 || rxf == 0 || rxn < 16 || txn < 16 {
        return;
    }
    // 不回包的命令一拍最多消化 8 个，好让 FIFO 排空。自己的 OPEN 只在排空之后发。
    let mut drained = 0u32;
    while drained < 8 {
        let head = smem_rd32(d + 12).unwrap_or(0) as u64;
        let tail = smem_rd32(d + 8).unwrap_or(0) as u64;
        if tail > rxn || head > rxn || tail > head || tail + 8 > head {
            break;
        }
        let Some(w0) = smem_rd32(rxf + tail) else {
            return;
        };
        let Some(w1) = smem_rd32(rxf + tail + 4) else {
            return;
        };
        let cmd = w0 & 0xffff;
        let param1 = w0 >> 16;
        let param2 = w1 & 0xffff;
        if cmd == 0 && unsafe { LG_BITS } & (1 << 16) == 0 {
            let Some(at) = lpass_tx_idle(d, txn, 8) else {
                return;
            };
            lpass_store(txf + at, 1 | (1 << 16));
            lpass_store(txf + at + 4, 1);
            lpass_tx_commit(d, txf, at, 8);
            unsafe { LG_BITS |= 1 << 16 };
            lpass_rx_advance(d, 8, rxn);
            return;
        }
        if cmd == 2 {
            let nlen = param2;
            if nlen > 32 || tail + 8 + nlen as u64 > head {
                return;
            }
            let step = (8 + nlen as u64 + 7) & !7;
            if tail + step > head {
                return;
            }
            if let Some(ch) = lpass_ch(rxf, tail, nlen) {
                let saw = 1u32 << ch;
                if unsafe { LG_BITS } & saw == 0 {
                    let Some(at) = lpass_tx_idle(d, txn, 8) else {
                        return;
                    };
                    lpass_store(txf + at, 4 | (param1 << 16));
                    lpass_store(txf + at + 4, 0);
                    lpass_tx_commit(d, txf, at, 8);
                    unsafe {
                        LG_BITS |= saw;
                        LG_NEED |= 1 << ch;
                        if ch == 3 {
                            FP_RCID = param1;
                        }
                    }
                    lpass_rx_advance(d, step, rxn);
                    return;
                }
            }
            lpass_rx_advance(d, step, rxn);
            drained += 1;
            continue;
        }
        if cmd == 7 {
            // 内核 qcom_glink_handle_intent_req：先 advertise INTENT(cmd5)，
            // 再 ACK；ACK 的 param1 是 **本机 lcid**，不是对方 cid。
            // 旧逻辑回显 param1（对方 lcid=FP_RCID）且不发 cmd5 → DSP
            // 以为 granted 却永远等不到 intent，init2 只能还 TX intent 不回包。
            let size = param2;
            if size == 0 || size > 0x10000 {
                lpass_rx_advance(d, 8, rxn);
                drained += 1;
                continue;
            }
            let our_lcid = if param1 == unsafe { FP_RCID } {
                4u32
            } else if (1..=4).contains(&param1) {
                // 兜底：按本机 ch+1 试；OPEN 时对方 cid 偶发与本机相同编号
                param1
            } else {
                4u32
            };
            let need = 16u64 + 8; // INTENT 16B + ACK 8B
            let Some(at) = lpass_tx_idle(d, txn, need) else {
                return;
            };
            let ch = our_lcid.saturating_sub(1) as usize;
            let sent = if ch < 4 {
                unsafe { LG_ISENT[ch] }
            } else {
                0
            };
            let id = (sent as u32).saturating_add(1);
            let sz = if size < 0x40 { 0x40 } else { size };
            lpass_store(txf + at, 5 | (our_lcid << 16));
            lpass_store(txf + at + 4, 1);
            lpass_store(txf + at + 8, sz);
            lpass_store(txf + at + 12, id);
            lpass_store(txf + at + 16, 8 | (our_lcid << 16));
            lpass_store(txf + at + 20, 1); // granted
            lpass_tx_commit(d, txf, at, 24);
            if ch < 4 && sent < 255 {
                unsafe { LG_ISENT[ch] = sent + 1 };
            }
            lpass_rx_advance(d, 8, rxn);
            return;
        }
        let step = if cmd == 0 || cmd == 1 || cmd == 4 || cmd == 6 || cmd == 8 || cmd == 13 || cmd == 14 || cmd == 15
        {
            8
        } else if cmd == 5 && tail + 16 <= head {
            let count = smem_rd32(rxf + tail + 4).unwrap_or(0);
            let size = smem_rd32(rxf + tail + 8).unwrap_or(0);
            let id = smem_rd32(rxf + tail + 12).unwrap_or(0);
            if count >= 1 && count <= 16 && size >= 40 && param1 < 8 {
                unsafe {
                    FP_ILOG[param1 as usize * 2] = id;
                    FP_ILOG[param1 as usize * 2 + 1] = size;
                    FP_IN |= 1 << param1;
                    if param1 == FP_RCID && (FP_REUSE == 2 || FP_INEW == 2) {
                        FP_INEW = 1;
                    }
                }
            }
            if count >= 1 && count <= 16 {
                8 + (count as u64) * 8
            } else {
                16
            }
        } else if cmd == 9 && tail + 16 <= head {
            let chunk = smem_rd32(rxf + tail + 8).unwrap_or(0) as u64;
            if chunk > 512 || tail + 16 + chunk > head + 16 {
                return;
            }
            // qcom_glink_rx_data：param1 是发送方的 lcid，也就是我们的 rcid。
            // 自己的 lcid 4 留着，免得方向看反。
            let mine = unsafe { FP_RCID };
            let watch = param1 == 4 || (mine != 0xffff_ffff && param1 == mine);
            if watch && unsafe { LG_FPN } == 0 {
                unsafe {
                    FP_RXP = param1;
                    FP_RLIID = smem_rd32(rxf + tail + 4).unwrap_or(0);
                }
                let words = ((chunk as u32).min(32) + 3) / 4;
                let mut i = 0u32;
                while i < words && i < 8 {
                    unsafe {
                        LG_FP[i as usize] =
                            smem_rd32(rxf + tail + 16 + i as u64 * 4).unwrap_or(0);
                    }
                    i += 1;
                }
            } else if watch && unsafe { FP_OPN } == 0 && unsafe { LG_FPN } >= 1 {
                unsafe {
                    FP_OPN = 1;
                    FP_OLIID = smem_rd32(rxf + tail + 4).unwrap_or(0);
                }
                let words = ((chunk as u32).min(16) + 3) / 4;
                let mut i = 0u32;
                while i < words && i < 4 {
                    unsafe {
                        FP_OPEN[i as usize] =
                            smem_rd32(rxf + tail + 16 + i as u64 * 4).unwrap_or(0);
                    }
                    i += 1;
                }
            } else if watch && unsafe { FP_REGN } == 0 && unsafe { FP_OPN } != 0 && unsafe { LG_FPN } >= 2
            {
                unsafe {
                    FP_REGN = 1;
                    FP_GLIID = smem_rd32(rxf + tail + 4).unwrap_or(0);
                }
                let words = ((chunk as u32).min(16) + 3) / 4;
                let mut i = 0u32;
                while i < words && i < 4 {
                    unsafe {
                        FP_REG[i as usize] =
                            smem_rd32(rxf + tail + 16 + i as u64 * 4).unwrap_or(0);
                    }
                    i += 1;
                }
            } else if watch
                && unsafe { FP_CLSN } == 0
                && unsafe { FP_REGN } != 0
                && unsafe { LG_FPN } >= 3
            {
                unsafe {
                    FP_CLSN = 1;
                    FP_CLIID = smem_rd32(rxf + tail + 4).unwrap_or(0);
                }
                let words = ((chunk as u32).min(16) + 3) / 4;
                let mut i = 0u32;
                while i < words && i < 4 {
                    unsafe {
                        FP_CLS[i as usize] =
                            smem_rd32(rxf + tail + 16 + i as u64 * 4).unwrap_or(0);
                    }
                    i += 1;
                }
            } else if watch
                && unsafe { FP_ININ } == 0
                && unsafe { FP_CLSN } != 0
                && unsafe { LG_FPN } >= 4
            {
                unsafe { FP_ININ = 1 };
                let words = ((chunk as u32).min(16) + 3) / 4;
                let mut i = 0u32;
                while i < words && i < 4 {
                    unsafe {
                        FP_INI[i as usize] =
                            smem_rd32(rxf + tail + 16 + i as u64 * 4).unwrap_or(0);
                    }
                    i += 1;
                }
            }
            if watch {
                let ctxw = smem_rd32(rxf + tail + 16).unwrap_or(0);
                let retw = smem_rd32(rxf + tail + 24).unwrap_or(0);
                let liid = smem_rd32(rxf + tail + 4).unwrap_or(0);
                if ctxw == 0x5002 && unsafe { FP_ININ } == 0 {
                    unsafe {
                        FP_ININ = 1;
                        FP_INI[0] = ctxw;
                        FP_INI[2] = retw;
                        FP_ILIID = liid;
                    }
                }
                if ctxw == 0x6002 {
                    unsafe {
                        FP_NXN = 1;
                        FP_NX[0] = ctxw;
                        FP_NX[2] = retw;
                        FP_NLIID = liid;
                        FP_NDONE = 0;
                        FP_REV = 1;
                    }
                    // prim out @768、nested inbufs @896（invoke 页）
                    let page = core::ptr::addr_of!(FP_PAGE) as u64;
                    dcache_ivac(page + 768, 256 + 16);
                    let mut k = 0usize;
                    while k < 4 {
                        unsafe {
                            FP_NOUT[k] = rd32((page + 768 + k as u64 * 4) as usize);
                        }
                        k += 1;
                    }
                    let n = unsafe { FP_NOUT[3] }.min(256);
                    unsafe { FP_RVINN = n };
                    let mut i = 0u32;
                    while i < n {
                        unsafe {
                            FP_RVIN[i as usize] =
                                *((page + 896 + i as u64) as *const u8);
                        }
                        i += 1;
                    }
                }
                unsafe { LG_FPN = LG_FPN.saturating_add(1) };
            }
            (16 + chunk + 7) & !7
        } else {
            return;
        };
        if step < 8 || tail + step > head {
            return;
        }
        if cmd == 4 && (1..=4).contains(&param1) {
            unsafe { LG_BITS |= 1 << (8 + (param1 - 1)) };
        }
        // __qcom_glink_rx_done：对方用自己的 lcid，liid 是我们发出去时用的 intent。
        if (cmd == 6 || cmd == 14) && param1 == unsafe { FP_RCID } {
            unsafe {
                FP_DN = FP_DN.wrapping_add(1);
                FP_RDP = param2;
            }
            if unsafe { FP_LAST } != 0xffff_ffff && param2 == unsafe { FP_LAST } {
                unsafe { FP_REUSE = if cmd == 14 { 1 } else { 2 } };
            }
        }
        if cmd == 0 {
            unsafe { LG_BITS |= 1 << 16 };
        }
        lpass_rx_advance(d, step, rxn);
        drained += 1;
    }
    let head = smem_rd32(d + 12).unwrap_or(0) as u64;
    let tail = smem_rd32(d + 8).unwrap_or(0) as u64;
    if tail > rxn || head > rxn || tail > head || tail + 8 <= head {
        // RX 未排空会提前返回；若 TX 也 pending，同样补踢，避免绕过下面分支。
        let th = smem_rd32(d + 4).unwrap_or(0);
        let tt = smem_rd32(d).unwrap_or(0);
        if th != tt {
            wr32(0x4080_0c, 3 << 16);
        }
        return;
    }
    let Some(at) = lpass_tx_idle(d, txn, 16) else {
        // TX 还有未消费数据。IPCC 补踢已证伪。内核在 tx_avail 不足时
        // 发 GLINK_CMD_READ_NOTIF(13)，请对方继续读 TX 环；此处同形，
        // 只发一次（FP_RNOTIF），其后仅补踢。
        let th = smem_rd32(d + 4).unwrap_or(0) as u64;
        let tt = smem_rd32(d).unwrap_or(0) as u64;
        // READ_NOTIF 已证伪为解锁手段，且会堵在 ACK 前。pending 时只补踢。
        wr32(0x4080_0c, 3 << 16);
        return;
    };
    let need = unsafe { LG_NEED };
    if need != 0 {
        let mut ch = need.trailing_zeros();
        // fastrpc 的 OPEN 等到 soak 转过一圈。开机早期发它，USB 起不来。
        if ch == 3 && unsafe { LG_ARM } == 0 {
            ch = 4;
        }
        if ch < 4 {
            let name = lpass_ch_name(ch);
            let lcid = ch + 1;
            let nlen = name.len() as u64;
            let total = (8 + nlen + 7) & !7;
            if at + total > txn {
                return;
            }
            let mut i = 0u64;
            while i < total {
                lpass_store(txf + at + i, 0);
                i += 4;
            }
            lpass_store(txf + at, 2 | (lcid << 16));
            lpass_store(txf + at + 4, nlen as u32);
            let mut o = 0usize;
            while o < name.len() {
                let mut b = [0u8; 4];
                let mut k = 0;
                while k < 4 && o + k < name.len() {
                    b[k] = name[o + k];
                    k += 1;
                }
                lpass_store(txf + at + 8 + o as u64, u32::from_le_bytes(b));
                o += 4;
            }
            lpass_tx_commit(d, txf, at, total);
            unsafe {
                LG_NEED &= !(1 << ch);
                LG_BITS |= 1 << (4 + ch);
            }
        }
        return;
    }
    if fastrpc_attach(d, txf, txn, at) {
        return;
    }
    // next2 / reverse 进行中不要灌 INTENT，避免堵死 TX。
    if unsafe { FP_SENT } == 10 {
        return;
    }
    let mut ch = 0u32;
    while ch < 4 {
        let acked = unsafe { LG_BITS } & (1 << (8 + ch)) != 0;
        let sent = unsafe { LG_ISENT[ch as usize] };
        if acked && sent < lpass_intent_n(ch) {
            lpass_store(txf + at, 5 | ((ch + 1) << 16));
            lpass_store(txf + at + 4, 1);
            lpass_store(txf + at + 8, lpass_intent_sz(ch, sent));
            lpass_store(txf + at + 12, (sent as u32) + 1);
            lpass_tx_commit(d, txf, at, 16);
            unsafe {
                LG_ISENT[ch as usize] = sent + 1;
                if sent + 1 >= lpass_intent_n(ch) {
                    LG_BITS |= 1 << (12 + ch);
                }
            }
            return;
        }
        ch += 1;
    }
}

/// ADSP 的 IPCRTR 是另一条 GLINK。qrtr.c 在这条端点上也要先发 HELLO。
fn lpass_qrtr() {
    let d = unsafe { LG_D };
    let txf = unsafe { LG_TX };
    let txn = unsafe { LG_TXN };
    let rxf = unsafe { LG_RX };
    let rxn = unsafe { LG_RXN };
    if d == 0 || txf == 0 || rxf == 0 || rxn < 16 {
        return;
    }
    let head = smem_rd32(d + 12).unwrap_or(0) as u64;
    let tail = smem_rd32(d + 8).unwrap_or(0) as u64;
    if head > rxn || tail > head {
        return;
    }
    let mut off = tail;
    let mut guard = 0u32;
    let mut iid = unsafe { LG_IID };
    let mut isz = unsafe { LG_ISZ };
    while off + 8 <= head && guard < 40 {
        guard += 1;
        let Some(w0) = smem_rd32(rxf + off) else {
            break;
        };
        let cmd = w0 & 0xffff;
        let step = if cmd == 5 && off + 16 <= head {
            let count = smem_rd32(rxf + off + 4).unwrap_or(0);
            let size = smem_rd32(rxf + off + 8).unwrap_or(0);
            let id = smem_rd32(rxf + off + 12).unwrap_or(0);
            if count >= 1 && size >= 52 && size >= isz {
                iid = id;
                isz = size;
            }
            16
        } else if cmd == 9 {
            let chunk = smem_rd32(rxf + off + 8).unwrap_or(0) as u64;
            if chunk > 512 || off + 16 + chunk > head + 16 {
                break;
            }
            if chunk >= 40 && smem_rd32(rxf + off + 20) == Some(2) {
                unsafe {
                    if off >= LG_HVIS {
                        LG_HN = LG_HN.wrapping_add(1);
                        LG_HVIS = off + ((16 + chunk + 7) & !7);
                    }
                }
            }
            (16 + chunk + 7) & !7
        } else if cmd == 2 {
            let nlen = smem_rd32(rxf + off + 4).unwrap_or(0) & 0xffff;
            if nlen > 64 {
                break;
            }
            (8 + nlen as u64 + 7) & !7
        } else {
            8
        };
        if step < 8 || off + step > head {
            break;
        }
        off += step;
    }
    unsafe {
        LG_IID = iid;
        LG_ISZ = isz;
    }
    if unsafe { LG_HELLO } != 0 || isz < 52 {
        return;
    }
    let tx_head = smem_rd32(d + 4).unwrap_or(0) as u64;
    let tx_tail = smem_rd32(d).unwrap_or(0) as u64;
    if tx_head != tx_tail || tx_head + 72 > txn {
        return;
    }
    let p = txf + tx_head;
    let mut i = 0u64;
    while i < 72 {
        lpass_store(p + i, 0);
        i += 4;
    }
    // IPCRTR 是我们开的 lcid 1。包和 modem 边上的 HELLO 一样。
    lpass_store(p, 9 | (1 << 16));
    lpass_store(p + 4, iid);
    lpass_store(p + 8, 52);
    lpass_store(p + 16, 1);
    lpass_store(p + 20, 2);
    lpass_store(p + 24, 1);
    lpass_store(p + 28, 0xffff_fffe);
    lpass_store(p + 36, 20);
    lpass_store(p + 40, 0xffff_ffff);
    lpass_store(p + 44, 0xffff_fffe);
    lpass_store(p + 48, 2);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    lpass_store(d + 4, (tx_head + 72) as u32);
    wr32(0x4080_0c, 3 << 16);
    unsafe { LG_HELLO = 1 };
}

fn lpass_glink() -> u32 {
    if unsafe { LG_PHASE } >= 2 {
        unsafe {
            core::arch::asm!("msr daifclr, #8", "isb", options(nostack));
        }
        const SMEM_BASE: u64 = 0x8090_0000;
        let toc = SMEM_BASE + 0x20_0000 - 0x1000;
        let index = ((0x80A0_0000u64 - 0x8000_0000) >> 21) as usize;
        let slot = unsafe { (core::ptr::addr_of_mut!(L2) as *mut u64).add(index) };
        let old = unsafe { slot.read_volatile() };
        l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
        lpass_glink_work();
        let gd = unsafe { LG_D };
        if gd != 0 {
            unsafe {
                FP_CH = smem_rd32(gd + 4).unwrap_or(0xffff_ffff);
                FP_CT = smem_rd32(gd).unwrap_or(0xffff_ffff);
            }
        }
        lpass_qrtr();
        l2_swap(index, old, toc >> 12);
        return unsafe { LG_ST };
    }
    unsafe {
        core::arch::asm!("msr daifclr, #8", "isb", options(nostack));
    }
    const SMEM_BASE: u64 = 0x8090_0000;
    let toc = SMEM_BASE + 0x20_0000 - 0x1000;
    let index = ((0x80A0_0000u64 - 0x8000_0000) >> 21) as usize;
    let slot = unsafe { (core::ptr::addr_of_mut!(L2) as *mut u64).add(index) };
    let old = unsafe { slot.read_volatile() };
    l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
    let code = lpass_glink_in(false);
    l2_swap(index, old, toc >> 12);
    if code == 0x20 {
        let at = unsafe { LG_AT };
        let len = unsafe { LG_LEN };
        if len > 0 && at >= 0x80A0_0000 && at.saturating_add(len) <= 0x80AFE000 {
            dcache_civac(at, len);
            unsafe { LG_FLUSH = at; }
        }
        l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
        let code2 = lpass_glink_in(true);
        l2_swap(index, old, toc >> 12);
        unsafe { LG_ST = code2 };
        return code2;
    }
    unsafe { LG_ST = code };
    code
}

/// 打开 TOC 那一页的 NC 窗口，做完 LPASS 边再换回去。
fn lpass_edge(pub_out: bool) -> (u32, u32, u32) {
    unsafe {
        core::arch::asm!("msr daifclr, #8", "isb", options(nostack));
    }
    const SMEM_BASE: u64 = 0x8090_0000;
    let toc = SMEM_BASE + 0x20_0000 - 0x1000;
    let index = ((0x80A0_0000u64 - 0x8000_0000) >> 21) as usize;
    let slot = unsafe { (core::ptr::addr_of_mut!(L2) as *mut u64).add(index) };
    let old = unsafe { slot.read_volatile() };
    if pub_out {
        unsafe { LPASS_FLUSHED = 0; }
    }
    l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
    let out = lpass_edge_in(pub_out, SMEM_BASE, toc);
    l2_swap(index, old, toc >> 12);
    if pub_out && out.0 == 9 {
        let hdr = unsafe { LPASS_HDR };
        let base = SMEM_BASE + out.2 as u64;
        // 只作废这一项和分区头，不作废整块，也不碰到 TOC。
        if hdr >= 0x80A0_0000 && hdr + 0x168 <= 0x80AFE000 && base < 0x80AFE000 {
            dcache_civac(hdr, 0x168);
            dcache_civac(base, 64);
            unsafe { LPASS_FLUSHED = hdr; }
            l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
            let out2 = lpass_edge_in(true, SMEM_BASE, toc);
            l2_swap(index, old, toc >> 12);
            unsafe { LPASS_FLUSHED = 0; }
            return out2;
        }
    }
    out
}

/// `qcom,smp2p-cdsp`：分区 host 0/5，入向 94，出向 432，remote-pid 5。
/// IPCC client CDSP(6) signal SMP2P(2)。结构对齐 `lpass_edge_in`。
fn cdsp_edge_in(pub_out: bool, smem_base: u64, toc: u64) -> (u32, u32, u32) {
    let magic = rd32(toc as usize);
    if magic != 0x434f_5424 {
        return (7, 0x8000_0000, 0);
    }
    let nent = rd32((toc + 8) as usize).min(48);
    let mut part = 0u32;
    let mut e = toc + 32;
    for _ in 0..nent {
        let off = rd32(e as usize);
        let sz = rd32((e + 4) as usize);
        let h0 = rd16a(e + 12) as u32;
        let h1 = rd16a(e + 14) as u32;
        e += 48;
        if off == 0 || sz < 32 || off >= 0x20_0000 || off + sz > 0x20_0000 {
            continue;
        }
        if (h0 == 0 && h1 == 5) || (h0 == 5 && h1 == 0) {
            part = off;
            break;
        }
    }
    if part == 0 {
        return (5, 0x8000_0000, 0);
    }
    let base = smem_base + part as u64;
    let Some(pm) = smem_rd32(base) else {
        return (6, 0x8000_0000, part);
    };
    if pm != 0x5452_5024 {
        return (6, 0x8000_0000, part);
    }
    let Some(psz) = smem_rd32(base + 8) else {
        return (6, 0x8000_0000, part);
    };
    let Some(free_u) = smem_rd32(base + 12) else {
        return (6, 0x8000_0000, part);
    };
    let Some(cached) = smem_rd32(base + 16) else {
        return (6, 0x8000_0000, part);
    };
    let room = 0x20_0000 - part;
    let end_off = free_u.min(psz).min(room);
    let mut at432 = 0u64;
    let mut at94 = 0u64;
    let mut boff = 0u32;
    let mut slot_ok = false;
    if end_off >= 32 {
        let end = base + end_off as u64;
        let mut p = base + 32;
        for _ in 0..128 {
            if p + 16 > end || p + 16 > 0x80B0_0000 || (p < 0x80A0_0000 && p + 16 > 0x80A0_0000) {
                boff = (p - base) as u32;
                slot_ok = boff == free_u;
                break;
            }
            let Some(can) = smem_rd32(p) else {
                return (8, 0x8000_0000, part);
            };
            if (can as u16) != 0xa5a5 {
                boff = (p - base) as u32;
                slot_ok = boff == free_u && can == 0 && smem_rd32(p + 4) == Some(0);
                break;
            }
            let item = (can >> 16) as u16;
            let Some(isz_w) = smem_rd32(p + 4) else {
                return (8, 0x8000_0000, part);
            };
            let Some(pads) = smem_rd32(p + 8) else {
                return (8, 0x8000_0000, part);
            };
            let isz = isz_w as u64;
            let pad = (pads >> 16) as u64;
            let data = p + 16 + pad;
            if item == 432 {
                at432 = data;
            }
            if item == 94 {
                at94 = data;
            }
            let next = p + 16 + pad + isz;
            if next <= p || isz > 0x2_0000 {
                return (8, 0x8000_0000, part);
            }
            p = next;
        }
    }
    let slave = if at94 != 0 {
        lpass_slave(at94)
    } else {
        0x8000_0000
    };
    if !pub_out && at94 != 0 {
        cdsp_negotiate(at432, at94);
    }
    if !pub_out || at432 != 0 {
        let code = if pub_out {
            0
        } else if at94 == 0 {
            0x10
        } else if slave == 0x8000_0004 {
            0x12
        } else if slave & 0x8000_0000 != 0 {
            0x13
        } else {
            0x11
        };
        return (code, slave, part);
    }
    if !slot_ok {
        return (8, slave, part);
    }
    const ALLOC: u32 = 0x168;
    if free_u.saturating_add(ALLOC) > cached {
        return (2, slave, part);
    }
    let hdr = base + free_u as u64;
    let item_end = hdr + ALLOC as u64;
    unsafe { CDSP_HDR = hdr };
    let upper = hdr >= 0x80A0_0000;
    if item_end > 0x80AFE000
        || base >= 0x80AFE000
        || (!upper && item_end > 0x80A0_0000)
    {
        return (9, slave, part);
    }
    if upper && unsafe { CDSP_FLUSHED } != hdr {
        return (9, slave, part);
    }
    const MUTEX: usize = 0x01f4_3000;
    let mut got = false;
    for _ in 0..256 {
        wr32(MUTEX, 1);
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        if rd32(MUTEX) == 1 {
            got = true;
            break;
        }
    }
    if !got {
        return (3, slave, part);
    }
    let Some(now) = smem_rd32(base + 12) else {
        wr32(MUTEX, 0);
        return (6, slave, part);
    };
    if now != free_u {
        wr32(MUTEX, 0);
        return (4, slave, part);
    }
    // canary 0xA5A5，item 432 = 0x1B0。size 0x158，padding_data 4。
    wr32(hdr as usize, 0x01b0_a5a5);
    wr32((hdr + 4) as usize, 0x158);
    wr32((hdr + 8) as usize, 4);
    wr32((hdr + 12) as usize, 0);
    let data = hdr + 16;
    for i in 0..86u64 {
        wr32((data + i * 4) as usize, 0);
    }
    wr32(data as usize, 0x504d_5324);
    // features = SSR_ACK，version 先留 0。local pid 0，remote pid 5。
    wr32((data + 4) as usize, 1 << 8);
    wr32((data + 8) as usize, 5 << 16);
    wr32((data + 12) as usize, 16 | (1 << 16));
    wr32((data + 20) as usize, 0x7473_616d);
    wr32((data + 24) as usize, 0x6b2d_7265);
    wr32((data + 28) as usize, 0x656e_7265);
    wr32((data + 32) as usize, 0x6c);
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    unsafe { (data as *mut u8).add(4).write_volatile(1) };
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    wr32((base + 12) as usize, free_u + ALLOC);
    if upper {
        unsafe {
            core::arch::asm!("dsb sy", options(nostack));
        }
        if base < 0x80A0_0000 {
            dcache_civac(base, 64);
        }
    } else {
        dcache_civac(hdr, ALLOC as u64);
        dcache_civac(base, 64);
    }
    wr32(MUTEX, 0);
    // CDSP client 6，SMP2P signal 2。
    wr32(0x4080_0c, (6 << 16) | 2);
    (1, slave, part)
}

fn cdsp_negotiate(out_item: u64, in_item: u64) {
    if unsafe { CDSP_ACKED } != 0 {
        return;
    }
    let Some(iw4) = smem_rd32(in_item + 4) else {
        return;
    };
    if (iw4 & 0xff) == 0 {
        return;
    }
    if out_item == 0 {
        unsafe { CDSP_ACKED = 1 };
        return;
    }
    if (iw4 & 0xff) != 1 {
        unsafe { CDSP_ACKED = 1 };
        return;
    }
    let Some(ow4) = smem_rd32(out_item + 4) else {
        return;
    };
    let Some(ow16) = smem_rd32(out_item + 16) else {
        return;
    };
    let neg = (iw4 >> 8) & (ow4 >> 8);
    let mut changed = false;
    if neg != (ow4 >> 8) {
        let addr = out_item + 4;
        wr32(addr as usize, (neg << 8) | 1);
        if addr < 0x80A0_0000 {
            dcache_civac(addr, 4);
        }
        changed = true;
    }
    let Some(iw16) = smem_rd32(in_item + 16) else {
        return;
    };
    if iw16 & 1 != 0 && ow16 & 2 == 0 {
        let addr = out_item + 16;
        wr32(addr as usize, ow16 | 2);
        if addr < 0x80A0_0000 {
            dcache_civac(addr, 4);
        }
        changed = true;
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    if changed {
        wr32(0x4080_0c, (6 << 16) | 2);
    }
    unsafe { CDSP_ACKED = 1 };
}

fn cdsp_edge(pub_out: bool) -> (u32, u32, u32) {
    unsafe {
        core::arch::asm!("msr daifclr, #8", "isb", options(nostack));
    }
    const SMEM_BASE: u64 = 0x8090_0000;
    let toc = SMEM_BASE + 0x20_0000 - 0x1000;
    let index = ((0x80A0_0000u64 - 0x8000_0000) >> 21) as usize;
    let slot = unsafe { (core::ptr::addr_of_mut!(L2) as *mut u64).add(index) };
    let old = unsafe { slot.read_volatile() };
    if pub_out {
        unsafe { CDSP_FLUSHED = 0 };
    }
    l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
    let out = cdsp_edge_in(pub_out, SMEM_BASE, toc);
    l2_swap(index, old, toc >> 12);
    if pub_out && out.0 == 9 {
        let hdr = unsafe { CDSP_HDR };
        let base = SMEM_BASE + out.2 as u64;
        if hdr >= 0x80A0_0000 && hdr + 0x168 <= 0x80AFE000 && base < 0x80AFE000 {
            dcache_civac(hdr, 0x168);
            dcache_civac(base, 64);
            unsafe { CDSP_FLUSHED = hdr };
            l2_swap(index, mmu::nc_block(0x80A0_0000), toc >> 12);
            let out2 = cdsp_edge_in(true, SMEM_BASE, toc);
            l2_swap(index, old, toc >> 12);
            unsafe { CDSP_FLUSHED = 0 };
            return out2;
        }
    }
    out
}

/// 在 NC 映射还在时读三项的内容。435 按 smp2p_smem_item，478 是四个 u32。
fn smem_payloads(
    at435: u64,
    sz435: u64,
    at478: u64,
    sz478: u64,
    at480: u64,
    sz480: u64,
) -> (u32, u32, u32, u32, u32, u32, u32, u32, u32) {
    let in_smem = |a: u64, n: u64| a % 4 == 0 && a >= 0x8090_0000 && a + n <= 0x80B0_0000;
    let mut slave = 0u32;
    let mut smpm = 0u32;
    let mut ev = 0u32;
    let mut name0 = 0u32;
    if in_smem(at435, 20) && sz435 >= 20 {
        smpm = rd32(at435 as usize);
        if smpm == 0x504d_5324 {
            let ver = rd32((at435 + 4) as usize) & 0xff;
            let n = (rd16a(at435 + 14) as u64).min(16);
            ev = (ver << 16) | (n as u32);
            if n > 0 && in_smem(at435 + 20, 4) {
                name0 = rd32((at435 + 20) as usize);
            }
            unsafe {
                SMEM_ENTS = [0; 10];
                let lim = n.min(2);
                if in_smem(at435 + 20, lim * 20) {
                    for i in 0..lim {
                        let ent = at435 + 20 + i * 20;
                        let slot = (i as usize) * 5;
                        for w in 0..4 {
                            SMEM_ENTS[slot + w] = rd32((ent + (w as u64) * 4) as usize);
                        }
                        SMEM_ENTS[slot + 4] = rd32((ent + 16) as usize);
                    }
                }
            }
            if in_smem(at435, 20 + n * 20) {
                for i in 0..n {
                    let ent = at435 + 20 + i * 20;
                    let w0 = rd32(ent as usize);
                    let w1 = rd32((ent + 4) as usize);
                    let w2 = rd32((ent + 8) as usize);
                    let w3 = rd32((ent + 12) as usize);
                    // "slave-kernel"：slav / e-ke / rnel
                    if w0 == 0x7661_6c73 && w1 == 0x656b_2d65 && w2 == 0x6c65_6e72 && w3 == 0 {
                        slave = rd32((ent + 16) as usize);
                    }
                    // "ipa\0..."
                    if w0 == 0x0061_7069 && w1 == 0 && w2 == 0 && w3 == 0 {
                        unsafe { MPSS_IN |= 1 };
                    }
                    // "wlan"
                    if w0 == 0x6e61_6c77 && w1 == 0 && w2 == 0 && w3 == 0 {
                        unsafe { MPSS_IN |= 2 };
                    }
                }
            }
        }
    }
    // 干净机器上回答之后，TFTP 换成 ota_firewall，约 6 秒 USB 掉。先不回。
    let mut d0 = 0u32;
    let mut d1 = 0u32;
    let mut d2 = 0u32;
    let mut d3 = 0u32;
    if in_smem(at478, 16) && sz478 >= 16 {
        d0 = rd32(at478 as usize);
        d1 = rd32((at478 + 4) as usize);
        d2 = rd32((at478 + 8) as usize);
        d3 = rd32((at478 + 12) as usize);
    }
    let mut rx0 = 0u32;
    unsafe { SMEM_RX = [0; 22]; }
    if in_smem(at480, 4) && sz480 >= 4 {
        rx0 = rd32(at480 as usize);
        // 前 0x58 字节是版本和三条 OPEN。新数据从那里开始拆。
        let rh = if at478 % 4 == 0 {
            rd32((at478 + 12) as usize) as u64
        } else {
            0
        };
        let start = if rh > 0x58 + 88 {
            (rh - 88) & !3
        } else if rh > 0x58 {
            0x58
        } else {
            0
        };
        let n = ((sz480 - start) / 4).min(22);
        if in_smem(at480 + start, n * 4) {
            for i in 0..n {
                unsafe {
                    SMEM_RX[i as usize] = rd32((at480 + start + i * 4) as usize);
                }
            }
        }
        unsafe { GLINK_RXLEN = sz480; }
        scan_new_servers(at480, rh);
    } else {
        unsafe {
            SMEM_NSVC = 0;
            SMEM_NLOOK = 0;
        }
    }
    (slave, smpm, d0, d1, d2, d3, rx0, ev, name0)
}

/// 从 GET_DOMAIN_LIST 请求里取出 TLV 1 的服务名。
/// 第一层字符串没有长度前缀，TLV 的值就是字符；modem 会在末尾多带一个 NUL。
/// 读到类型 1 才返回 true。名字写进 PD_NAME，碰到 NUL 就停。
fn pd_take_name(fifo: u64, off: u64, chunk: u64, w1: u32) -> bool {
    let msglen = ((w1 >> 8) & 0xffff) as u64;
    if chunk < 32 || msglen < 3 || 7 + msglen > chunk - 32 || msglen > 96 {
        return false;
    }
    let base = fifo + off + 55;
    let mut p = 0u64;
    while p + 3 <= msglen {
        let t = rd8(base + p);
        let ln = rd8(base + p + 1) as u64 | ((rd8(base + p + 2) as u64) << 8);
        p += 3;
        if p + ln > msglen {
            return false;
        }
        if t == 1 {
            let mut n = 0usize;
            let mut k = 0u64;
            while k < ln && n < 64 {
                let b = rd8(base + p + k);
                if b == 0 {
                    break;
                }
                unsafe {
                    PD_NAME[n] = b;
                }
                n += 1;
                k += 1;
            }
            unsafe {
                let mut i = n;
                while i < 64 {
                    PD_NAME[i] = 0;
                    i += 1;
                }
                PD_NAME_N = n as u8;
                let elf = b"kernel/elf_loader";
                if n == elf.len() {
                    let mut same = true;
                    let mut i = 0usize;
                    while i < n {
                        if PD_NAME[i] != elf[i] {
                            same = false;
                            break;
                        }
                        i += 1;
                    }
                    if same {
                        PD_SAW_ELF = 1;
                    }
                }
            }
            return true;
        }
        p += ln;
    }
    false
}

/// 顺着 GLINK 命令走，把 TX_DATA 里的 QRTR NEW_SERVER 服务号记下来。
fn scan_new_servers(fifo: u64, rh: u64) {
    unsafe {
        SMEM_NSVC = 0;
        SMEM_SVC = [0; 16];
        SMEM_NLOOK = 0;
        SMEM_LOOK = [0; 16];
        SMEM_QSTOP = 0;
        SMEM_QCMD = 0;
    }
    if rh < unsafe { RX_VIS } {
        unsafe { RX_VIS = 0; }
    }
    if rh < unsafe { QP_VIS } {
        unsafe { QP_VIS = 0; }
    }
    if rh < unsafe { RX_PARSED } {
        unsafe { RX_PARSED = 0; }
    }
    // 从上次停的地方继续。每次从 0 重扫、最多 200 个包，ACK 落在后面就永远看不见。
    let mut off = unsafe { RX_PARSED };
    let mut guard = 0u32;
    while off + 8 <= rh && guard < 4000 {
        guard += 1;
        let w0 = rd32((fifo + off) as usize);
        let cmd = w0 & 0xffff;
        let step = if cmd == 0
            || cmd == 1
            || cmd == 4
            || cmd == 6
            || cmd == 7
            || cmd == 8
            || cmd == 13
            || cmd == 14
            || cmd == 15
        {
            8
        } else if cmd == 2 {
            let nlen = rd32((fifo + off + 4) as usize) & 0xffff;
            if nlen > 64 {
                break;
            }
            (8 + nlen as u64 + 7) & !7
        } else if cmd == 5 {
            16
        } else if cmd == 9 {
            let chunk = rd32((fifo + off + 8) as usize) as u64;
            if chunk == 0 || off + 16 + chunk > rh + 16 {
                break;
            }
            let step = (16 + chunk + 7) & !7;
            // 大于 512 的包不解析，但不要把后面的 ACK 截掉。
            if chunk > 512 {
                off += step;
                continue;
            }
            // 公告发完再还。只还本机发出的 1..=10，通道 2。不推接收尾。
            let lcid = w0 >> 16;
            let liid = rd32((fifo + off + 4) as usize);
            unsafe {
                // 不限 intent id：OPEN 可能用掉 >10 的 id，不还则 modem 发不出 ALLOC。
                if GLINK_INTENTS
                    && HOST_ADV >= 4
                    && qrtr_lcid(lcid)
                    && liid != 0
                    && off >= RX_VIS
                    && RX_LIID_N < 16
                {
                    RX_LIID[RX_LIID_N as usize] = liid;
                    RX_LIID_N += 1;
                    RX_VIS = off + step;
                }
            }
            if chunk >= 36 {
                let typ = rd32((fifo + off + 20) as usize);
                let dst_node = rd32((fifo + off + 40) as usize);
                let dst_port = rd32((fifo + off + 44) as usize);
                let xfer_port = unsafe { TF_XFER_PORT };
                let on_xfer = xfer_port != 0 && dst_port == xfer_port;
                if typ == 1 && (dst_port == TF_DATA_PORT || dst_port == TF_SVC_PORT || on_xfer) {
                    tf_note_ack(off, fifo);
                }
                if typ == 1 && chunk >= 52 && unsafe { TF_ST } >= 1 {
                    let op = ((rd8(fifo + off + 48) as u32) << 8) | (rd8(fifo + off + 49) as u32);
                    // 0x12 是自家假 pd-mapper 口，QMI 头 00 04 恒撞 ACK 码，不入册。
                    if on_xfer || dst_port == TF_SVC_PORT
                        || (dst_port != 0x12 && (op == 4 || op == 5))
                    {
                        // 刀 G6：第三元=op5 的错误码 / op4 的块号（其余 op 填 ffff）。
                        // EOT(9) 与其他错一眼分清。
                        let arg = if chunk >= 54 {
                            ((rd8(fifo + off + 50) as u32) << 8) | (rd8(fifo + off + 51) as u32)
                        } else {
                            0xffff
                        };
                        let idx = (unsafe { TF_RX_LOG_N } & 3) as usize;
                        unsafe {
                            TF_RX_LOG_P[idx] = dst_port;
                            TF_RX_LOG_O[idx] = op;
                            TF_RX_LOG_A[idx] = arg;
                            TF_RX_LOG_N = TF_RX_LOG_N.wrapping_add(1);
                        }
                        // 刀 G10：op4/op5 顺带抄原文 ≤24B 进双槽环（定宽 nibble
                        // 渲染进 rr=）。op5 消息文本＝拒收理由原文。
                        if op == 4 || op == 5 {
                            let size = rd32((fifo + off + 36) as usize) as usize;
                            let room = chunk as usize - 48;
                            let n = size.min(room).min(24);
                            let ridx = (unsafe { TF_RXR_N } & 1) as usize;
                            let mut i = 0usize;
                            while i < n {
                                unsafe {
                                    TF_RXR[ridx][i] = rd8(fifo + off + 48 + i as u64);
                                }
                                i += 1;
                            }
                            unsafe {
                                TF_RXR_L[ridx] = n as u8;
                                TF_RXR_N = TF_RXR_N.wrapping_add(1);
                            }
                        }
                    }
                }
            }
            if chunk >= 40 {
                let typ = rd32((fifo + off + 20) as usize);
                let dst_node = rd32((fifo + off + 40) as usize);
                let dst_port = rd32((fifo + off + 44) as usize);
                let confirm = rd32((fifo + off + 32) as usize);
                if typ == 2 {
                    unsafe {
                        if off >= HELLO_VIS {
                            HELLO_RX = HELLO_RX.wrapping_add(1);
                            HELLO_VIS = off + step;
                        }
                    }
                }
                // 刀 G5：全口 RRQ/WRQ 日志。tf 扫描只认 dst_port==0x11，客户端若
                // 换口发请求整段盲——窗2 静默期 tt 已清空（TX 全被消费）、pr 停爬，
                // 请求可能正落在看不见的口上。任何发到 node1 的 op1/op2 都入册，
                // 水位去重与 HELLO/QP 同法（off 只进不退）。
                if typ == 1 && dst_node == 1 && chunk >= 50 && off >= unsafe { TF_RL_VIS } {
                    let op0 = ((rd8(fifo + off + 48) as u32) << 8) | (rd8(fifo + off + 49) as u32);
                    if op0 == 1 || op0 == 2 {
                        let sp = rd32((fifo + off + 28) as usize);
                        let (_, saw_rs, rsz, saw_sk, skv) = if op0 == 1 {
                            tf_rrq_rsize(fifo, off, chunk)
                        } else {
                            (false, false, 0, false, 0)
                        };
                        let fl = (if saw_rs { 1u32 } else { 0 })
                            | (if saw_sk { 2u32 } else { 0 });
                        let idx = (unsafe { TF_RL_N } % 6) as usize;
                        unsafe {
                            TF_RL_P[idx] = dst_port;
                            TF_RL_S[idx] = sp;
                            TF_RL_O[idx] = op0;
                            TF_RL_R[idx] = rsz;
                            TF_RL_K[idx] = skv;
                            TF_RL_F[idx] = fl;
                            TF_RL_N = TF_RL_N.wrapping_add(1);
                        }
                    }
                    unsafe { TF_RL_VIS = off + step; }
                }
                if typ == 1 && dst_node == 1 && chunk >= 40 && off >= unsafe { QP_VIS } {
                    let w = rd32((fifo + off + 48) as usize);
                    let w1 = rd32((fifo + off + 52) as usize);
                    let qtype = w & 0xff;
                    let msgid = ((w >> 24) & 0xff) | ((w1 & 0xff) << 8);
                    unsafe {
                        let slot = (QP_HIST_N % 8) as usize;
                        QP_HIST[slot] = (dst_port & 0xff)
                            | ((msgid & 0xff) << 8)
                            | ((qtype & 0xff) << 16)
                            | (((dst_port >> 8) & 0xff) << 24);
                        QP_HIST_N = QP_HIST_N.wrapping_add(1);
                        QP_VIS = off + step;
                    }
                }
                if typ == 1 && dst_node == 1 && confirm != 0 {
                    unsafe {
                        if off >= CF_VIS && !CF_PEND {
                            CF_SN = rd32((fifo + off + 24) as usize);
                            CF_SP = rd32((fifo + off + 28) as usize);
                            CF_DN = dst_node;
                            CF_DP = dst_port;
                            CF_PEND = true;
                            CF_VIS = off + step;
                        }
                    }
                }
                if typ == 1 && dst_node == 1 && dst_port == 0x12 && chunk >= 56 {
                    let w = rd32((fifo + off + 48) as usize);
                    let w1 = rd32((fifo + off + 52) as usize);
                    let qtype = w & 0xff;
                    let txn = (w >> 8) & 0xffff;
                    let msgid = ((w >> 24) & 0xff) | ((w1 & 0xff) << 8);
                    let mut seen = false;
                    unsafe {
                        let mut i = 0u32;
                        while i < PD_SEEN_N {
                            if PD_SEEN[i as usize] == txn {
                                seen = true;
                                break;
                            }
                            i += 1;
                        }
                    }
                    if qtype == 0 && msgid == 33 && !seen && unsafe { !PD_PEND } {
                        if pd_take_name(fifo, off, chunk, w1) {
                            unsafe {
                                PD_TXN = txn;
                                PD_NODE = rd32((fifo + off + 24) as usize);
                                PD_PORT = rd32((fifo + off + 28) as usize);
                                PD_PEND = true;
                            }
                        }
                    }
                }
                if typ == 1 && dst_node == 1 && dst_port == 0x10 && chunk >= 40 {
                    let w = rd32((fifo + off + 48) as usize);
                    let w1 = rd32((fifo + off + 52) as usize);
                    let qtype = w & 0xff;
                    let txn = (w >> 8) & 0xffff;
                    let msgid = ((w >> 24) & 0xff) | ((w1 & 0xff) << 8);
                    // 对端 QRTR port：多客户端各自从 txn=1 起算，必须进去重键。
                    let sport = rd32((fifo + off + 28) as usize);
                    let skey = (sport << 16) | (txn & 0xffff);
                    let mut seen = false;
                    unsafe {
                        let mut i = 0u32;
                        while i < RF_SEEN_N {
                            if RF_SEEN[i as usize] == skey {
                                seen = true;
                                break;
                            }
                            i += 1;
                        }
                    }
                    if qtype == 0 && msgid >= 1 && msgid <= 5 && seen {
                        unsafe { RF_DROP |= 1 };
                    }
                    if qtype == 0 && msgid >= 1 && msgid <= 5 && !seen && !rf_queued(skey) {
                        let node = rd32((fifo + off + 24) as usize);
                        if !rf_enqueue(fifo, off, chunk, txn, node, sport, msgid) {
                            unsafe { RF_DROP |= 2 };
                        }
                    }
                }
                if typ == 1 && dst_node == 1 && dst_port == PM_QRTR_PORT && chunk >= 40 {
                    let w = rd32((fifo + off + 48) as usize);
                    let w1 = rd32((fifo + off + 52) as usize);
                    let qtype = w & 0xff;
                    let txn = (w >> 8) & 0xffff;
                    let msgid = ((w >> 24) & 0xff) | ((w1 & 0xff) << 8);
                    if qtype == 0 && unsafe { !PM_PEND } {
                        unsafe {
                            PM_HIT = PM_HIT.wrapping_add(1);
                            PM_TXN = txn;
                            PM_NODE = rd32((fifo + off + 24) as usize);
                            PM_PORT = rd32((fifo + off + 28) as usize);
                            PM_MSG = msgid;
                            PM_PEND = true;
                        }
                    }
                }
                // chunk：QRTR32+QMI；RESTART RESP 仅 result → chunk=48，旧门槛 56 会丢。
                if typ == 1 && dst_node == 1 && dst_port == SR_QRTR_PORT && chunk >= 48 {
                    let w = rd32((fifo + off + 48) as usize);
                    let w1 = rd32((fifo + off + 52) as usize);
                    let qtype = w & 0xff;
                    let msgid = ((w >> 24) & 0xff) | ((w1 & 0xff) << 8);
                    if qtype == 2 && msgid == 0x20 && unsafe { SR_PEND } {
                        let st = qmi_tlv_u32(fifo, off, chunk, 0x10).unwrap_or(0xffff_ffff);
                        let res = qmi_tlv_u32(fifo, off, chunk, 0x02).unwrap_or(0xffff_ffff);
                        unsafe {
                            SR_STATE = st;
                            SR_RES = res;
                            SR_PEND = false;
                            // 仅 DOWN 才 RESTART；UNINIT(0x7fffffff) 听 IND（对齐 Linux）。
                            if st == SR_STATE_DOWN && st != SR_STATE_UNINIT && SR_RP_N == 0 {
                                SR_NEED_RP = true;
                            }
                        }
                    }
                    if qtype == 2 && msgid == 0x24 && unsafe { SR_RP_PEND } {
                        let res = qmi_tlv_u32(fifo, off, chunk, 0x02).unwrap_or(0xffff_ffff);
                        unsafe {
                            SR_RP_ERR = res;
                            SR_RP_PEND = false;
                        }
                    }
                    // STATE_UPDATED IND：TLV01=state TLV03=txn → SET_ACK。
                    if qtype == 4 && msgid == 0x22 {
                        let st = qmi_tlv_u32(fifo, off, chunk, 0x01).unwrap_or(0xffff_ffff);
                        let tid = qmi_tlv_u16(fifo, off, chunk, 0x03).unwrap_or(0);
                        unsafe {
                            SR_STATE = st;
                            SR_IND_N = SR_IND_N.wrapping_add(1);
                            SR_ACK_TXN = tid;
                            SR_ACK_PEND = true;
                            if st == SR_STATE_DOWN && SR_RP_N == 0 {
                                SR_NEED_RP = true;
                            }
                        }
                    }
                }
                // IPA INIT_MODEM_DRIVER RESP（msgid 0x0021）→ ii=
                if typ == 1
                    && dst_node == 1
                    && (dst_port == IPA_QRTR_PORT || dst_port == IPA_SVC_PORT)
                    && chunk >= 48
                {
                    let w = rd32((fifo + off + 48) as usize);
                    let w1 = rd32((fifo + off + 52) as usize);
                    let qtype = w & 0xff;
                    let msgid = ((w >> 24) & 0xff) | ((w1 & 0xff) << 8);
                    if qtype == 2 && msgid == IPA_INIT_MSGID && unsafe { II_PEND } {
                        let res = qmi_tlv_u32(fifo, off, chunk, 0x02).unwrap_or(0xffff_ffff);
                        unsafe {
                            II_RES = res;
                            II_PEND = false;
                            II_ST = 2;
                        }
                    }
                    if qtype == 0 && dst_port == IPA_SVC_PORT {
                        unsafe {
                            if IPA_REQ == 0 {
                                IPA_REQ = msgid;
                            }
                            IPA_LAST = msgid;
                        }
                    }
                    if msgid != 0x27 && unsafe { JX_OTHER } == 0 {
                        unsafe {
                            JX_OTHER = (dst_port << 16) | ((qtype as u32) << 8) | msgid;
                        }
                    }
                    // INDICATION_REGISTER_REQ。内核回 TLV 0x02，INIT 已完成再发 0x22。
                    if qtype == 0 && msgid == 0x20 && !unsafe { IR_PEND } && unsafe { IR_DONE } == 0
                    {
                        unsafe {
                            IR_TXN = (w >> 8) & 0xffff;
                            IR_NODE = rd32((fifo + off + 24) as usize);
                            IR_PORT = rd32((fifo + off + 28) as usize);
                            IR_PEND = true;
                        }
                    }
                    // QMI_IPA_CONFIG_REQ。内核 ipa_mhi_handle_ipa_config_req 返回 0，
                    // 应答只有 TLV 0x02 成功。
                    if qtype == 0 && msgid == 0x27 && dst_port == IPA_SVC_PORT && !unsafe { CFG_PEND }
                    {
                        let txn = (w >> 8) & 0xffff;
                        let port = rd32((fifo + off + 28) as usize);
                        // 同一笔停在接收环里时不要每轮再回。
                        if unsafe { txn != CFG_DONE_TXN || port != CFG_DONE_PORT } {
                            unsafe {
                                CFG_TXN = txn;
                                CFG_NODE = rd32((fifo + off + 24) as usize);
                                CFG_PORT = port;
                                if CFG_CAP_N == 0 {
                                    let mut k = 0usize;
                                    while k < 4 {
                                        CFG_CAP[k] = rd32((fifo + off + 48 + (k as u64) * 4) as usize);
                                        k += 1;
                                    }
                                    CFG_CAP_N = 1;
                                }
                                // TLV 0x10 值 3 是 PCIE，不是 USB。第一次 sm 之后才回成功。
                                CFG_PEND = true;
                            }
                        }
                    }
                    // modem → AP：INIT_MODEM_DRIVER_CMPLT_REQ，msgid 0x35。
                    if qtype == 0 && msgid == 0x35 && !unsafe { IC_PEND } {
                        let txn = (w >> 8) & 0xffff;
                        unsafe {
                            IC_N = IC_N.wrapping_add(1);
                            IC_TXN = txn;
                            IC_NODE = rd32((fifo + off + 24) as usize);
                            IC_PORT = rd32((fifo + off + 28) as usize);
                            IC_PEND = true;
                        }
                    }
                }
                if typ == 1 && dst_node == 1 && dst_port == 0x11 && chunk >= 34 {
                    let op0 = ((rd8(fifo + off + 48) as u32) << 8) | (rd8(fifo + off + 49) as u32);
                    if op0 == 4 || op0 == 5 {
                        tf_note_ack(off, fifo);
                    }
                    // 刀 G3：地址刷新先于一切闸。客户端口逐笔换新（0x22→0x23 实测），
                    // pend 占用或 seen 命中也不能让回信地址陈旧——每见一笔发往
                    // 0x11 的请求（RRQ/WRQ/stat）就把回信地址与捕获时刻刷成最新。
                    // 陈旧地址投给已关 socket，modem 路由器静默丢：tt 消费、rx 恒零、
                    // tf 无限爬的合并根因。
                    if op0 != 4 && op0 != 5 {
                        unsafe {
                            let sn = rd32((fifo + off + 24) as usize);
                            let sp = rd32((fifo + off + 28) as usize);
                            if TF_RQ_NODE != sn || TF_RQ_PORT != sp {
                                TF_RQ2_NODE = TF_RQ_NODE;
                                TF_RQ2_PORT = TF_RQ_PORT;
                                TF_RQ_NODE = sn;
                                TF_RQ_PORT = sp;
                            }
                            TF_NODE = sn;
                            TF_PORT = sp;
                            TF_RQ_AT = time::cntvct();
                        }
                    }
                    let key = off as u32;
                    let mut seen = false;
                    unsafe {
                        let mut i = 0u32;
                        while i < TF_SEEN_N {
                            if TF_SEEN[i as usize] == key {
                                seen = true;
                                break;
                            }
                            i += 1;
                        }
                    }
                    if op0 != 4 && op0 != 5 && !seen && unsafe { !TF_PEND } {
                        let op = op0;
                        let mut name = [0u8; 64];
                        let mut nn = 0usize;
                        let max = chunk - 32;
                        while nn < 64 && (nn as u64) + 2 < max {
                            let b = rd8(fifo + off + 50 + nn as u64);
                            if b == 0 {
                                break;
                            }
                            name[nn] = b;
                            nn += 1;
                        }
                        unsafe {
                            TF_OP = op;
                            TF_OFF = key;
                            TF_NAME = name;
                            TF_NAME_N = nn as u8;
                            TF_PEND = true;
                            if op == 1 {
                                let (has, saw_rs, rsz, saw_sk, skv) = tf_rrq_rsize(fifo, off, chunk);
                                TF_HASOPT = if has { 1 } else { 0 };
                                TF_HAS_RS = if saw_rs { 1 } else { 0 };
                                TF_RSIZE = rsz;
                                TF_HAS_SK = if saw_sk { 1 } else { 0 };
                                TF_SEEK = skv;
                                tf_save_so(fifo, off, chunk);
                            }
                        }
                    }
                }
                if chunk >= 52 && (typ == 4 || typ == 10) {
                    let svc = rd32((fifo + off + 52) as usize);
                    let inst = rd32((fifo + off + 56) as usize);
                    if typ == 4 {
                        let n = unsafe { SMEM_NSVC };
                        if svc == 0x45 {
                            unsafe { SAW_45 = 0x8000_0000 | (inst & 0x7fff_ffff) };
                        }
                        if n < 8 {
                            unsafe {
                                SMEM_SVC[n as usize * 2] = svc;
                                SMEM_SVC[n as usize * 2 + 1] = inst;
                                SMEM_NSVC = n + 1;
                            }
                        }
                        // sm iq=：首见 QMI IPA svc 0x31 即闩；偏 inst 0x201 记 node:port。
                        if svc == 0x31 && chunk >= 52 {
                            let node = rd32((fifo + off + 60) as usize);
                            let port = rd32((fifo + off + 64) as usize);
                            unsafe {
                                if IQ_SEEN == 0 {
                                    IQ_SEEN = 1;
                                    IQ_INST = inst;
                                    if port != 0 {
                                        IQ_NODE = node;
                                        IQ_PORT = port;
                                    }
                                } else if inst == 0x201 && port != 0 {
                                    // 偏 stock qrtr 0x31/0x201；可覆盖首见非 0x201。
                                    IQ_INST = inst;
                                    IQ_NODE = node;
                                    IQ_PORT = port;
                                }
                            }
                        }
                        // modem SERVREG_NOTIF：记下 node:port，供 PDR REGISTER。
                        // modem QRTR node 常见为 0（AP 才是 1）；只拒 port==0。
                        if svc == SR_SVC && inst == SR_INST && chunk >= 52 {
                            let node = rd32((fifo + off + 60) as usize);
                            let port = rd32((fifo + off + 64) as usize);
                            unsafe {
                                if port != 0 && (!SR_HAVE || SR_PORT != port || SR_NODE != node) {
                                    SR_NODE = node;
                                    SR_PORT = port;
                                    SR_HAVE = true;
                                }
                            }
                        }
                    } else {
                        let n = unsafe { SMEM_NLOOK };
                        if n < 8 {
                            unsafe {
                                SMEM_LOOK[n as usize * 2] = svc;
                                SMEM_LOOK[n as usize * 2 + 1] = inst;
                                SMEM_NLOOK = n + 1;
                            }
                        }
                        // qrtr ns：本机有的服务立刻排队回 NEW_SERVER。
                        if host_svc_port(svc, inst).is_some() && unsafe { LOOK_PEND } == 0 {
                            unsafe {
                                LOOK_PEND = svc | (inst << 16);
                                LOOK_N = LOOK_N.wrapping_add(1);
                            }
                        }
                    }
                }
            }
            step
        } else {
            unsafe {
                SMEM_QSTOP = off as u32;
                SMEM_QCMD = cmd;
            }
            break
        };
        if step < 8 || off + step <= off {
            break;
        }
        off += step;
    }
    unsafe {
        RX_PARSED = off;
    }
}

/// 对方的发送 FIFO 就是这块。head 是它写到的位置，tail 是我们消费到的位置。
/// qcom_glink_smem.c 的 tx_avail = length - head + tail，再留 16 字节。
/// tail 停在 0 时，head 顶到长度就写不进去。公告发出后，把 tail 推到已解析处。
fn glink_rx_advance(at478: u64, sz480: u64) {
    if at478 == 0 || unsafe { HOST_ADV } < 4 || sz480 == 0 {
        return;
    }
    let head = rd32((at478 + 12) as usize) as u64;
    let tail = rd32((at478 + 8) as usize) as u64;
    let parsed = unsafe { RX_PARSED };
    if parsed <= tail || parsed > head || parsed > sz480 {
        return;
    }
    wr32((at478 + 8) as usize, parsed as u32);
    if at478 < 0x80A0_0000 {
        dcache_civac(at478, 16);
    }
}

struct FwSeg {
    cl: u16,
    sz: u32,
    pa: u32,
}

/// image/ 里 filesz>0 的可装载段。FAT16 簇号，簇 16KB，链在分区里是连续的。
const FW_SEGS: &[FwSeg] = &[
    FwSeg { cl: 6, sz: 10544, pa: 0x8c00_0000 },
    FwSeg { cl: 7, sz: 8776, pa: 0x8c00_3000 },
    FwSeg { cl: 8, sz: 226240, pa: 0x8c10_0000 },
    FwSeg { cl: 22, sz: 180448, pa: 0x8c18_0000 },
    FwSeg { cl: 34, sz: 207800, pa: 0x8c1a_d000 },
    FwSeg { cl: 47, sz: 55080, pa: 0x8c1e_0000 },
    FwSeg { cl: 51, sz: 132360, pa: 0x8c1e_e000 },
    FwSeg { cl: 60, sz: 409684, pa: 0x8c20_f000 },
    FwSeg { cl: 86, sz: 3117084, pa: 0x8c28_0000 },
    FwSeg { cl: 277, sz: 57536, pa: 0x8c58_0000 },
    FwSeg { cl: 281, sz: 45, pa: 0x8c58_f000 },
    FwSeg { cl: 282, sz: 20480, pa: 0x8c59_0000 },
    FwSeg { cl: 284, sz: 2531824, pa: 0x8c59_5000 },
    FwSeg { cl: 439, sz: 327680, pa: 0x8c80_0000 },
    FwSeg { cl: 459, sz: 4014080, pa: 0x8c88_0000 },
    FwSeg { cl: 705, sz: 1159984, pa: 0x8cc5_5000 },
    FwSeg { cl: 776, sz: 650160, pa: 0x8cd8_0000 },
    FwSeg { cl: 816, sz: 1597440, pa: 0x8ce4_0000 },
    FwSeg { cl: 914, sz: 8007496, pa: 0x8cfc_6000 },
    FwSeg { cl: 1403, sz: 10306108, pa: 0x8e62_3000 },
    FwSeg { cl: 2033, sz: 25814821, pa: 0x9325_c000 },
    FwSeg { cl: 3610, sz: 100672, pa: 0x9909_8000 },
    FwSeg { cl: 3617, sz: 38723648, pa: 0x9910_0000 },
    FwSeg { cl: 5981, sz: 1110968, pa: 0x9b60_0000 },
    FwSeg { cl: 6049, sz: 65536, pa: 0x9b71_0000 },
    FwSeg { cl: 6053, sz: 465356, pa: 0x9b72_0000 },
    FwSeg { cl: 6082, sz: 77348, pa: 0x9b80_0000 },
    FwSeg { cl: 6087, sz: 4984, pa: 0x9b81_3000 },
    FwSeg { cl: 6088, sz: 176, pa: 0x9b81_d000 },
    FwSeg { cl: 6089, sz: 268, pa: 0x9b81_e000 },
];

const MODEM_LBA: u32 = 115208;

static mut MODEM_FAT: [u8; 24576] = [0; 24576];

fn fat_next(cl: u32) -> u16 {
    let off = (cl as usize) * 2;
    let fat = unsafe { &MODEM_FAT };
    u16::from_le_bytes([fat[off], fat[off + 1]])
}

fn load_modem_segs(ufs: &mut Ufs) -> Result<u32, u16> {
    for i in 0..6u32 {
        ufs.read10(MODEM_LBA + 1 + i, 1, DATA).map_err(|_| 0u16)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                DATA as *const u8,
                MODEM_FAT.as_mut_ptr().add(i as usize * 4096),
                4096,
            );
        }
    }
    unsafe {
        core::ptr::write_bytes(0x8c00_0000 as *mut u8, 0, 0x1180_0000);
    }
    for seg in FW_SEGS {
        let mut left = seg.sz as usize;
        let mut dst = seg.pa as u64;
        let mut cl = seg.cl as u32;
        while left > 0 {
            if cl >= 0xFFF8 {
                return Err(seg.cl);
            }
            let lba = MODEM_LBA + 17 + (cl - 2) * 4;
            for s in 0..4u32 {
                if left == 0 {
                    break;
                }
                ufs.read10(lba + s, 1, DATA).map_err(|_| seg.cl)?;
                let n = left.min(4096);
                unsafe {
                    core::ptr::copy_nonoverlapping(DATA as *const u8, dst as *mut u8, n);
                }
                dst += n as u64;
                left -= n;
            }
            if left == 0 {
                break;
            }
            cl = fat_next(cl) as u32;
        }
    }
    Ok(FW_SEGS.len() as u32)
}

fn bat_f(arb: &spmi::Arb) -> Option<u64> {
    let qg = unsafe { SOAK_QG };
    let chg = unsafe { SOAK_CHG };
    if qg == 0 || chg == 0 {
        return None;
    }
    let mut b = [0u8; 1];
    let mut rd = |ap: u32, off: u8| -> Option<u8> {
        arb.read(ap, off, &mut b).ok()?;
        Some(b[0])
    };
    let v = (rd(qg, 0xC0)? as u64) | (rd(qg, 0xC1)? as u64) << 8;
    let i = (rd(qg, 0xC2)? as u64) | (rd(qg, 0xC3)? as u64) << 8;
    let soc = rd(qg, 0xBF)? as u64;
    let c6 = rd(chg, 0x06)? as u64;
    let c42 = rd(chg, 0x42)? as u64;
    Some((v << 48) | (i << 32) | (soc << 24) | (c6 << 16) | (c42 << 8) | 0x17)
}

fn ctrl_line_is(want: &[u8]) -> bool {
    let n = unsafe { CTRL_LINE_N };
    n == want.len() && unsafe { &CTRL_LINE[..n] } == want
}

fn ctrl_reply(sock: &mut smoltcp::socket::tcp::Socket, body: &[u8]) -> bool {
    sock.send_slice(body).is_ok()
}

fn ctrl_service(
    ufs: &mut Ufs,
    freq: u32,
    t: &mut [u8; 96],
    arb: &spmi::Arb,
    n_apid: u32,
    sockets: &mut smoltcp::iface::SocketSet<'_>,
) -> bool {
    use smoltcp::socket::tcp::{Socket, State};

    let handle = match unsafe { CTRL_HANDLE } {
        Some(handle) => handle,
        None => return false,
    };
    let sock = sockets.get_mut::<Socket>(handle);
    let st = tcp_state_code(sock.state());
    if st != unsafe { TCP_ST } {
        unsafe { TCP_ST = st };
        tcp_mark(ufs, freq, t, " tcp st=0x", st as u64);
    }
    // 只有一个套接字。半开握手不完成时，后来的 SYN 对不上四元组，smoltcp 直接 RST。
    if sock.state() == State::SynReceived {
        let now = time::cntvct();
        let at = unsafe { TCP_SYN_AT };
        if at == 0 {
            unsafe { TCP_SYN_AT = now };
        } else if freq != 0 && now.wrapping_sub(at) > freq as u64 {
            sock.abort();
            let _ = sock.listen(CTRL_PORT);
            unsafe { TCP_SYN_AT = 0 };
            tcp_mark(ufs, freq, t, " tcp re=0x", 1);
        }
    } else {
        unsafe { TCP_SYN_AT = 0 };
    }
    match sock.state() {
        State::Established => {
            if unsafe { !CTRL_GREETED } && sock.may_send() {
                if ctrl_reply(sock, b"aginx\n") {
                    unsafe { CTRL_GREETED = true };
                    s20_keep(ufs, freq, t, " s20 tcp=0x", 1);
                    return true;
                }
                return false;
            }
            let mut tmp = [0u8; 32];
            let mut queued = false;
            loop {
                let n = match sock.recv_slice(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                for &b in &tmp[..n] {
                    if b == b'\n' || b == b'\r' {
                        let code = if unsafe { CTRL_OVERFLOW } {
                            0
                        } else if ctrl_line_is(b"id") {
                            1
                        } else if ctrl_line_is(b"ping") {
                            2
                        } else if ctrl_line_is(b"up") {
                            4
                        } else if ctrl_line_is(b"fb") {
                            5
                        } else if ctrl_line_is(b"log") {
                            6
                        } else if ctrl_line_is(b"bat") {
                            7
                        } else if ctrl_line_is(b"sm") {
                            8
                        } else if ctrl_line_is(b"pv") {
                            9
                        } else if ctrl_line_is(b"px") {
                            10
                        } else if ctrl_line_is(b"qm") {
                            11
                        } else if ctrl_line_is(b"qn") {
                            12
                        } else if ctrl_line_is(b"qc") {
                            13
                        } else if ctrl_line_is(b"qs") {
                            14
                        } else if ctrl_line_is(b"fr") {
                            15
                        } else if ctrl_line_is(b"tx") {
                            16
                        } else if unsafe { CTRL_LINE_N } == 0 {
                            3
                        } else {
                            0
                        };
                        unsafe {
                            CTRL_LINE_N = 0;
                            CTRL_OVERFLOW = false;
                        }
                        if code == 6 {
                            let n = ring_tail(ufs, unsafe { &mut LOG_OUT });
                            let sent = if n > 0 {
                                ctrl_reply(sock, unsafe { &LOG_OUT[..n] })
                            } else {
                                ctrl_reply(sock, b"log\n")
                            };
                            if sent {
                                s20_keep(ufs, freq, t, " s20 cmd=0x", 6);
                                queued = true;
                            }
                            continue;
                        }
                        // 刀 G10：+rp=/rr= 转录字段（~330 字符）——1792 顶满会截尾，扩 2560。
                        let mut reply = [0u8; 2560];
                        let mut rn = 0usize;
                        match code {
                            1 => push_str(&mut reply, &mut rn, "redfin\n"),
                            2 => push_str(&mut reply, &mut rn, "pong\n"),
                            4 => {
                                push_str(&mut reply, &mut rn, "up ");
                                let secs = if freq == 0 {
                                    0
                                } else {
                                    time::cntvct() / freq as u64
                                };
                                push_u64(&mut reply, &mut rn, secs);
                                push_str(&mut reply, &mut rn, " seq ");
                                push_u64(&mut reply, &mut rn, unsafe { RING.seq });
                                push_str(&mut reply, &mut rn, "\n");
                            }
                            5 => match fb_check(arb, n_apid) {
                                Ok(()) => {
                                    if unsafe { CTRL_FB } == 0 {
                                        unsafe { CTRL_FB = 1 };
                                    }
                                    push_str(&mut reply, &mut rn, "fb\n");
                                }
                                Err(err) => {
                                    push_str(&mut reply, &mut rn, "fb ");
                                    push_hex(&mut reply, &mut rn, err);
                                    push_str(&mut reply, &mut rn, "\n");
                                    s20_keep(ufs, freq, t, " s20 fb=0x", err);
                                }
                            },
                            7 => match bat_f(arb) {
                                Some(f) => {
                                    push_str(&mut reply, &mut rn, "bat f=");
                                    push_hex(&mut reply, &mut rn, f);
                                    push_str(&mut reply, &mut rn, "\n");
                                }
                                None => push_str(&mut reply, &mut rn, "bat\n"),
                            },
                            8 => {
                                smem_scan(ufs, true);
                                unsafe { CFG_ARM = 1; }
                                let s = unsafe { &SMEM_SNAP };
                                push_str(&mut reply, &mut rn, "sm i=");
                                push_hex(&mut reply, &mut rn, s.init as u64);
                                push_str(&mut reply, &mut rn, " v=");
                                push_hex(&mut reply, &mut rn, s.ver as u64);
                                push_str(&mut reply, &mut rn, " n=");
                                push_hex(&mut reply, &mut rn, s.nent as u64);
                                push_str(&mut reply, &mut rn, " p=");
                                push_hex(&mut reply, &mut rn, s.part as u64);
                                push_str(&mut reply, &mut rn, " g=");
                                push_hex(&mut reply, &mut rn, s.g as u64);
                                push_str(&mut reply, &mut rn, " u=");
                                push_hex(&mut reply, &mut rn, s.free as u64);
                                push_str(&mut reply, &mut rn, " s=");
                                push_hex(&mut reply, &mut rn, s.slave as u64);
                                push_str(&mut reply, &mut rn, " m=");
                                push_hex(&mut reply, &mut rn, s.smpm as u64);
                                push_str(&mut reply, &mut rn, " d=");
                                push_hex(&mut reply, &mut rn, s.d0 as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, s.d1 as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, s.d2 as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, s.d3 as u64);
                                push_str(&mut reply, &mut rn, " r=");
                                push_hex(&mut reply, &mut rn, s.rx0 as u64);
                                push_str(&mut reply, &mut rn, " rk=");
                                push_hex(&mut reply, &mut rn, unsafe { RX_DONE_N } as u64);
                                push_str(&mut reply, &mut rn, " cf=");
                                push_hex(&mut reply, &mut rn, unsafe { CF_N } as u64);
                                push_str(&mut reply, &mut rn, " hh=");
                                push_hex(&mut reply, &mut rn, unsafe { HELLO_RX } as u64);
                                push_str(&mut reply, &mut rn, " iz=");
                                push_hex(&mut reply, &mut rn, unsafe { SEEN_ISZ } as u64);
                                push_str(&mut reply, &mut rn, " ga=");
                                push_hex(&mut reply, &mut rn, if unsafe { GLINK_ACKED } { 1 } else { 0 });
                                push_str(&mut reply, &mut rn, " rl=");
                                push_hex(&mut reply, &mut rn, unsafe { GLINK_RXLEN });
                                push_str(&mut reply, &mut rn, " b=");
                                push_hex(&mut reply, &mut rn, s.boff as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, s.b0 as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, s.b1 as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, s.blast as u64);
                                push_str(&mut reply, &mut rn, " k=");
                                push_hex(&mut reply, &mut rn, s.k as u64);
                                push_str(&mut reply, &mut rn, " c=");
                                push_hex(&mut reply, &mut rn, s.cached as u64);
                                push_str(&mut reply, &mut rn, " gk=");
                                push_hex(&mut reply, &mut rn, s.gk as u64);
                                push_str(&mut reply, &mut rn, " e=");
                                push_hex(&mut reply, &mut rn, s.ev as u64);
                                push_str(&mut reply, &mut rn, " n0=");
                                push_hex(&mut reply, &mut rn, s.n0 as u64);
                                push_str(&mut reply, &mut rn, " a=");
                                for i in 0..5 {
                                    if i != 0 {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    push_hex(&mut reply, &mut rn, unsafe { SMEM_ENTS[i] } as u64);
                                }
                                push_str(&mut reply, &mut rn, " f=");
                                for i in 5..10 {
                                    if i != 5 {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    push_hex(&mut reply, &mut rn, unsafe { SMEM_ENTS[i] } as u64);
                                }
                                push_str(&mut reply, &mut rn, " qs=");
                                push_hex(&mut reply, &mut rn, unsafe { SMEM_QSTOP } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { SMEM_QCMD } as u64);
                                push_str(&mut reply, &mut rn, " pv=");
                                for i in 0..9 {
                                    if i != 0 {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    push_hex(&mut reply, &mut rn, unsafe { PROXY[i] } as u64);
                                }
                                push_str(&mut reply, &mut rn, " pa=");
                                push_hex(&mut reply, &mut rn, unsafe { PROXY_AUX[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { PROXY_AUX[1] } as u64);
                                push_str(&mut reply, &mut rn, " qm=");
                                for i in 0..8 {
                                    if i != 0 {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    push_hex(&mut reply, &mut rn, unsafe { QMPW[i] } as u64);
                                }
                                push_str(&mut reply, &mut rn, " pr=");
                                push_hex(&mut reply, &mut rn, unsafe { PD_N } as u64);
                                push_str(&mut reply, &mut rn, " pn=");
                                let nn = unsafe { PD_NAME_N } as usize;
                                let mut pi = 0usize;
                                while pi < nn && pi < 64 {
                                    let b = unsafe { PD_NAME[pi] };
                                    if (0x21..0x7f).contains(&b) && rn < reply.len() {
                                        reply[rn] = b;
                                        rn += 1;
                                    }
                                    pi += 1;
                                }
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { PD_HITS } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { PD_QLEN } as u64);
                                push_str(&mut reply, &mut rn, " pe=");
                                push_hex(&mut reply, &mut rn, unsafe { PD_SAW_ELF } as u64);
                                push_str(&mut reply, &mut rn, " rf=");
                                push_hex(&mut reply, &mut rn, unsafe { RF_MSG } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                let rfn = unsafe { RF_PATH_N } as usize;
                                let mut ri = 0usize;
                                while ri < rfn && ri < 64 {
                                    let b = unsafe { RF_PATH[ri] };
                                    if (0x21..0x7f).contains(&b) && rn < reply.len() {
                                        reply[rn] = b;
                                        rn += 1;
                                    }
                                    ri += 1;
                                }
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_ID } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_N } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_SZ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_AS } as u64);
                                push_str(&mut reply, &mut rn, " rh=");
                                push_hex(&mut reply, &mut rn, unsafe { RF_HIT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_DROP } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_HIST[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_HIST[1] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_HIST[2] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_HIST[3] } as u64);
                                push_str(&mut reply, &mut rn, " ik=");
                                push_hex(&mut reply, &mut rn, unsafe { RF_IOK } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { RF_IFAIL } as u64);
                                push_str(&mut reply, &mut rn, " rs=");
                                {
                                    let rn0 = unsafe { RF_RSP_N } as usize;
                                    let mut i = 0usize;
                                    while i < rn0 && i < 8 {
                                        let b = unsafe { RF_RSP[i] };
                                        let hi = b >> 4;
                                        let lo = b & 0xf;
                                        if rn + 2 <= reply.len() {
                                            reply[rn] = if hi < 10 { b'0' + hi } else { b'a' + (hi - 10) };
                                            reply[rn + 1] = if lo < 10 { b'0' + lo } else { b'a' + (lo - 10) };
                                            rn += 2;
                                        }
                                        i += 1;
                                    }
                                }
                                push_str(&mut reply, &mut rn, " pm=");
                                push_hex(&mut reply, &mut rn, unsafe { PM_HIT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { PM_N } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { PM_MSG } as u64);
                                push_str(&mut reply, &mut rn, " sr=");
                                push_hex(&mut reply, &mut rn, unsafe { SR_HIT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { SR_STATE } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { SR_RES } as u64);
                                push_str(&mut reply, &mut rn, " si=");
                                push_hex(&mut reply, &mut rn, unsafe { SR_IND_N } as u64);
                                push_str(&mut reply, &mut rn, " sp=");
                                if unsafe { SR_HAVE } {
                                    push_hex(&mut reply, &mut rn, unsafe { SR_NODE } as u64);
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(&mut reply, &mut rn, unsafe { SR_PORT } as u64);
                                } else {
                                    push_str(&mut reply, &mut rn, "ffffffff");
                                }
                                push_str(&mut reply, &mut rn, " rp=");
                                push_hex(&mut reply, &mut rn, unsafe { SR_RP_N } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { SR_RP_ERR } as u64);
                                push_str(&mut reply, &mut rn, " lk=");
                                push_hex(&mut reply, &mut rn, unsafe { LOOK_N } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { LOOK_PEND } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { HOST_ADV } as u64);
                                push_str(&mut reply, &mut rn, " qp=");
                                push_hex(&mut reply, &mut rn, unsafe { QP_HIST_N } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { QP_HIST[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { QP_HIST[1] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { QP_HIST[2] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { QP_HIST[3] } as u64);
                                push_str(&mut reply, &mut rn, " es=");
                                push_hex(&mut reply, &mut rn, unsafe { RFS_SEED } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GPT_ENTRIES } as u64);
                                push_str(&mut reply, &mut rn, " gn=");
                                let gnn = unsafe { RFS_GNN } as usize;
                                let mut gi = 0usize;
                                while gi < gnn && gi < 64 {
                                    let b = unsafe { RFS_GN[gi] };
                                    if (0x21..0x7f).contains(&b) && rn < reply.len() {
                                        reply[rn] = b;
                                        rn += 1;
                                    }
                                    gi += 1;
                                }
                                push_str(&mut reply, &mut rn, " tf=");
                                let tfn = unsafe { TF_NAME_N } as usize;
                                let mut ti = 0usize;
                                while ti < tfn && ti < 64 {
                                    let b = unsafe { TF_NAME[ti] };
                                    if (0x21..0x7f).contains(&b) && rn < reply.len() {
                                        reply[rn] = b;
                                        rn += 1;
                                    }
                                    ti += 1;
                                }
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_OP } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_N } as u64);
                                push_str(&mut reply, &mut rn, " tz=");
                                push_hex(&mut reply, &mut rn, unsafe { TZ_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TZ_SZ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TZ_CL } as u64);
                                // tb=st.blk — 0 空闲 1 OACK 2 DATA 3 送完 8 读失败 9 对端 ERROR
                                push_str(&mut reply, &mut rn, " tb=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_BLKNO } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_XFER_PORT } as u64);
                                push_str(&mut reply, &mut rn, " rx=");
                                // 滚动备忘：总数.随后最多 4 槽 port.op.arg（时间序，旧→新；
                                // arg=op5 错误码/op4 块号，刀 G6）
                                push_hex(&mut reply, &mut rn, unsafe { TF_RX_LOG_N } as u64);
                                let n_log = unsafe { TF_RX_LOG_N };
                                let mut k_log = n_log.saturating_sub(4);
                                while k_log < n_log {
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(
                                        &mut reply,
                                        &mut rn,
                                        unsafe { TF_RX_LOG_P[(k_log & 3) as usize] } as u64,
                                    );
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(
                                        &mut reply,
                                        &mut rn,
                                        unsafe { TF_RX_LOG_O[(k_log & 3) as usize] } as u64,
                                    );
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(
                                        &mut reply,
                                        &mut rn,
                                        unsafe { TF_RX_LOG_A[(k_log & 3) as usize] } as u64,
                                    );
                                    k_log += 1;
                                }
                                // 刀 G8：dv=进入.读零.tx拒.环闸挡——DATA 路径堵点
                                // 实测（G7 立即 DATA 零上环的定位插桩）。
                                push_str(&mut reply, &mut rn, " dv=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_DV } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_DZ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_DX } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_DC } as u64);
                                // 刀 G9c：dv 第五元=retry OACK 服务数。
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_DR } as u64);
                                // 刀 G12：pq=call.noarm.nost.nopend（tf_reply
                                // 四分位，宽限 synth 后计）＋ ox=ok.no（OACK TX
                                // 成败）——dv=0 矛盾的实测堵点。
                                push_str(&mut reply, &mut rn, " pq=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_PQ_CALL } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_PQ_NOARM } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_PQ_NOST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_PQ_NOPEND } as u64);
                                push_str(&mut reply, &mut rn, " ox=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_PQ_TXOK } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_PQ_TXNO } as u64);
                                // ob=blk.wsz.ts — 写进 OACK 的数。wsz=0 表示请求没带 wsize。
                                // rs=有键.值。rs=1.0 是 stat。
                                push_str(&mut reply, &mut rn, " ob=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_REQ_BLK } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_REQ_WSZ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_HAS_TS } as u64);
                                push_str(&mut reply, &mut rn, " rs=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_HAS_RS } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_RSIZE } as u64);
                                push_str(&mut reply, &mut rn, " tr=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_REJ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_REJ_PORT } as u64);
                                // tt=TX环 head.tail.环容 —— tail 推进=modem 消费（送达 glink 层）
                                push_str(&mut reply, &mut rn, " tt=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_TXH } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_TXT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_TXSZ } as u64);
                                // 刀 G2：rq=本笔 RRQ 客户端 node.port，rq2=上一笔（port 换新
                                // =客户端重开 socket）；cp=CTRL 口副本探针已发与否。
                                push_str(&mut reply, &mut rn, " rq=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_RQ_NODE } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_RQ_PORT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_RQ2_NODE } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_RQ2_PORT } as u64);
                                push_str(&mut reply, &mut rn, " cp=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_CTRL_PROBE } as u64);
                                // 刀 G3：rd=OACK 发出距最新 RRQ 捕获的毫秒（对赌客户端
                                // ~1s 的 socket 存活窗）；tm=RRQ 请求的 timeoutms。
                                push_str(&mut reply, &mut rn, " rd=");
                                let freq3 = time::freq_or(FREQ_FALLBACK as u32) as u64;
                                let d3 = if freq3 > 0
                                    && unsafe { TF_RQ_AT } != 0
                                    && unsafe { TF_OACK_AT } > unsafe { TF_RQ_AT }
                                {
                                    (unsafe { TF_OACK_AT } - unsafe { TF_RQ_AT }) * 1000 / freq3
                                } else {
                                    0
                                };
                                push_hex(&mut reply, &mut rn, d3);
                                push_str(&mut reply, &mut rn, " tm=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_REQ_TMO } as u64);
                                // 刀 G5：rl=全口请求日志（6 槽，旧→新）。槽=dst口.源口.op.
                                // fl.rsize.seek；fl bit0=rsize 键在、bit1=seek 键在。
                                // 补盲洞：此前请求只认 0x11，换口请求整段不可见。
                                push_str(&mut reply, &mut rn, " rl=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_RL_N } as u64);
                                {
                                    let total = unsafe { TF_RL_N } as usize;
                                    let shown = total.min(6);
                                    let start = total.saturating_sub(6);
                                    let mut k = 0usize;
                                    while k < shown {
                                        let idx = (start + k) % 6;
                                        if k != 0 {
                                            push_str(&mut reply, &mut rn, ",");
                                        }
                                        push_hex(&mut reply, &mut rn, unsafe { TF_RL_P[idx] } as u64);
                                        push_str(&mut reply, &mut rn, ".");
                                        push_hex(&mut reply, &mut rn, unsafe { TF_RL_S[idx] } as u64);
                                        push_str(&mut reply, &mut rn, ".");
                                        push_hex(&mut reply, &mut rn, unsafe { TF_RL_O[idx] } as u64);
                                        push_str(&mut reply, &mut rn, ".");
                                        push_hex(&mut reply, &mut rn, unsafe { TF_RL_F[idx] } as u64);
                                        push_str(&mut reply, &mut rn, ".");
                                        push_hex(&mut reply, &mut rn, unsafe { TF_RL_R[idx] } as u64);
                                        push_str(&mut reply, &mut rn, ".");
                                        push_hex(&mut reply, &mut rn, unsafe { TF_RL_K[idx] } as u64);
                                        k += 1;
                                    }
                                    if shown == 0 {
                                        push_str(&mut reply, &mut rn, "-");
                                    }
                                }
                                // me=pub.vout.inflags — MPSS SMP2P ipa 刀
                                push_str(&mut reply, &mut rn, " me=");
                                push_hex(&mut reply, &mut rn, unsafe { MPSS_PUB } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { MPSS_VOUT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { MPSS_IN } as u64);
                                push_str(&mut reply, &mut rn, " mv=");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_SMP_SENT } as u64);
                                // bc=NAME.st.addr.unit.width,... — IPA BCM cmd-db 只读（序：IP0 SH0 MC0 ACV SN1 CN0 SH4）
                                bcm_probe();
                                push_str(&mut reply, &mut rn, " bc=");
                                {
                                    let mut bi = 0usize;
                                    while bi < 7 {
                                        if bi != 0 {
                                            push_str(&mut reply, &mut rn, ",");
                                        }
                                        push_str(&mut reply, &mut rn, BCM_TAGS[bi]);
                                        push_str(&mut reply, &mut rn, ".");
                                        push_hex(&mut reply, &mut rn, unsafe { BCM_ST[bi] } as u64);
                                        push_str(&mut reply, &mut rn, ".");
                                        push_hex(&mut reply, &mut rn, unsafe { BCM_ADDR[bi] } as u64);
                                        push_str(&mut reply, &mut rn, ".");
                                        push_hex(&mut reply, &mut rn, unsafe { BCM_UNIT[bi] } as u64);
                                        push_str(&mut reply, &mut rn, ".");
                                        push_hex(&mut reply, &mut rn, unsafe { BCM_WIDTH[bi] } as u64);
                                        bi += 1;
                                    }
                                }
                                // I4：STE + ACTIVE votes（一次）；sm ste=/bv=/ip=
                                ipa_i4_once(freq);
                                push_str(&mut reply, &mut rn, " ste=");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_STE_MASK } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_STE[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_STE[1] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_STE[2] } as u64);
                                push_str(&mut reply, &mut rn, " bv=");
                                push_hex(&mut reply, &mut rn, unsafe { BCM_BV } as u64);
                                // ip=st.p0.p1.p2 — IPA fws PAS（ADSP 后；未跑则 st=0 p=ffffffff）
                                push_str(&mut reply, &mut rn, " ip=");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_P[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_P[1] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_P[2] } as u64);
                                // cd=st.p0.p1.p2 — CDSP PAS（仅 IPA+ADSP 绿后；未跑则 st=0 p=ffffffff）
                                push_str(&mut reply, &mut rn, " cd=");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_P[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_P[1] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_P[2] } as u64);
                                push_str(&mut reply, &mut rn, " cb=");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_CB } as u64);
                                // ce=pub.rdy.wt — CDSP ONLINE（bit1）；sm 时刷新 rdy
                                {
                                    let (_c, v, _p) = cdsp_edge(false);
                                    if v & 0x8000_0000 == 0 {
                                        unsafe { CDSP_RDY = v };
                                    }
                                }
                                push_str(&mut reply, &mut rn, " ce=");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_PUB } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_RDY } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_WT } as u64);
                                push_str(&mut reply, &mut rn, " cz=");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_Z } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { CDSP_ZB } as u64);
                                push_str(&mut reply, &mut rn, " ad=");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_P[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_P[1] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_P[2] } as u64);
                                {
                                    let (_c, v, _p) = lpass_edge(false);
                                    if v & 0x8000_0000 == 0 {
                                        unsafe { ADSP_SLV = v };
                                    }
                                    if unsafe { SM_ARMED } {
                                        if v & 4 != 0 {
                                            adsp_proxy_unvote(freq);
                                        }
                                    } else {
                                        unsafe { SM_ARMED = true };
                                    }
                                }
                                push_str(&mut reply, &mut rn, " au=");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_UV } as u64);
                                push_str(&mut reply, &mut rn, " ae=");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_PUB } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_SLV } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_WT } as u64);
                                push_str(&mut reply, &mut rn, " ap=");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_PRE } as u64);
                                push_str(&mut reply, &mut rn, " ft=");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_FT } as u64);
                                push_str(&mut reply, &mut rn, " az=");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_Z } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_ZB } as u64);
                                unsafe { ADSP_SFR = sfr_load(freq, 423) };
                                push_str(&mut reply, &mut rn, " ah=");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_IN[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_IN[1] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_IN[2] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_ACK } as u64);
                                push_str(&mut reply, &mut rn, " lh=");
                                push_hex(&mut reply, &mut rn, unsafe { LG_HN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { LG_HELLO } as u64);
                                push_str(&mut reply, &mut rn, " fi=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_IN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_ILOG[2] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_ILOG[3] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_REQ } as u64);
                                push_str(&mut reply, &mut rn, " fs=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_SENT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_SMR } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RCID } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_ISZ } as u64);
                                push_str(&mut reply, &mut rn, " fy=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_PREV[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_PREV[1] } as u64);
                                push_str(&mut reply, &mut rn, " fo=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_SENT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_REUSE } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RDONE } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RLIID } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_OPN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                if unsafe { FP_OPN } != 0 && unsafe { FP_SENT } == 4 {
                                    let page = core::ptr::addr_of!(FP_PAGE) as u64;
                                    dcache_ivac(page + 896, 64);
                                    unsafe {
                                        FP_HDL = rd32((page + 896) as usize);
                                        FP_DL = rd32((page + 900) as usize);
                                    }
                                }
                                push_hex(&mut reply, &mut rn, unsafe { FP_HDL } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_DL } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_OPEN[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_OPEN[2] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_REGN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_REG[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_REG[2] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                if unsafe { FP_SENT } == 8 && unsafe { FP_CLSN } != 0 && unsafe { FP_CDL } == 0xffff_ffff
                                {
                                    let page = core::ptr::addr_of!(FP_PAGE) as u64;
                                    dcache_ivac(page + 640, 64);
                                    unsafe { FP_CDL = rd32((page + 640) as usize) };
                                }
                                push_hex(&mut reply, &mut rn, unsafe { FP_CLSN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_CLS[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_CLS[2] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_CDL } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_ININ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_INI[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_INI[2] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_NXN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_NX[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_NX[2] } as u64);
                                if unsafe { FP_NXN } != 0 && unsafe { FP_SENT } == 10 {
                                    let page = core::ptr::addr_of!(FP_PAGE) as u64;
                                    dcache_ivac(page + 768, 128);
                                    let mut k = 0usize;
                                    while k < 4 {
                                        unsafe {
                                            FP_NOUT[k] = rd32((page + 768 + k as u64 * 4) as usize);
                                        }
                                        k += 1;
                                    }
                                }
                                let mut k = 0usize;
                                while k < 4 {
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(&mut reply, &mut rn, unsafe { FP_NOUT[k] } as u64);
                                    k += 1;
                                }
                                push_str(&mut reply, &mut rn, " rv=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_REV } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RVN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RVH } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RVSC } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RVRES } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { AST_METHOD } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                let rvn = unsafe { FP_RVNAMEN } as usize;
                                let mut ri = 0usize;
                                while ri < rvn && ri < 32 {
                                    let b = unsafe { FP_RVNAME[ri] };
                                    if (0x21..0x7f).contains(&b) && rn < reply.len() {
                                        reply[rn] = b;
                                        rn += 1;
                                    }
                                    ri += 1;
                                }
                                push_str(&mut reply, &mut rn, " ri=");
                                let rvin_n = unsafe { FP_RVINN }.min(29) as usize;
                                let mut ri = 0usize;
                                while ri < rvin_n {
                                    push_hex(&mut reply, &mut rn, unsafe { FP_RVIN[ri] } as u64);
                                    if ri + 1 < rvin_n {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    ri += 1;
                                }
                                push_str(&mut reply, &mut rn, " gq=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_CH } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_CT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_SH } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_DN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RDP } as u64);
                                push_str(&mut reply, &mut rn, " fp=");
                                push_hex(&mut reply, &mut rn, unsafe { LG_FPN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RXP } as u64);
                                let mut fi = 0usize;
                                while fi < 4 {
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(&mut reply, &mut rn, unsafe { LG_FP[fi] } as u64);
                                    fi += 1;
                                }
                                push_str(&mut reply, &mut rn, " lg=");
                                push_hex(&mut reply, &mut rn, unsafe { LG_BITS } as u64);
                                push_str(&mut reply, &mut rn, " ag=");
                                push_hex(&mut reply, &mut rn, unsafe { LG_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { LG_PHASE } as u64);
                                push_str(&mut reply, &mut rn, " af=");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_SFR.st } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { ADSP_SFR.sz } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                let mut ai = 0usize;
                                while ai < 32 {
                                    let w = unsafe { ADSP_SFR.w[ai] };
                                    let bytes = w.to_le_bytes();
                                    let mut bi = 0usize;
                                    while bi < 4 && rn < reply.len() {
                                        let b = bytes[bi];
                                        if b == 0 {
                                            ai = 32;
                                            break;
                                        }
                                        if (0x20..0x7f).contains(&b) {
                                            reply[rn] = b;
                                            rn += 1;
                                        }
                                        bi += 1;
                                    }
                                    ai += 1;
                                }
                                push_str(&mut reply, &mut rn, " ql=");
                                let nlook = unsafe { SMEM_NLOOK }.min(8);
                                for i in 0..nlook {
                                    if i != 0 {
                                        push_str(&mut reply, &mut rn, ",");
                                    }
                                    push_hex(&mut reply, &mut rn, unsafe { SMEM_LOOK[i as usize * 2] } as u64);
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(
                                        &mut reply,
                                        &mut rn,
                                        unsafe { SMEM_LOOK[i as usize * 2 + 1] } as u64,
                                    );
                                }
                                push_str(&mut reply, &mut rn, " w45=");
                                push_hex(&mut reply, &mut rn, unsafe { SAW_45 } as u64);
                                // 刀 G10：px=保存的 SO RRQ 原文（定宽 nibble）——
                                // 文件名+mode+选项区逐字节，解析器盲区的尾巴直接可见，
                                // 方言真图。tf_save_so 只存 SO 文件（TF_OPTBUF/TF_OPTLEN）。
                                // 刀 G11：让出 rp= 撞名（旧 rp=0.ffffffff 在前），帽 96→160。
                                push_str(&mut reply, &mut rn, " px=");
                                let n_rp = (unsafe { TF_OPTLEN } as usize).min(160);
                                let mut i_rp = 0usize;
                                while i_rp < n_rp {
                                    let b = unsafe { TF_OPTBUF[i_rp] };
                                    push_hex(&mut reply, &mut rn, (b >> 4) as u64);
                                    push_hex(&mut reply, &mut rn, (b & 0xF) as u64);
                                    i_rp += 1;
                                }
                                // 刀 G11：rpl=qrtr 尺寸段.抄本窗——size>room ⇒ 跨
                                // fragment 截断（G12 重组立案）；size≤room ⇒ 抄本即全量。
                                push_str(&mut reply, &mut rn, " rpl=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_RPL_SZ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_RPL_RM } as u64);
                                // 刀 G10：rr=最近 2 笔 op4/op5 原文（时间序旧→新，各 ≤24B）。
                                // op5 例：0005 0009 "End of transfer" ——消息文本＝拒收理由。
                                push_str(&mut reply, &mut rn, " rr=");
                                push_hex(&mut reply, &mut rn, unsafe { TF_RXR_N } as u64);
                                let n_rr = unsafe { TF_RXR_N }.min(2);
                                let mut k_rr = n_rr;
                                while k_rr > 0 {
                                    let ridx = ((unsafe { TF_RXR_N } - k_rr) & 1) as usize;
                                    let len = unsafe { TF_RXR_L[ridx] } as usize;
                                    push_str(&mut reply, &mut rn, ".");
                                    let mut i_rr = 0usize;
                                    while i_rr < len {
                                        let b = unsafe { TF_RXR[ridx][i_rr] };
                                        push_hex(&mut reply, &mut rn, (b >> 4) as u64);
                                        push_hex(&mut reply, &mut rn, (b & 0xF) as u64);
                                        i_rr += 1;
                                    }
                                    k_rr -= 1;
                                }
                                push_str(&mut reply, &mut rn, " q=");
                                let nsvc = unsafe { SMEM_NSVC }.min(8);
                                for i in 0..nsvc {
                                    if i != 0 {
                                        push_str(&mut reply, &mut rn, ",");
                                    }
                                    push_hex(&mut reply, &mut rn, unsafe { SMEM_SVC[i as usize * 2] } as u64);
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(
                                        &mut reply,
                                        &mut rn,
                                        unsafe { SMEM_SVC[i as usize * 2 + 1] } as u64,
                                    );
                                }
                                // iq=seen.inst — IPA QMI 0x31 NEW_SERVER 闩（零写）
                                push_str(&mut reply, &mut rn, " iq=");
                                push_hex(&mut reply, &mut rn, unsafe { IQ_SEEN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IQ_INST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IQ_NODE } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IQ_PORT } as u64);
                                // ig= — IPA/GSI 只读 MMIO（PAS 绿后一次；零写）
                                ipa_gsi_ro_probe();
                                ipa_gsi_ring_carve();
                                push_str(&mut reply, &mut rn, " ig=");
                                push_hex(&mut reply, &mut rn, unsafe { IG_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_IVER } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_GCFG } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_GSTAT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_GVER } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_HP2 } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_CH0 } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_CA } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_CB } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_SMEM } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IG_R } as u64);
                                // gc= — GSI CH7 只读（与 ig 同次；零写；非 alloc/start）
                                push_str(&mut reply, &mut rn, " gc=");
                                push_hex(&mut reply, &mut rn, unsafe { GC_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GC_C0 } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GC_C1 } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GC_C2 } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GC_C3 } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GC_IRQ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GC_ERR } as u64);
                                // gr= — CH7 DRAM 划区（allocate-only；无 doorbell）
                                push_str(&mut reply, &mut rn, " gr=");
                                push_hex(&mut reply, &mut rn, unsafe { GR_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GR_CH } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GR_EVT } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GR_RX } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { GR_SZ } as u64);
                                // ii=st.txn.res — INIT_MODEM_DRIVER 单发闩
                                push_str(&mut reply, &mut rn, " ii=");
                                push_hex(&mut reply, &mut rn, unsafe { II_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { II_TXN } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { II_RES } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { II_WHY } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { II_HX } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { II_TL } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                let mut pk = 0usize;
                                while pk < 8 {
                                    if pk != 0 {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    push_hex(&mut reply, &mut rn, unsafe { II_PK[pk] } as u64);
                                    pk += 1;
                                }
                                push_str(&mut reply, &mut rn, " ic=");
                                push_hex(&mut reply, &mut rn, unsafe { IC_N } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IC_SENT } as u64);
                                push_str(&mut reply, &mut rn, " im=");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_REQ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IPA_LAST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { CFG_SENT } as u64);
                                push_str(&mut reply, &mut rn, " cg=");
                                if unsafe { CFG_CAP_N } == 0 {
                                    push_str(&mut reply, &mut rn, "-");
                                } else {
                                    let mut k = 0usize;
                                    while k < 4 {
                                        if k != 0 {
                                            push_str(&mut reply, &mut rn, ".");
                                        }
                                        push_hex(&mut reply, &mut rn, unsafe { CFG_CAP[k] } as u64);
                                        k += 1;
                                    }
                                }
                                push_str(&mut reply, &mut rn, " jx=");
                                if unsafe { JX_OTHER } == 0 {
                                    push_str(&mut reply, &mut rn, "-");
                                } else {
                                    push_hex(&mut reply, &mut rn, (unsafe { JX_OTHER } >> 16) as u64);
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(&mut reply, &mut rn, ((unsafe { JX_OTHER } >> 8) & 0xff) as u64);
                                    push_str(&mut reply, &mut rn, ".");
                                    push_hex(&mut reply, &mut rn, (unsafe { JX_OTHER } & 0xff) as u64);
                                }
                                push_str(&mut reply, &mut rn, " ir=");
                                push_hex(&mut reply, &mut rn, unsafe { IR_DONE } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { IND_SENT } as u64);
                                push_str(&mut reply, &mut rn, " x=");
                                for i in 0..22 {
                                    if i != 0 {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    push_hex(&mut reply, &mut rn, unsafe { SMEM_RX[i] } as u64);
                                }
                                push_str(&mut reply, &mut rn, " rn=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RNOTIF } as u64);
                                push_str(&mut reply, &mut rn, " gd=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_GDONE } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_GLIID } as u64);
                                push_str(&mut reply, &mut rn, "\n");
                            }
                            9 => {
                                proxy_vote(ufs, freq, t);
                                push_str(&mut reply, &mut rn, "pv=");
                                for i in 0..9 {
                                    if i != 0 {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    push_hex(&mut reply, &mut rn, unsafe { PROXY[i] } as u64);
                                }
                                push_str(&mut reply, &mut rn, " pa=");
                                push_hex(&mut reply, &mut rn, unsafe { PROXY_AUX[0] } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { PROXY_AUX[1] } as u64);
                                push_str(&mut reply, &mut rn, "\n");
                            }
                            10 => {
                                let addr = unsafe { PROXY[0] };
                                let sel = unsafe { PROXY[1] };
                                if sel > 15 || (addr >> 16) & 7 != CMD_DB_HW_ARC {
                                    push_str(&mut reply, &mut rn, "px skip\n");
                                } else {
                                    let sts = rpmh_cmd(addr, sel, freq);
                                    unsafe { PROXY[2] = sts };
                                    push_str(&mut reply, &mut rn, "px ");
                                    push_hex(&mut reply, &mut rn, sts as u64);
                                    push_str(&mut reply, &mut rn, "\n");
                                }
                            }
                            11 => {
                                // mbox_desc 在 msgram+0。只读 16 个字，不写、不踢 IPCC。
                                push_str(&mut reply, &mut rn, "qm=");
                                for i in 0..16 {
                                    if i != 0 {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    push_hex(
                                        &mut reply,
                                        &mut rn,
                                        rd32(QMP_MSGRAM + i * 4) as u64,
                                    );
                                }
                                push_str(&mut reply, &mut rn, "\n");
                            }
                            12 => {
                                // __qmp_rx_worker 的 LINK_DISCONNECTED：回 ack，
                                // 把 mcore 写成 LINK_UP，再踢 AOP 的 signal 0。
                                // 不打开通道，也不发 load_state。
                                let magic = rd32(QMP_MSGRAM);
                                let ulink = rd32(QMP_MSGRAM + 12);
                                let mlink = rd32(QMP_MSGRAM + 36);
                                if magic != 0x4d41_494c {
                                    push_str(&mut reply, &mut rn, "qn mag\n");
                                } else if ulink != 0xffff {
                                    push_str(&mut reply, &mut rn, "qn ul\n");
                                } else if mlink == 0xffff {
                                    push_str(&mut reply, &mut rn, "qn have\n");
                                } else {
                                    let sz = rd32(QMP_MSGRAM + 52);
                                    let off = rd32(QMP_MSGRAM + 56);
                                    wr32(QMP_MSGRAM + 16, ulink);
                                    wr32(QMP_MSGRAM + 36, 0xffff);
                                    wr32(QMP_MSGRAM + 40, 0xffff_0000);
                                    wr32(QMP_MSGRAM + 44, 0xffff_0000);
                                    wr32(QMP_MSGRAM + 48, 0xffff_0000);
                                    wr32(QMP_MSGRAM + 52, sz);
                                    wr32(QMP_MSGRAM + 56, off);
                                    unsafe {
                                        core::arch::asm!("dsb sy", options(nostack));
                                    }
                                    wr32(0x4080_0c, 0);
                                    push_str(&mut reply, &mut rn, "qn 1\n");
                                }
                            }
                            13 => {
                                // 链路已经 UP 时：补上远端通道 ack，本机通道写成
                                // CONNECTED，再踢。对应 qmp_startup。
                                let magic = rd32(QMP_MSGRAM);
                                let mlink = rd32(QMP_MSGRAM + 36);
                                let mack = rd32(QMP_MSGRAM + 40);
                                let mch = rd32(QMP_MSGRAM + 44);
                                if magic != 0x4d41_494c || mlink != 0xffff || mack != 0xffff {
                                    push_str(&mut reply, &mut rn, "qc nolink\n");
                                } else if mch == 0xffff {
                                    push_str(&mut reply, &mut rn, "qc have\n");
                                } else {
                                    let uch = rd32(QMP_MSGRAM + 20);
                                    let uack = rd32(QMP_MSGRAM + 24);
                                    if uch != uack {
                                        wr32(QMP_MSGRAM + 24, uch);
                                    }
                                    wr32(QMP_MSGRAM + 44, 0xffff);
                                    unsafe {
                                        core::arch::asm!("dsb sy", options(nostack));
                                    }
                                    wr32(0x4080_0c, 0);
                                    push_str(&mut reply, &mut rn, "qc 1\n");
                                }
                            }
                            14 => {
                                // 两边通道都是 CONNECTED 才发。96 字节，和 pil_notify_aop 一样。
                                let uch = rd32(QMP_MSGRAM + 20);
                                let uack = rd32(QMP_MSGRAM + 24);
                                let mch = rd32(QMP_MSGRAM + 44);
                                let mack = rd32(QMP_MSGRAM + 48);
                                let slot = QMP_MSGRAM + 0x128;
                                if uch != 0xffff || uack != 0xffff || mch != 0xffff || mack != 0xffff
                                {
                                    push_str(&mut reply, &mut rn, "qs nopen\n");
                                } else if rd32(slot) != 0 {
                                    push_str(&mut reply, &mut rn, "qs busy\n");
                                } else {
                                    let text =
                                        b"{class: image, res: load_state, name: modem, val: on}";
                                    let mut msg = [0u8; 96];
                                    msg[..text.len()].copy_from_slice(text);
                                    let mut i = 0;
                                    while i < 24 {
                                        let b = i * 4;
                                        let w = msg[b] as u32
                                            | (msg[b + 1] as u32) << 8
                                            | (msg[b + 2] as u32) << 16
                                            | (msg[b + 3] as u32) << 24;
                                        wr32(slot + 4 + i * 4, w);
                                        i += 1;
                                    }
                                    unsafe {
                                        core::arch::asm!("dsb sy", options(nostack));
                                    }
                                    wr32(slot, 96);
                                    let _ = rd32(slot);
                                    unsafe {
                                        core::arch::asm!("dsb sy", options(nostack));
                                    }
                                    wr32(0x4080_0c, 0);
                                    let end = time::cntvct() + (freq as u64) / 5;
                                    let mut left = rd32(slot);
                                    while left != 0 && time::cntvct() < end {
                                        left = rd32(slot);
                                    }
                                    push_str(&mut reply, &mut rn, "qs ");
                                    push_hex(&mut reply, &mut rn, left as u64);
                                    push_str(&mut reply, &mut rn, "\n");
                                }
                            }
                            15 => {
                                sfr_read(freq);
                                let s = unsafe { &SFR };
                                push_str(&mut reply, &mut rn, "fr st=");
                                push_hex(&mut reply, &mut rn, s.st as u64);
                                push_str(&mut reply, &mut rn, " sz=");
                                push_hex(&mut reply, &mut rn, s.sz as u64);
                                push_str(&mut reply, &mut rn, " gp=");
                                push_hex(&mut reply, &mut rn, s.gp as u64);
                                push_str(&mut reply, &mut rn, " at=");
                                push_hex(&mut reply, &mut rn, s.at as u64);
                                push_str(&mut reply, &mut rn, " c=");
                                push_hex(&mut reply, &mut rn, s.c as u64);
                                push_str(&mut reply, &mut rn, " w=");
                                for i in 0..8 {
                                    if i != 0 {
                                        push_str(&mut reply, &mut rn, ".");
                                    }
                                    push_hex(&mut reply, &mut rn, s.w[i] as u64);
                                }
                                push_str(&mut reply, &mut rn, "\n");
                            }
                            16 => {
                                // 发送环 pending 起 56B hex，对账 intent/TX_DATA/READ_NOTIF。
                                let d = unsafe { LG_D };
                                let txf = unsafe { LG_TX };
                                let txn = unsafe { LG_TXN };
                                let tt = if d != 0 {
                                    smem_rd32(d).unwrap_or(0) as u64
                                } else {
                                    0
                                };
                                let th = if d != 0 {
                                    smem_rd32(d + 4).unwrap_or(0) as u64
                                } else {
                                    0
                                };
                                push_str(&mut reply, &mut rn, "tx gq=");
                                push_hex(&mut reply, &mut rn, th);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, tt);
                                push_str(&mut reply, &mut rn, " rn=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_RNOTIF } as u64);
                                push_str(&mut reply, &mut rn, " gd=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_GDONE } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { FP_GLIID } as u64);
                                push_str(&mut reply, &mut rn, " ix=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_IXTRA } as u64);
                                push_str(&mut reply, &mut rn, " ru=");
                                push_hex(&mut reply, &mut rn, unsafe { FP_REUSE } as u64);
                                push_str(&mut reply, &mut rn, " h=");
                                if txf != 0 && tt + 56 <= txn {
                                    dcache_ivac(txf + tt, 56);
                                    let mut i = 0u64;
                                    while i < 14 {
                                        if i != 0 {
                                            push_str(&mut reply, &mut rn, ".");
                                        }
                                        push_hex(
                                            &mut reply,
                                            &mut rn,
                                            smem_rd32(txf + tt + i * 4).unwrap_or(0xffff_ffff) as u64,
                                        );
                                        i += 1;
                                    }
                                } else {
                                    push_str(&mut reply, &mut rn, "-");
                                }
                                // 前 16B 上下文（若 tail>=16）
                                push_str(&mut reply, &mut rn, " p=");
                                if txf != 0 && tt >= 16 {
                                    let po = tt - 16;
                                    dcache_ivac(txf + po, 16);
                                    let mut i = 0u64;
                                    while i < 4 {
                                        if i != 0 {
                                            push_str(&mut reply, &mut rn, ".");
                                        }
                                        push_hex(
                                            &mut reply,
                                            &mut rn,
                                            smem_rd32(txf + po + i * 4).unwrap_or(0xffff_ffff) as u64,
                                        );
                                        i += 1;
                                    }
                                } else {
                                    push_str(&mut reply, &mut rn, "-");
                                }
                                push_str(&mut reply, &mut rn, "\n");
                            }
                            3 => {}

                            _ => push_str(&mut reply, &mut rn, "?\n"),
                        }
                        if rn > 0 && ctrl_reply(sock, &reply[..rn]) {
                            s20_keep(ufs, freq, t, " s20 cmd=0x", code as u64);
                            queued = true;
                        }
                    } else if unsafe { CTRL_LINE_N } < 64 {
                        unsafe {
                            let i = CTRL_LINE_N;
                            CTRL_LINE[i] = b;
                            CTRL_LINE_N = i + 1;
                        }
                    } else {
                        unsafe { CTRL_OVERFLOW = true };
                    }
                }
            }
            queued
        }
        // 只有一个套接字。停在 LastAck 上时，下一笔 SYN 会被 smoltcp RST，
        // 第二次 2323 之后 ping 也没了。直接 abort 再听。
        State::CloseWait | State::LastAck | State::FinWait1 | State::FinWait2 | State::TimeWait => {
            sock.abort();
            let _ = sock.listen(CTRL_PORT);
            unsafe {
                CTRL_GREETED = false;
                CTRL_LINE_N = 0;
                CTRL_OVERFLOW = false;
                TCP_SYN_AT = 0;
            }
            false
        }
        State::Closed => {
            let rc = tcp_listen_rc(sock.listen(CTRL_PORT));
            if rc != 0 && unsafe { !TCP_LS_BAD } {
                unsafe { TCP_LS_BAD = true };
                tcp_mark(ufs, freq, t, " tcp ls=0x", rc as u64);
            }
            unsafe {
                CTRL_GREETED = false;
                CTRL_LINE_N = 0;
                CTRL_OVERFLOW = false;
            }
            false
        }
        _ => false,
    }
}

fn ctrl_poll(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96], arb: &spmi::Arb, n_apid: u32) {
    if freq == 0 {
        return;
    }
    if unsafe { !CTRL_UP } {
        let rc = ctrl_boot(freq);
        tcp_mark(ufs, freq, t, " tcp ls=0x", rc as u64);
    }
    if let Some(flags) = tcp_rx_flags() {
        if flags != unsafe { TCP_RXF } {
            unsafe { TCP_RXF = flags };
            tcp_mark(ufs, freq, t, " tcp rx=0x", flags as u64);
        }
    }
    unsafe {
        CTX_UFS = ufs;
        CTX_FREQ = freq;
        CTX_T = t;
    }
    let now = smoltcp::time::Instant::from_millis((time::cntvct() * 1000 / freq as u64) as i64);
    let again = unsafe {
        let iface = match CTRL_IFACE.as_mut() {
            Some(iface) => iface,
            None => return,
        };
        let mut phy = CtrlPhy;
        let slots = &mut *core::ptr::addr_of_mut!(CTRL_SLOTS);
        let mut sockets = smoltcp::iface::SocketSet::new(&mut slots[..]);
        iface.poll(now, &mut phy, &mut sockets);
        ctrl_service(ufs, freq, t, arb, n_apid, &mut sockets)
    };
    if again {
        unsafe {
            if let Some(iface) = CTRL_IFACE.as_mut() {
                let mut phy = CtrlPhy;
                let slots = &mut *core::ptr::addr_of_mut!(CTRL_SLOTS);
                let mut sockets = smoltcp::iface::SocketSet::new(&mut slots[..]);
                iface.poll(now, &mut phy, &mut sockets);
            }
        }
    }
}

fn s20_poll(ufs: &mut Ufs, freq: u32, t: &mut [u8; 96]) {
    let count = (rd32(USB_DWC3 + 0xc40c) & 0xfffc).min(4096);
    if count == 0 {
        return;
    }
    let mut left = count;
    while left > 0 {
        let lpos = unsafe { EP0.lpos };
        let ev = rd32(USB_EVBUF + lpos as usize);
        unsafe {
            EP0.lpos = (lpos + 4) % 4096;
            EP0.nevt = EP0.nevt.wrapping_add(1);
        }
        left -= 4;
        s20_event(ufs, freq, t, ev);
    }
    wr32(USB_DWC3 + 0xc40c, count);
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
    unsafe { GPT_ENTRIES = entries };

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
    // L0 活体是 USB_PD（pd_active=1），打开 dwc3 的是 policy_engine
    // 的 start_usb_peripheral，不是 CC 附着本身。CC 裸世界已经
    // attach。这里只读、失败不复位：
    //   cc = Type-C 0x06 | 0x09<<8 | 0x0B<<16（Rp / 状态机 / MISC）
    //   as = USBIN 0x07 | 0x08<<8 | 0x10<<16（APSD 状态 / 结果 / PLUGIN）
    //   pd = PDPHY 0x46 | 0x40<<8 | 0x4C<<16（EN_CONTROL / MSG_CONFIG / FILTER）
    // 缺副本或事务失败该字节为 0xFF。
    let sniff = |ppid: u32, off: u8| -> u64 {
        let Some(ap) = arb.find_apid_owner(ppid, 0, n_apid) else {
            return 0xff;
        };
        let mut b = [0u8; 1];
        match arb.read(ap, off, &mut b) {
            Ok(()) => b[0] as u64,
            Err(_) => 0xff,
        }
    };
    let cc = sniff(0x215, 0x06) | (sniff(0x215, 0x09) << 8) | (sniff(0x215, 0x0b) << 16);
    let apsd = sniff(0x213, 0x07) | (sniff(0x213, 0x08) << 8) | (sniff(0x213, 0x10) << 16);
    let pd = sniff(0x217, 0x46) | (sniff(0x217, 0x40) << 8) | (sniff(0x217, 0x4c) << 16);
    s20_log(&mut ufs, freq, &mut t, " s20 cc=0x", cc);
    s20_log(&mut ufs, freq, &mut t, " s20 as=0x", apsd);
    s20_log(&mut ufs, freq, &mut t, " s20 pd=0x", pd);
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
            let dev = UfsDev { ufs: &mut ufs, base: p.start as u32 };
            match ext4::Fs::open(dev, unsafe { &mut *core::ptr::addr_of_mut!(SC1) }) {
                Err(_) => open_err = 3,
                Ok(mut fs) => {
                    for (i, path) in TARGETS.iter().enumerate() {
                        match fs.resolve(path, unsafe { &mut *core::ptr::addr_of_mut!(SC2) }) {
                            Err(_) => slots[i][0] = 4,
                            Ok(ino) => {
                                let mut st = crc32::crc32_start();
                                let mut off = 0u64;
                                let mut nread = 0u64;
                                let mut err = 0u64;
                                loop {
                                    let buf = unsafe { &mut E2BUF };
                                    match fs.read_file(
                                        ino,
                                        off,
                                        buf,
                                        unsafe { &mut *core::ptr::addr_of_mut!(SC2) },
                                    ) {
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

    // ⑩-3 S3 首刀（s19 段）：USB HS-only 设备面上电。分段全落环、
    //     零错误复位（探针诊断=读数本身；超时/失配记 e 位掩码）。
    //     e 位图：bit0=GDSC PWR_ON 超时 bit1/2=RCG UPDATE 悬
    //     bits4..19=PHY 十六步失配 bit20=GSNPSID 非 5533
    //     bit21=DCTL 软复位悬。
    //     【读序铁律】GDSC/时钟未上前 dwc3 与 QUSB2 空间读=总线挂死
    //     （同步异常→环+复位，一刀白烧）——as-found 只读 GPLL0 与
    //     GDSCR（GCC 常活区）；dwc3/QUSB2 读全部后置到上电后。
    let t_s19 = time::cntvct();

    // A 交接残态二读（GCC 常活区，安全）：GPLL0 LUCID STATUS/MODE、
    //     GDSCR 原态（bit31 PWR_ON/bit0 SW_COLLAPSE=交接遗嘱）。
    let gpll0 = ((rd32(USB_GCC + 0x30) as u64) << 32) | rd32(USB_GCC) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 a0=0x", gpll0);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let a1 = rd32(USB_GDSC) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 a1=0x", a1);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);

    // PHY 寄存器能写不代表 pad 有 3.3V。三路是 RPMh VRM，没有 SPMI 偏移。
    rpmh_usb_rails(&mut ufs, freq, &mut t);
    // pv 实读：cx.lvl=0x30000 下标 6，mss.lvl=0x30060 下标 6，xo.lvl=0x300a0 数据 3。
    // 都在 PAS 之前。cx 单独投过，状态 0x10101，USB 能起来。
    let cxw = rpmh_cmd(0x30000, 6, freq);
    let msw = rpmh_cmd(0x30060, 6, freq);
    let xow = rpmh_cmd(0x300a0, 3, freq);
    unsafe {
        PROXY[0] = 0x30000;
        PROXY[1] = 6;
        PROXY[2] = cxw;
        PROXY[3] = 0x30060;
        PROXY[4] = 6;
        PROXY[5] = msw;
        PROXY[6] = 0x300a0;
        PROXY[7] = 3;
        PROXY[8] = xow;
    }
    s20_log(&mut ufs, freq, &mut t, " s20 cxw=0x", cxw as u64);
    s20_log(&mut ufs, freq, &mut t, " s20 msw=0x", msw as u64);
    s20_log(&mut ufs, freq, &mut t, " s20 xow=0x", xow as u64);
    // L2（vdd-pdphy）已投。再按 pd_phy_open 打开 PD PHY。
    pd_phy_open(&mut ufs, freq, &mut t);

    // B 上电正典（dwc3_msm_resume 序）：GDSC on→USB30 BCR→五时钟腿
    // →（此刻起 dwc3/QUSB2 读安全）→QUSB2 BCR→十六步 PHY。
    let mut e: u64 = 0;
    // ① GDSC：bit0 SW_COLLAPSE←0，poll bit31 PWR_ON（≤100ms）。
    rmw32(USB_GDSC, 1, 0);
    let end = time::cntvct() + freq as u64 / 10;
    while rd32(USB_GDSC) & 0x8000_0000 == 0 {
        if time::cntvct() > end {
            e |= 1;
            break;
        }
    }
    // ② USB30_PRIM_BCR（GCC+0xf000）：assert→100µs→deassert。
    wr32(USB_GCC + 0xf000, 1);
    let end = time::cntvct() + freq as u64 / 10_000;
    while time::cntvct() < end {}
    wr32(USB_GCC + 0xf000, 0);
    // ③ 五时钟腿。master RCG（0xf020）：CFG=GPLL0_MAIN(sel 1)/4.5
    //    （SRC_DIV=2h−1=8）→UPDATE poll 清；utmi RCG（0xf038）：
    //    XO(sel 0)/1（SRC_DIV=1）→UPDATE。CBCR=bit0←1、poll bit31
    //    CLK_OFF 清（≤1ms，超时不记位——b0 读数自证）。
    let mut rcg = |base: usize, cfg: u32, bit: u64| {
        wr32(base + 4, cfg);
        wr32(base, 1);
        let end = time::cntvct() + freq as u64 / 100;
        while rd32(base) & 1 != 0 {
            if time::cntvct() > end {
                e |= bit;
                break;
            }
        }
    };
    rcg(USB_GCC + 0xf020, (1 << 8) | 8, 2);
    rcg(USB_GCC + 0xf038, 1, 3);
    let cbcr = |base: usize| {
        wr32(base, 1);
        let end = time::cntvct() + freq as u64 / 1000;
        while rd32(base) & 0x8000_0000 != 0 {
            if time::cntvct() > end {
                break;
            }
        }
    };
    cbcr(USB_GCC + 0xf010); // master
    cbcr(USB_GCC + 0xf07c); // iface（cfg_noc axi）
    cbcr(USB_GCC + 0xf01c); // mock_utmi
    cbcr(USB_GCC + 0xf080); // bus_aggr
    cbcr(USB_GCC + 0xf018); // sleep
    // dwc3-msm.c resume：醒 HS PHY 之前先 vote TCXO。dts 第六个钟
    // clocks 名 "xo" = GCC_USB3_PRIM_CLKREF_CLK，门在 GCC+0x8c010。
    // mock UTMI 只喂控制器，不喂 PHY pad。
    cbcr(USB_GCC + 0x8c010);
    s20_log(&mut ufs, freq, &mut t, " s20 xo=0x", rd32(USB_GCC + 0x8c010) as u64);
    // phy-msm-snps-hs.c：eud_enable_reg（0x088e2000）非 0 则 init 直接返回，
    // PHY 不碰 D+/D-。eud.c CSR_EUD_EN = eud_base+0x1014。这里只读。
    let eud = ((rd32(0x088e_2000) as u64) << 32) | rd32(0x088e_1014) as u64;
    s20_log(&mut ufs, freq, &mut t, " s20 eu=0x", eud);
    let b0 = ((rd32(USB_GCC + 0xf010) as u64) << 32)
        | ((rd32(USB_GCC + 0xf01c) as u64) << 16)
        | rd32(USB_GCC + 0xf07c) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 b0=0x", b0);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // 上电后 dwc3/QUSB2 读安全：验身 + QUSB2 复位前原态（POR 遗嘱）。
    let a2 = ((rd32(USB_DWC3 + 0xc120) as u64) << 32) | rd32(USB_DWC3 + 0xc110) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 a2=0x", a2);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let a3 = ((rd32(USB_HSPHY + 0x94) as u64) << 32) | rd32(USB_HSPHY + 0x50) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 a3=0x", a3);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // ④ QUSB2 BCR（GCC+0x12000）：assert→100µs→deassert。
    wr32(USB_GCC + 0x12000, 1);
    let end = time::cntvct() + freq as u64 / 10_000;
    while time::cntvct() < end {}
    wr32(USB_GCC + 0x12000, 0);
    // ⑤ write-readback。X0/X1 用 lito-redfin-usb.dtsi 的覆盖
    //    （0xC7@0x6C、0xCC@0x70），不再写通用 lito 的 X2=0x17。
    //    (offset, mask, val)——序号即 e 位（bit4 起）。
    const PHY: [(u32, u32, u32); 15] = [
        (0x94, 1 << 1, 1 << 1),                                 // ① CMN_OVERRIDE_EN
        (0x50, 1 << 1, 1 << 1),                                 // ② POR
        (0x54, 0x7 << 4, 0),                                    // ③ FSEL←0
        (0x58, 1 << 5, 1 << 5),                                 // ④ PLLBTUNE
        (0xa0, 0x3, 0x2),                                       // ⑤ REFCLK_SEL
        (0x58, 1 << 4, 1 << 4),                                 // ⑥ VBUSVLDEXTSEL0
        (0x60, 1 << 0, 1 << 0),                                 // ⑦ VBUSVLDEXT0
        (0x6c, 0, 0xc7),                                        // ⑧ X0（redfin）
        (0x70, 0, 0xcc),                                        // ⑨ X1（redfin）
        (0x5c, 1 << 0, 1 << 0),                                 // ⑩ VREGBYPASS
        (0x64, (1 << 2) | (1 << 3), (1 << 2) | (1 << 3)),       // ⑫ SUSPEND_N|SEL
        (0x3c, 1 << 0, 1 << 0),                                 // ⑬ SLEEPM
        (0x50, 1 << 1, 0),                                      // ⑭ POR←0（出复位）
        (0x64, 1 << 3, 0),                                      // ⑮ SUSPEND_N_SEL←0
        (0x94, 1 << 1, 0),                                      // ⑯ OVERRIDE_EN←0
    ];
    let mut b1: u64 = 0;
    for (i, &(off, mask, val)) in PHY.iter().enumerate() {
        let a = USB_HSPHY + off as usize;
        if mask == 0 {
            wr32(a, val);
        } else {
            rmw32(a, mask, val);
        }
        let rb = rd32(a);
        // X0/X1/X2 内核只认低 8 位（PARAM_OVRD_MASK=0xFF）。整字比较会把
        // 高位状态当成失败。原始回读照记，低 8 位对齐才算过。
        if mask == 0 {
            let label = match i {
                7 => " s19 x0=0x",
                8 => " s19 x1=0x",
                _ => "",
            };
            if !label.is_empty() {
                let txt = hex_text(&mut t, unsafe { RING.seq }, label, rb as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
            }
        }
        let ok = if mask == 0 { rb & 0xff == val } else { rb & mask == val };
        if !ok {
            e |= 1 << (4 + i);
            b1 = (b1 << 4) | (i as u64 + 1);
        }
    }
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 b1=0x", b1);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // X2 板级表不写。记下复位后的原值。
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 x2=0x", rd32(USB_HSPHY + 0x74) as u64);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);

    // C dwc3 上线：软复位→GCTL 设备向→HS-only QSCRATCH 三步→事件
    // 缓冲→DCFG HS→DCTL 清 SFTDISCONNECT→2s 枚举窗→三读。
    let snps = rd32(USB_DWC3 + 0xc120);
    if snps >> 16 != 0x5533 {
        e |= 1 << 20;
    }
    // DCTL.CSFTRST = bit30（core.h:420）。bit11 是 ACCEPTU2ENA，不会自清；
    // 旧配方把 GCTL.CORESOFTRESET（core.h:253，另一个寄存器的 bit11）抄到了
    // DCTL 上，poll 必然超时，随后 DEPCMD 打在未复位的设备侧，总线停摆。
    rmw32(USB_DWC3 + 0xc704, 1 << 30, 1 << 30);
    let end = time::cntvct() + freq as u64 / 100;
    while rd32(USB_DWC3 + 0xc704) & (1 << 30) != 0 {
        if time::cntvct() > end {
            e |= 1 << 21;
            break;
        }
    }
    // 位已清：gadget.c 在 CSFTRST 之后空等 50ms 再碰 PHY 域。
    if rd32(USB_DWC3 + 0xc704) & (1 << 30) == 0 {
        let end = time::cntvct() + freq as u64 / 20;
        while time::cntvct() < end {}
    }
    rmw32(USB_DWC3 + 0xc110, 3 << 12, 2 << 12); // GCTL PRTCAP=DEVICE
    // HS-only QSCRATCH 三步（dwc3-msm POST_RESET 正牌）。
    let qs = USB_DWC3 + 0xf8800 + 8;
    rmw32(qs, 1 << 8, 1 << 8); // PIPE_UTMI_CLK_DIS
    let end = time::cntvct() + freq as u64 / 300_000;
    while time::cntvct() < end {}
    rmw32(qs, (1 << 0) | (1 << 3), (1 << 0) | (1 << 3)); // CLK_SEL|PHYSTATUS_SW
    let end = time::cntvct() + freq as u64 / 300_000;
    while time::cntvct() < end {}
    rmw32(qs, 1 << 8, 0);
    // 事件缓冲 4KB @0xB0005000（NC 窗空闲槽；先清零再挂）。
    for i in 0..1024usize {
        wr32(USB_EVBUF + i * 4, 0);
    }
    wr32(USB_DWC3 + 0xc400, USB_EVBUF as u32); // GEVNTADRLO(0)
    wr32(USB_DWC3 + 0xc404, (USB_EVBUF >> 32) as u32); // HI
    wr32(USB_DWC3 + 0xc408, 4096); // GEVNTSIZ(0)
    wr32(USB_DWC3 + 0xc40c, rd32(USB_DWC3 + 0xc40c)); // count 清零
    rmw32(USB_DWC3 + 0xc700, 7, 0); // DCFG DEVSPD=HS
    // 保持断开。CDP 的 D+ 脉冲必须是主机看到的第一下，不能在这里先上线两秒。
    let end = time::cntvct() + freq as u64 * 2;
    while time::cntvct() < end {}
    let c1 = ((rd32(USB_DWC3 + 0xc70c) as u64) << 32) | rd32(USB_DWC3 + 0xc40c) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 c1=0x", c1);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let c2 = unsafe { core::ptr::read_volatile(USB_EVBUF as *const u64) };
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 c2=0x", c2);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let t_s19 = time::cntvct() - t_s19;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 e=0x", e);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    let txt = hex_text(&mut t, unsafe { RING.seq }, " s19 t=0x", t_s19);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);

    // ⑩-4 S3 第二刀（s20）：EP0 武装 + 重新上线。枚举轮询挂在 soak 窗。
    s20_start(&mut ufs, freq, &mut t);

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
    unsafe {
        SOAK_QG = qg;
        SOAK_CHG = chg;
    }
    let v = ((PPID_QG as u64) << 16) | ((qg as u64) << 8) | arb.owner(qg) as u64;
    let txt = hex_text(&mut t, unsafe { RING.seq }, " o248=0x", v);
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // 出向 SMP2P 必须在 modem 拉起来之前放好。晚写的话它不再回头看。
    smem_scan(&mut ufs, false);
    s20_keep(
        &mut ufs,
        freq,
        &mut t,
        " s20 smk=0x",
        unsafe { SMEM_SNAP.k } as u64,
    );
    // LPASS 出向项 429 要在 PAS 之前写好。ADSP 认的是 slave-kernel 的 bit1，
    // 不是 PAS_AUTH 的返回值。分区是 host 0/2，IPCC 是 client 3 signal 2。
    let (pk, slv0, _part) = lpass_edge(true);
    unsafe {
        ADSP_PUB = pk;
        ADSP_PRE = slv0;
        if slv0 & 0x8000_0000 == 0 {
            ADSP_SLV = slv0;
        }
    }
    let txt = hex_text(
        &mut t,
        unsafe { RING.seq },
        " adsp smp=0x",
        unsafe { LPASS_HDR },
    );
    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
    // ADSP 先于 modem。固件在 vendor_a 的 ext4 里。失败只落环，modem 照旧。
    match load_adsp(&mut ufs, freq, &mut t) {
        Err(code) => {
            unsafe { ADSP_ST = code };
            let txt = hex_text(&mut t, unsafe { RING.seq }, " adsp e=0x", code as u64);
            ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
        }
        Ok(n) => {
            unsafe { ADSP_ST = 0x8000_0000 | n as u32 };
            let txt = hex_text(&mut t, unsafe { RING.seq }, " adsp n=0x", n as u64);
            ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
            let ar = aop_image(freq, b"{class: image, res: load_state, name: adsp, val: on}");
            let txt = hex_text(&mut t, unsafe { RING.seq }, " adsp aop=0x", ar as u64);
            ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
            if ar == 0 {
                let wiped = sfr_wipe(freq, 423);
                unsafe {
                    ADSP_Z = wiped.st;
                    ADSP_ZB = wiped.w[0] & 0xff;
                }
                let (p0, p1, p2) = pas_go(1, 0x8920_0000, 0x0280_0000);
                unsafe {
                    ADSP_P[0] = p0 as u32;
                    ADSP_P[1] = p1 as u32;
                    ADSP_P[2] = p2 as u32;
                }
                let txt = hex_text(&mut t, unsafe { RING.seq }, " adsp p0=0x", p0 as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                let txt = hex_text(&mut t, unsafe { RING.seq }, " adsp p1=0x", p1 as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                let txt = hex_text(&mut t, unsafe { RING.seq }, " adsp p2=0x", p2 as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                // subsys_start 等 err-ready，最多 proxy-timeout 10 秒。等到再开 modem。
                let t0 = time::cntvct();
                let dead = t0 + freq as u64 * 10;
                let step = if freq == 0 { 1 } else { freq as u64 / 20 };
                loop {
                    let (_c, v, _p) = lpass_edge(false);
                    if v & 0x8000_0000 == 0 {
                        unsafe { ADSP_SLV = v };
                    }
                    if v & 1 != 0 && unsafe { ADSP_FT } == 0xffff_ffff {
                        let ms = if freq == 0 {
                            0
                        } else {
                            (time::cntvct().wrapping_sub(t0) * 1000) / freq as u64
                        };
                        unsafe { ADSP_FT = ms as u32 };
                    }
                    // bit0 还在时先别收。内核先处理 fatal，干净的 ONLINE 只有 bit1。
                    if (adsp_online(v) && v & 1 == 0) || time::cntvct() >= dead {
                        break;
                    }
                    let nap = time::cntvct() + step;
                    while time::cntvct() < nap {
                        s20_poll(&mut ufs, freq, &mut t);
                        ctrl_poll(&mut ufs, freq, &mut t, &arb, n_apid);
                    }
                }
                let ms = if freq == 0 {
                    0
                } else {
                    (time::cntvct().wrapping_sub(t0) * 1000) / freq as u64
                };
                unsafe { ADSP_WT = ms as u32 };
                unsafe { ADSP_SFR = sfr_load(freq, 423) };
                let gc = lpass_glink();
                let txt = hex_text(&mut t, unsafe { RING.seq }, " adsp gl=0x", gc as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                let txt = hex_text(
                    &mut t,
                    unsafe { RING.seq },
                    " adsp rdy=0x",
                    ((ms & 0xffff_ffff) << 32) | unsafe { ADSP_SLV } as u64,
                );
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                // 库存是 ADSP ONLINE 之后大约 3 秒才 PIL modem。
                // bit2 到了就撤代理票，这段时间一直喂 USB。
                // bit0 这时还没来。fastrpc 的 OPEN 从这里开始才允许，并且只在接收 FIFO 空了之后发。
                unsafe { LG_ARM = 1 };
                let settle = time::cntvct() + freq as u64 * 3;
                loop {
                    let (_c, v, _p) = lpass_edge(false);
                    if v & 0x8000_0000 == 0 {
                        unsafe { ADSP_SLV = v };
                    }
                    if v & 1 != 0 && unsafe { ADSP_FT } == 0xffff_ffff {
                        let ms = if freq == 0 {
                            0
                        } else {
                            (time::cntvct().wrapping_sub(t0) * 1000) / freq as u64
                        };
                        unsafe { ADSP_FT = ms as u32 };
                    }
                    if v & 4 != 0 {
                        adsp_proxy_unvote(freq);
                    }
                    let _ = lpass_glink();
                    if time::cntvct() >= settle {
                        break;
                    }
                    let nap = time::cntvct() + step;
                    while time::cntvct() < nap {
                        s20_poll(&mut ufs, freq, &mut t);
                        ctrl_poll(&mut ufs, freq, &mut t, &arb, n_apid);
                    }
                }
            }
        }
    }

    // IPA fws：ADSP PAS 三步全 0 之后。先 STE+七票，再 pas_go(0xf)。
    // 禁 ceeb6fd 链首（IPA-before-ADSP）。CDSP 仅在本段 IPA 绿后、modem 前。
    if unsafe { ADSP_ST } & 0x8000_0000 != 0
        && unsafe { ADSP_P[0] } == 0
        && unsafe { ADSP_P[1] } == 0
        && unsafe { ADSP_P[2] } == 0
    {
        ipa_i4_once(freq);
        match load_ipa_fws(&mut ufs, freq, &mut t) {
            Err(code) => {
                unsafe { IPA_ST = code };
                let txt = hex_text(&mut t, unsafe { RING.seq }, " ipa e=0x", code as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
            }
            Ok(n) => {
                unsafe { IPA_ST = 0x8000_0000 | n as u32 };
                let txt = hex_text(&mut t, unsafe { RING.seq }, " ipa n=0x", n as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                let (p0, p1, p2) = pas_go(0xf, 0x8BC0_0000, 0x0001_0000);
                unsafe {
                    IPA_P[0] = p0 as u32;
                    IPA_P[1] = p1 as u32;
                    IPA_P[2] = p2 as u32;
                }
                let txt = hex_text(&mut t, unsafe { RING.seq }, " ipa p0=0x", p0 as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                let txt = hex_text(&mut t, unsafe { RING.seq }, " ipa p1=0x", p1 as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                let txt = hex_text(&mut t, unsafe { RING.seq }, " ipa p2=0x", p2 as u64);
                ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                // PAS 绿后一次：只读 IPA/GSI MMIO（ig=）；无 doorbell/INIT。
                if p0 == 0 && p1 == 0 && p2 == 0 {
                    ipa_gsi_ro_probe();
                    ipa_gsi_ring_carve();
                    // CH7 START 之后 INIT 被应答，USB 随即掉线。先不 START。
                }
            }
        }
    }

    // CDSP-before-modem：仅当 IPA PAS 绿（高位 + p0..p2==0）。
    // ADSP 绿是 IPA 门前提，故隐含。IPA skip/fail → 不跑，防 8d88cd5 冷挂重放。
    // 序：smp2p → AOP cdsp on → load_cdsp → pil-cdsp bus(CO2+CO0) → pas_go(18) → err-ready → modem。
    if unsafe { IPA_ST } & 0x8000_0000 != 0
        && unsafe { IPA_P[0] } == 0
        && unsafe { IPA_P[1] } == 0
        && unsafe { IPA_P[2] } == 0
    {
        let (pk, slv0, _part) = cdsp_edge(true);
        unsafe {
            CDSP_PUB = pk;
            if slv0 & 0x8000_0000 == 0 {
                CDSP_RDY = slv0;
            }
        }
        let txt = hex_text(&mut t, unsafe { RING.seq }, " cdsp smp=0x", pk as u64);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
        let ar = aop_image(freq, b"{class: image, res: load_state, name: cdsp, val: on}");
        let txt = hex_text(&mut t, unsafe { RING.seq }, " cdsp aop=0x", ar as u64);
        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
        if ar == 0 {
            match load_cdsp(&mut ufs, freq, &mut t) {
                Err(code) => {
                    unsafe { CDSP_ST = code };
                    let txt = hex_text(&mut t, unsafe { RING.seq }, " cdsp e=0x", code as u64);
                    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                }
                Ok(n) => {
                    unsafe { CDSP_ST = 0x8000_0000 | n as u32 };
                    let txt = hex_text(&mut t, unsafe { RING.seq }, " cdsp n=0x", n as u64);
                    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                    // AUTH 前 pil-cdsp ACTIVE（CO2+CO0）；再 sfr_wipe(601) 对齐 ADSP 423。
                    cdsp_bus_vote(freq);
                    let txt = hex_text(&mut t, unsafe { RING.seq }, " cdsp cb=0x", unsafe { CDSP_CB } as u64);
                    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                    let wiped = sfr_wipe(freq, 601);
                    unsafe {
                        CDSP_Z = wiped.st;
                        CDSP_ZB = wiped.w[0] & 0xff;
                    }
                    let txt = hex_text(
                        &mut t,
                        unsafe { RING.seq },
                        " cdsp cz=0x",
                        ((unsafe { CDSP_Z } as u64) << 32) | unsafe { CDSP_ZB } as u64,
                    );
                    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                    let (p0, p1, p2) = pas_go(18, 0x8740_0000, 0x01E0_0000);
                    unsafe {
                        CDSP_P[0] = p0 as u32;
                        CDSP_P[1] = p1 as u32;
                        CDSP_P[2] = p2 as u32;
                    }
                    let txt = hex_text(&mut t, unsafe { RING.seq }, " cdsp p0=0x", p0 as u64);
                    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                    let txt = hex_text(&mut t, unsafe { RING.seq }, " cdsp p1=0x", p1 as u64);
                    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                    let txt = hex_text(&mut t, unsafe { RING.seq }, " cdsp p2=0x", p2 as u64);
                    ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                    if p0 == 0 && p1 == 0 && p2 == 0 {
                        let t0 = time::cntvct();
                        let dead = t0 + freq as u64 * 10;
                        let step = if freq == 0 { 1 } else { freq as u64 / 20 };
                        loop {
                            let (_c, v, _p) = cdsp_edge(false);
                            if v & 0x8000_0000 == 0 {
                                unsafe { CDSP_RDY = v };
                            }
                            // bit0 fatal 时先别收；干净 ONLINE 只有 bit1。
                            if (adsp_online(v) && v & 1 == 0) || time::cntvct() >= dead {
                                break;
                            }
                            let nap = time::cntvct() + step;
                            while time::cntvct() < nap {
                                s20_poll(&mut ufs, freq, &mut t);
                                ctrl_poll(&mut ufs, freq, &mut t, &arb, n_apid);
                            }
                        }
                        let ms = if freq == 0 {
                            0
                        } else {
                            (time::cntvct().wrapping_sub(t0) * 1000) / freq as u64
                        };
                        unsafe { CDSP_WT = ms as u32 };
                        let v = unsafe { CDSP_RDY };
                        let txt = hex_text(
                            &mut t,
                            unsafe { RING.seq },
                            " cdsp rdy=0x",
                            ((ms & 0xffff_ffff) << 32) | v as u64,
                        );
                        ring_put(&mut ufs, freq, logring::LVL_INFO, txt);
                    }
                }
            }
        }
    }

    // 冲 0x45：modem PAS 前灌真 EFS 影子，避免空白 fsg → DOG → 无 wlan_pd。
    rfs_seed_efs(&mut ufs);
    s20_log(&mut ufs, freq, &mut t, " s20 es=0x", unsafe { RFS_SEED } as u64);
    // msm_sharedmem_probe 在 modem 起来之前就把窗口交给 MSS。
    let _asg = rfs_assign();
    s20_log(&mut ufs, freq, &mut t, " s20 rfs=0x", unsafe { RF_AS } as u64);
    // pil_boot 第一件事。qs 在 PAS 之后被吃掉，但没有多出 0x45。
    let aop = aop_image(
        freq,
        b"{class: image, res: load_state, name: modem, val: on}",
    );
    unsafe { QMPW[0] = aop; }
    s20_log(&mut ufs, freq, &mut t, " s20 aop=0x", aop as u64);
    // 邮箱长度回到 0 才装 modem。0xe6 是通道还没四边 CONNECTED。
    if aop == 0 {
        pas_init_image(&mut ufs, freq, &mut t);
    }

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
        while time::cntvct() < end {
            s20_poll(&mut ufs, freq, &mut t);
            ctrl_poll(&mut ufs, freq, &mut t, &arb, n_apid);
            unsafe {
                LISTEN_TICK = LISTEN_TICK.wrapping_add(1);
                // HELLO 之后把三包公告赶在 modem 的 RFS 客户超时之前。
                // 传输中每 2 圈扫一次，ACK 要在 timeoutms 里回来。
                let xfer = TF_ST == 1 || TF_ST == 2 || (TF_PEND && TF_OP == 1 && tf_is_ruleset());
                let every = if GLINK_HELLO && HOST_ADV < 4 {
                    5
                } else if xfer {
                    2
                } else {
                    200
                };
                if LISTEN_TICK >= every {
                    LISTEN_TICK = 0;
                    lpass_glink();
                }
            }
            if unsafe { LISTEN_TICK } == 0 {
                smem_scan(&mut ufs, true);
            }
            fb_tick(&arb, freq, &mut ufs, &mut t);
            let slice = time::cntvct() + freq as u64 / 1000;
            while time::cntvct() < slice && time::cntvct() < end {
                core::hint::spin_loop();
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // 999 = Rust panic 面（u4j 无此码——出现即新病，非 u4j 等价行为）
    let freq = time::freq_or(FREQ_FALLBACK as u32);
    instrument::report_reset(PS_HOLD, 999, freq)
}
