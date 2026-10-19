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
static mut TCP_RX: [u8; 2048] = [0; 2048];
static mut TCP_TX: [u8; 2048] = [0; 2048];
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
        Ok(n) => s20_keep(ufs, freq, t, " s20 ld=0x", n as u64),
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
/// rmtfs（端口 0x10）和 tftp（端口 0x11）各留一个未答的请求。
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
/// 打到 0x10 的请求次数（含因 PEND/seen 丢掉的）。
static mut RF_HIT: u32 = 0;
/// 丢掉次数：bit0=seen bit1=pend。
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
static mut TF_NODE: u32 = 0;
static mut TF_PORT: u32 = 0;
static mut TF_OP: u32 = 0;
static mut TF_OFF: u32 = 0;
static mut TF_SEEN: [u32; 8] = [0xffff_ffff; 8];
static mut TF_SEEN_N: u32 = 0;
static mut TF_N: u32 = 0;
static mut TF_NAME: [u8; 64] = [0; 64];
static mut TF_NAME_N: u8 = 0;
/// TFTP 真喂：st 0=闲 1=已命中待发 2=等 ACK 3=完成 4=未找到 5=路径/几何 6=UFS 7=空指针
/// sz=文件长 cl=首簇 blk=下一块号(1-based) off=已确认偏移
static mut TF_ST: u32 = 0;
static mut TF_SZ: u32 = 0;
static mut TF_CL: u32 = 0;
static mut TF_BLK: u32 = 0;
static mut TF_SOFF: u32 = 0;
static mut TF_CL_CUR: u32 = 0xffff_ffff;
static mut TF_CL_BUF: [u8; 16384] = [0; 16384];
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
fn smem_scan(do_glink: bool) {
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
                    tf_reply(at478, at479, sz479);
                    pm_reply(at478, at479, sz479);
                    sr_tick(at478, at479, sz479);
                    round += 1;
                    // Linux：fs1/fs2/fsg 连续 OPEN 后再 ALLOC；同轮多吞几笔。
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
    if rx_head <= rx_tail || rx_head > sz480 || rx_head > 0x80 {
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
            if lcid == 2 && count >= 1 && size >= 52 && (!found || size > unsafe { LAST_ISZ }) {
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
        if unsafe { HOST_ADV } < 4 {
            // qrtr.c：对方 HELLO 之后推本机服务。一次一包（一 intent）；0x35 打头。
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
    if n >= 4 || head + 72 > sz479 {
        return;
    }
    let (svc, inst, port) = if n == 0 {
        // Linux：svc=0x35 inst=0x1001 at 1:16407（非旧探针 1/0x13）。
        (PM_SVC, PM_INST, PM_QRTR_PORT)
    } else if n == 1 {
        (14, 1, 0x10)
    } else if n == 2 {
        (4096, 1, 0x11)
    } else {
        (64, 257, 0x12)
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
    // TFTP DATA 512+4=516；intent 常见 ≥0x800。旧 n≤256/total≤512 会卡 RRQ 真喂。
    if !unsafe { HAVE_IID } || n > 520 || chunk > isz || total > 640 || tx_head + total as u64 > sz479
    {
        return false;
    }
    let p = at479 + tx_head;
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
    wr32((at478 + 4) as usize, (tx_head + total as u64) as u32);
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

/// rmtfs。OPEN 对得上 storage.c 的路径就回 caller_id。
/// ALLOC 的 buff_size 0 按 rmt_storage_alloc_buff_cb 回窗口物理地址，并改走偏移。
fn rf_reply(at478: u64, at479: u64, sz479: u64) {
    if unsafe { !RF_PEND } || at478 == 0 || at479 == 0 {
        return;
    }
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
                    (unsafe { RF_PORT } << 16) | (txn & 0xffff);
                RF_SEEN_N += 1;
            }
            RF_PEND = false;
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
            RF_PEND = false;
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
/// 读 BPB，算出 root / data；对不上已知 data 起点就拒。
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
    // 4K LBA 相对分区起点
    let sec_per_4k = 4096 / bps;
    if sec_per_4k == 0 || data_sec % sec_per_4k != 0 {
        return Err(0x53);
    }
    let data_off = data_sec / sec_per_4k;
    let root_off = (reserved + nfats * fat_sz) / sec_per_4k;
    let spc_4k = (spc * bps) / 4096;
    if spc_4k == 0 || data_off != 17 || spc_4k != 4 {
        // 仍返回算出的数，但标错码；调用方可用硬编码回退
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
            if c >= b'a' && c <= b'z' { c - 32 } else { c }
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
            if c >= b'a' && c <= b'z' { c - 32 } else { c }
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
    let mut name = [b' '; 8];
    let mut ext = [b' '; 3];
    let mut dot = None;
    let mut i = 0usize;
    while i < comp.len() {
        if comp[i] == b'.' {
            dot = Some(i);
            break;
        }
        i += 1;
    }
    let (npart, epart) = match dot {
        Some(d) => (&comp[..d], &comp[d + 1..]),
        None => (comp, &comp[0..0]),
    };
    let mut i = 0usize;
    while i < npart.len() && i < 8 {
        let c = npart[i];
        name[i] = if c >= b'a' && c <= b'z' { c - 32 } else { c };
        i += 1;
    }
    i = 0;
    while i < epart.len() && i < 3 {
        let c = epart[i];
        ext[i] = if c >= b'a' && c <= b'z' { c - 32 } else { c };
        i += 1;
    }
    (name, ext)
}

/// 在 root 或子目录簇链里找 8.3 名。返回 (cluster, size, is_dir)。
fn fat_dir_find(
    ufs: &mut Ufs,
    data_lba: u32,
    dir_cl: u32,
    root_lba: u32,
    root_secs: u32,
    name: &[u8],
    ext: &[u8],
) -> Result<(u32, u32, bool), u32> {
    let (name8, ext3) = {
        let mut n = [0u8; 8];
        let mut e = [0u8; 3];
        n.copy_from_slice(name);
        e.copy_from_slice(ext);
        (n, e)
    };
    // root: dir_cl==0
    if dir_cl == 0 {
        let mut s = 0u32;
        while s < root_secs {
            ufs.read10(root_lba + s, 1, DATA).map_err(|_| 0x56u32)?;
            let b = unsafe { core::slice::from_raw_parts(DATA as *const u8, 4096) };
            let mut off = 0usize;
            while off + 32 <= 4096 {
                let ent = &b[off..off + 32];
                if ent[0] == 0 {
                    return Err(0x57);
                }
                if ent[0] != 0xe5 && ent[11] & 0x0f != 0x0f && fat83_eq(ent, &name8, &ext3) {
                    let cl = u16::from_le_bytes([ent[26], ent[27]]) as u32
                        | ((u16::from_le_bytes([ent[20], ent[21]]) as u32) << 16);
                    let sz = u32::from_le_bytes([ent[28], ent[29], ent[30], ent[31]]);
                    return Ok((cl, sz, ent[11] & 0x10 != 0));
                }
                off += 32;
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
            let mut off = 0usize;
            while off + 32 <= 4096 {
                let ent = &b[off..off + 32];
                if ent[0] == 0 {
                    return Err(0x57);
                }
                if ent[0] != 0xe5 && ent[11] & 0x0f != 0x0f && fat83_eq(ent, &name8, &ext3) {
                    let c2 = u16::from_le_bytes([ent[26], ent[27]]) as u32
                        | ((u16::from_le_bytes([ent[20], ent[21]]) as u32) << 16);
                    let sz = u32::from_le_bytes([ent[28], ent[29], ent[30], ent[31]]);
                    return Ok((c2, sz, ent[11] & 0x10 != 0));
                }
                off += 32;
            }
            s += 1;
        }
        cl = fat_next(cl) as u32;
    }
    Err(0x57)
}

/// 相对 modem 根的路径，如 image/modem_pr/so/549_38_0.mbn。
fn fat_path_open(ufs: &mut Ufs, rel: &[u8]) -> Result<(u32, u32), u32> {
    // 优先 BPB；对不上已知 data=+17/spc=4 时回退：reserved1 + 2×FAT6 + root4 = 17。
    let (root_lba, root_secs, data_lba) = match modem_fat_geom(ufs) {
        Ok(g) => g,
        Err(_) => (MODEM_LBA + 13, 4, MODEM_LBA + 17),
    };
    let mut dir_cl = 0u32;
    let mut start = 0usize;
    let mut i = 0usize;
    while i <= rel.len() {
        if i == rel.len() || rel[i] == b'/' {
            if i > start {
                let comp = &rel[start..i];
                let last = i == rel.len();
                let (n8, e3) = fat_split83(comp);
                let (cl, sz, is_dir) =
                    fat_dir_find(ufs, data_lba, dir_cl, root_lba, root_secs, &n8, &e3)?;
                if last {
                    if is_dir {
                        return Err(0x58);
                    }
                    return Ok((cl, sz));
                }
                if !is_dir {
                    return Err(0x58);
                }
                dir_cl = cl;
            }
            start = i + 1;
        }
        i += 1;
    }
    Err(0x57)
}

fn tf_strip_readonly(name: &[u8]) -> Option<&[u8]> {
    const P: &[u8] = b"/readonly/firmware/";
    if name.len() > P.len() && &name[..P.len()] == P {
        Some(&name[P.len()..])
    } else {
        None
    }
}

fn tf_load_cluster(ufs: &mut Ufs, data_lba: u32, cl: u32) -> Result<(), u32> {
    if unsafe { TF_CL_CUR } == cl {
        return Ok(());
    }
    if cl < 2 || cl >= 0xfff8 {
        return Err(0x59);
    }
    let lba = data_lba + (cl - 2) * 4;
    let mut s = 0u32;
    while s < 4 {
        ufs.read10(lba + s, 1, DATA).map_err(|_| 0x56u32)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                DATA as *const u8,
                TF_CL_BUF.as_mut_ptr().add(s as usize * 4096),
                4096,
            );
        }
        s += 1;
    }
    unsafe { TF_CL_CUR = cl };
    Ok(())
}

fn tf_file_byte(ufs: &mut Ufs, data_lba: u32, start_cl: u32, off: u32) -> Result<u8, u32> {
    let mut cl = start_cl;
    let mut o = off;
    let mut guard = 0u32;
    while o >= 16384 {
        cl = fat_next(cl) as u32;
        if cl < 2 || cl >= 0xfff8 {
            return Err(0x59);
        }
        o -= 16384;
        guard += 1;
        if guard > 65536 {
            return Err(0x59);
        }
    }
    tf_load_cluster(ufs, data_lba, cl)?;
    Ok(unsafe { TF_CL_BUF[o as usize] })
}

/// tftp。WRQ 仍 ACK0；RRQ 对 /readonly/firmware/* 从 modem_a FAT 真喂 512B DATA。
fn tf_reply(at478: u64, at479: u64, sz479: u64) {
    if at478 == 0 || at479 == 0 {
        return;
    }
    let tx_head = rd32((at478 + 4) as usize) as u64;
    let tx_tail = rd32(at478 as usize) as u64;
    if tx_head != tx_tail {
        return;
    }
    // 进行中的 DATA：等 ACK 时由 RX 清 TF_ST=1 再进这里发下一块。
    if unsafe { TF_ST } == 1 {
        let ufs = unsafe {
            if CTX_UFS.is_null() {
                TF_ST = 7;
                return;
            }
            &mut *CTX_UFS
        };
        let data_lba = MODEM_LBA + 17;
        let left = unsafe { TF_SZ.saturating_sub(TF_SOFF) };
        let ntake = if left > 512 { 512 } else { left } as usize;
        let mut q = [0u8; 520];
        q[0] = 0;
        q[1] = 3; // DATA
        q[2] = (unsafe { TF_BLK } >> 8) as u8;
        q[3] = unsafe { TF_BLK } as u8;
        let mut i = 0usize;
        while i < ntake {
            match tf_file_byte(ufs, data_lba, unsafe { TF_CL }, unsafe { TF_SOFF } + i as u32) {
                Ok(b) => q[4 + i] = b,
                Err(e) => {
                    unsafe { TF_ST = e };
                    return;
                }
            }
            i += 1;
        }
        if glink_tx_payload(
            at478,
            at479,
            tx_head,
            sz479,
            0x11,
            unsafe { TF_NODE },
            unsafe { TF_PORT },
            &q[..4 + ntake],
        ) {
            unsafe {
                // 2=等 ACK；短块也等最后 ACK，RX 里把 SOFF 推满后标 3。
                TF_ST = 2;
                TF_N = TF_N.wrapping_add(1);
            }
        }
        return;
    }
    if unsafe { !TF_PEND } {
        return;
    }
    let op = unsafe { TF_OP };
    if op == 2 {
        // ACK block 0 — WRQ 探针
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
            unsafe {
                if TF_SEEN_N < 8 {
                    TF_SEEN[TF_SEEN_N as usize] = TF_OFF;
                    TF_SEEN_N += 1;
                }
                TF_PEND = false;
                TF_N += 1;
            }
        }
        return;
    }
    if op != 1 {
        let mut q = [0u8; 16];
        q[0] = 0;
        q[1] = 5;
        q[2] = 0;
        q[3] = 4;
        let text = b"bad op";
        let mut n = 4usize;
        let mut i = 0usize;
        while i < text.len() && n < q.len() - 1 {
            q[n] = text[i];
            n += 1;
            i += 1;
        }
        q[n] = 0;
        n += 1;
        if glink_tx_payload(
            at478,
            at479,
            tx_head,
            sz479,
            0x11,
            unsafe { TF_NODE },
            unsafe { TF_PORT },
            &q[..n],
        ) {
            unsafe {
                if TF_SEEN_N < 8 {
                    TF_SEEN[TF_SEEN_N as usize] = TF_OFF;
                    TF_SEEN_N += 1;
                }
                TF_PEND = false;
                TF_N += 1;
            }
        }
        return;
    }
    // RRQ
    let namelen = unsafe { TF_NAME_N } as usize;
    let mut name = [0u8; 64];
    unsafe {
        let mut i = 0;
        while i < namelen && i < 64 {
            name[i] = TF_NAME[i];
            i += 1;
        }
    }
    let rel = match tf_strip_readonly(&name[..namelen]) {
        Some(r) => r,
        None => {
            // 非 firmware 路径：保持旧 ERROR
            let mut q = [0u8; 32];
            q[0] = 0;
            q[1] = 5;
            q[2] = 0;
            q[3] = 1;
            let text = b"file not found";
            let mut n = 4usize;
            let mut i = 0usize;
            while i < text.len() && n < q.len() - 1 {
                q[n] = text[i];
                n += 1;
                i += 1;
            }
            q[n] = 0;
            n += 1;
            if glink_tx_payload(
                at478,
                at479,
                tx_head,
                sz479,
                0x11,
                unsafe { TF_NODE },
                unsafe { TF_PORT },
                &q[..n],
            ) {
                unsafe {
                    TF_ST = 4;
                    if TF_SEEN_N < 8 {
                        TF_SEEN[TF_SEEN_N as usize] = TF_OFF;
                        TF_SEEN_N += 1;
                    }
                    TF_PEND = false;
                    TF_N += 1;
                }
            }
            return;
        }
    };
    let ufs = unsafe {
        if CTX_UFS.is_null() {
            TF_ST = 7;
            TF_PEND = false;
            return;
        }
        &mut *CTX_UFS
    };
    match fat_path_open(ufs, rel) {
        Ok((cl, sz)) => {
            unsafe {
                TF_CL = cl;
                TF_SZ = sz;
                TF_SOFF = 0;
                TF_BLK = 1;
                TF_CL_CUR = 0xffff_ffff;
                TF_ST = 1;
                TF_PEND = false;
                if TF_SEEN_N < 8 {
                    TF_SEEN[TF_SEEN_N as usize] = TF_OFF;
                    TF_SEEN_N += 1;
                }
            }
            // 立刻发第一块
            tf_reply(at478, at479, sz479);
        }
        Err(e) => {
            unsafe { TF_ST = if e < 0x100 { e } else { 5 } };
            let mut q = [0u8; 32];
            q[0] = 0;
            q[1] = 5;
            q[2] = 0;
            q[3] = 1;
            let text = b"file not found";
            let mut n = 4usize;
            let mut i = 0usize;
            while i < text.len() && n < q.len() - 1 {
                q[n] = text[i];
                n += 1;
                i += 1;
            }
            q[n] = 0;
            n += 1;
            if glink_tx_payload(
                at478,
                at479,
                tx_head,
                sz479,
                0x11,
                unsafe { TF_NODE },
                unsafe { TF_PORT },
                &q[..n],
            ) {
                unsafe {
                    if TF_SEEN_N < 8 {
                        TF_SEEN[TF_SEEN_N as usize] = TF_OFF;
                        TF_SEEN_N += 1;
                    }
                    TF_PEND = false;
                    TF_N += 1;
                }
            }
        }
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
    let new_free = free_u + ALLOC;
    wr32((base + 12) as usize, new_free);
    dcache_civac(base, 64);
    wr32(MUTEX, 0);
    // MPSS client 2，SMP2P signal 2。
    wr32(0x4080_0c, (2 << 16) | 2);
    (1, new_free, cached)
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
                    // "slave-kernel"
                    // "slave-kernel"：slav / e-ke / rnel
                    if w0 == 0x7661_6c73 && w1 == 0x656b_2d65 && w2 == 0x6c65_6e72 && w3 == 0 {
                        slave = rd32((ent + 16) as usize);
                        break;
                    }
                }
            }
        }
    }
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
    let mut off = 0u64;
    let mut guard = 0u32;
    while off + 8 <= rh && guard < 200 {
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
            if chunk > 512 || off + 16 + chunk > rh + 16 {
                break;
            }
            let step = (16 + chunk + 7) & !7;
            // 公告发完再还。只还本机发出的 1..=10，通道 2。不推接收尾。
            let lcid = w0 >> 16;
            let liid = rd32((fifo + off + 4) as usize);
            unsafe {
                // 不限 intent id：OPEN 可能用掉 >10 的 id，不还则 modem 发不出 ALLOC。
                if GLINK_INTENTS
                    && HOST_ADV >= 4
                    && lcid == 2
                    && liid != 0
                    && off >= RX_VIS
                    && RX_LIID_N < 16
                {
                    RX_LIID[RX_LIID_N as usize] = liid;
                    RX_LIID_N += 1;
                    RX_VIS = off + step;
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
                    if qtype == 0 && msgid >= 1 && msgid <= 5 && !seen && unsafe { RF_PEND } {
                        unsafe { RF_DROP |= 2 };
                    }
                    if qtype == 0 && msgid >= 1 && msgid <= 5 && !seen && unsafe { !RF_PEND } {
                        unsafe {
                            RF_HIT = RF_HIT.wrapping_add(1);
                            let slot = (RF_HIST_N % 8) as usize;
                            // msgid|txn<<8|sport<<24（sport 低 8 位）
                            RF_HIST[slot] = (msgid & 0xff)
                                | ((txn & 0xffff) << 8)
                                | ((sport & 0xff) << 24);
                            RF_HIST_N = RF_HIST_N.wrapping_add(1);
                        }
                        if msgid == 1 {
                            let msglen = ((w1 >> 8) & 0xffff) as u64;
                            let mut name = [0u8; 64];
                            let mut nn = 0usize;
                            if chunk >= 32 && msglen >= 3 && 7 + msglen <= chunk - 32 && msglen <= 96 {
                                let base = fifo + off + 55;
                                let mut p = 0u64;
                                while p + 3 <= msglen {
                                    let t = rd8(base + p);
                                    let ln = rd8(base + p + 1) as u64
                                        | ((rd8(base + p + 2) as u64) << 8);
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
                            unsafe {
                                RF_PATH = name;
                                RF_PATH_N = nn as u8;
                            }
                        }
                        unsafe {
                            RF_TXN = txn;
                            RF_NODE = rd32((fifo + off + 24) as usize);
                            RF_PORT = rd32((fifo + off + 28) as usize);
                            RF_MSG = msgid;
                            RF_OFF = off;
                            RF_FIFO = fifo;
                            RF_PEND = true;
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
                if typ == 1 && dst_node == 1 && dst_port == 0x11 && chunk >= 36 {
                    let op = ((rd8(fifo + off + 48) as u32) << 8) | (rd8(fifo + off + 49) as u32);
                    // ACK：推进已确认偏移，清成可发下一块。
                    if op == 4 && unsafe { TF_ST } == 2 {
                        let blk = ((rd8(fifo + off + 50) as u32) << 8) | (rd8(fifo + off + 51) as u32);
                        if blk == unsafe { TF_BLK } {
                            unsafe {
                                let ntake = {
                                    let left = TF_SZ.saturating_sub(TF_SOFF);
                                    if left > 512 { 512 } else { left }
                                };
                                TF_SOFF = TF_SOFF.saturating_add(ntake);
                                if TF_SOFF >= TF_SZ || ntake < 512 {
                                    TF_ST = 3;
                                } else {
                                    TF_BLK = TF_BLK.wrapping_add(1);
                                    TF_ST = 1;
                                }
                            }
                        }
                    }
                }
                if typ == 1 && dst_node == 1 && dst_port == 0x11 && chunk >= 34 {
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
                    if !seen && unsafe { !TF_PEND } {
                        let op = ((rd8(fifo + off + 48) as u32) << 8) | (rd8(fifo + off + 49) as u32);
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
                            TF_NODE = rd32((fifo + off + 24) as usize);
                            TF_PORT = rd32((fifo + off + 28) as usize);
                            TF_OP = op;
                            TF_OFF = key;
                            TF_NAME = name;
                            TF_NAME_N = nn as u8;
                            TF_PEND = true;
                        }
                    }
                }
                if chunk >= 52 && (typ == 4 || typ == 10) {
                    let svc = rd32((fifo + off + 52) as usize);
                    let inst = rd32((fifo + off + 56) as usize);
                    if typ == 4 {
                        let n = unsafe { SMEM_NSVC };
                        if n < 8 {
                            unsafe {
                                SMEM_SVC[n as usize * 2] = svc;
                                SMEM_SVC[n as usize * 2 + 1] = inst;
                                SMEM_NSVC = n + 1;
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
    unsafe { RX_PARSED = off; }
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
                        let mut reply = [0u8; 1536];
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
                                smem_scan(true);
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
                                push_hex(&mut reply, &mut rn, unsafe { TF_ST } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_SZ } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_CL } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_BLK } as u64);
                                push_str(&mut reply, &mut rn, ".");
                                push_hex(&mut reply, &mut rn, unsafe { TF_SOFF } as u64);
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
        State::CloseWait | State::LastAck | State::FinWait1 | State::FinWait2 | State::TimeWait => {
            sock.close();
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
    smem_scan(false);
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
                let every = if GLINK_HELLO && HOST_ADV < 4 { 5 } else { 200 };
                if LISTEN_TICK >= every {
                    LISTEN_TICK = 0;
                    lpass_glink();
                    smem_scan(true);
                }
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
