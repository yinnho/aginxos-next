//! UFSHCI 事务层（裸件契约，TRUTH.md §7 定稿，u4f–u4j 实证）：
//!
//! - UTRLBA/RSR 32 位写；UTRD 每发重铺（DW2 预填 0x0F、DW4=UCD、
//!   DW6=RUO 0x80|RUL 0x80）；**DW1 不写**（DUNL 保持 0）。
//! - DW7 = **offset<<16 | count**（ufshci.h:560 length 在低半——10-14 夜
//!   翻案，u3 原值才对）：SSU 0x0100_0000（PRDT off 0x100、零条）；
//!   READ 0x0100_0001（off 0x100=UCD+0x400、一条 PRDT）。
//! - 命令 UPIU：头 3 dword（12B）；exp_len BE32 @+12（`.swap_bytes()` 律）；
//!   CDB @+16；响应 @+0x200（transcode byte0==0x21、SCSI 状态 byte+6、
//!   sense 定式 @+0x22、key @+0x24）；PRDT base @+0x400、size=字节−1。
//! - SSU ACTIVE 先行：bootloader 交接留 PowerDown，普通 LU 一律 NOT
//!   READY（#492 根因）。
//! - 每发 tag++；dsb sy 后敲铃；门铃 1s 超时；OCS→0x21→状态→sense 逐层判。

use crate::time;

/// 事务失败分类。设备侧映射成时延仪器码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    /// 门铃 1s 未清（分命令码）
    Timeout,
    /// OCS≠0（控制器层错）
    Ocs(u8),
    /// 响应 transcode≠0x21
    BadResp,
    /// SCSI CHECK CONDITION（sense key）
    Check(u8),
}

fn rd32(p: usize) -> u32 {
    unsafe { (p as *const u32).read_volatile() }
}
fn wr32(p: usize, v: u32) {
    unsafe { (p as *mut u32).write_volatile(v) }
}
fn wr64(p: usize, v: u64) {
    unsafe { (p as *mut u64).write_volatile(v) }
}
fn wr8(p: usize, v: u8) {
    unsafe { (p as *mut u8).write_volatile(v) }
}
fn rd8(p: usize) -> u8 {
    unsafe { (p as *const u8).read_volatile() }
}

pub struct Ufs {
    hci: usize,   // UFSHCI 寄存器基址
    utrd: usize,  // UTRD 槽 0（32B）
    ucd: usize,   // UCD（cmd 0x200 | resp 0x200 | PRDT @+0x400）
    freq: u32,    // 门铃超时用时钟
    tag: u8,      // 每发自增
}

const UTRLBA: usize = 0x50; // +0x54 高 32 位
const UTRLDBR: usize = 0x58; // 门铃
const UTRLRSR: usize = 0x60; // run/stop

impl Ufs {
    pub fn new(hci: usize, utrd: usize, ucd: usize, freq: u32) -> Self {
        Ufs { hci, utrd, ucd, freq, tag: 0 }
    }

    /// 列基址 + run/stop（32 位写，u4j 同款）。
    pub fn init(&mut self) {
        wr32(self.hci + UTRLBA, self.utrd as u32);
        wr32(self.hci + UTRLBA + 4, 0);
        wr32(self.hci + UTRLRSR, 1);
    }

    /// 重铺 UTRD/UCD：UCD 清 0x600、UTRD 清 32B、DW2=0x0F、DW4=UCD、DW6=0x00800080。
    fn prep(&mut self) {
        for off in (0..0x600).step_by(8) {
            wr64(self.ucd + off, 0);
        }
        for off in (0..32).step_by(8) {
            wr64(self.utrd + off, 0);
        }
        wr32(self.utrd + 8, 0x0F);
        wr64(self.utrd + 16, self.ucd as u64);
        wr32(self.utrd + 24, 0x0080_0080);
    }

    /// 敲铃并逐层判：tag、dsb、门铃、1s 轮询、OCS、0x21、状态/sense。
    fn issue(&mut self) -> Result<(), Fail> {
        wr8(self.ucd + 3, self.tag);
        self.tag = self.tag.wrapping_add(1);
        unsafe { core::arch::asm!("dsb sy") };
        wr32(self.hci + UTRLDBR, 1);
        let deadline = time::cntvct() + self.freq as u64;
        while rd32(self.hci + UTRLDBR) & 1 != 0 {
            if time::cntvct() >= deadline {
                return Err(Fail::Timeout);
            }
        }
        let ocs = (rd32(self.utrd + 8) & 0x0F) as u8;
        if ocs != 0 {
            return Err(Fail::Ocs(ocs));
        }
        if rd8(self.ucd + 0x200) != 0x21 {
            return Err(Fail::BadResp);
        }
        if rd8(self.ucd + 0x206) == 0 {
            return Ok(());
        }
        // CHECK CONDITION：sense 定式 0x70 才取 key，否则 0xFF（u4j 同款）
        let mut key = 0xFF;
        if rd8(self.ucd + 0x222) & 0x7F == 0x70 {
            key = rd8(self.ucd + 0x224) & 0x0F;
        }
        Err(Fail::Check(key))
    }

    /// SSU ACTIVE → W-LUN 0xD0（power condition 1<<4）。UA(key 6) 重试由调用方。
    pub fn ssu_active(&mut self) -> Result<(), Fail> {
        self.prep();
        wr32(self.utrd, 0x1000_0000);          // DW0：CT1、无 DD
        wr32(self.utrd + 28, 0x0100_0000);     // DW7：PRDT off 0x100 | 0 条
        wr32(self.ucd, 0x00D0_0001);           // 内存 01 00 D0 00（trans/flags/LUN D0/tag0）
        wr8(self.ucd + 16, 0x1B);              // CDB[0] START STOP UNIT
        wr8(self.ucd + 20, 0x10);              // CDB[4] power condition ACTIVE
        self.issue()
    }

    /// WRITE(10) FUA：lba、blocks（4096B 块）、buf 为调用者组好的源（不清）。
    /// UA(key 6) 内部重试 ≤2。sense 7（DATA PROTECT）由调用者识别。
    /// u5a 契约：DW0 0x1200_0000、flags 0x20、CDB 2A 08。
    pub fn write10(&mut self, lba: u32, blocks: u16, buf: usize) -> Result<(), Fail> {
        let bytes = (blocks as u32) << 12;
        let mut retries = 0u8;
        loop {
            self.prep();
            wr32(self.utrd, 0x1200_0000);      // DW0：type1|DD 写
            wr32(self.utrd + 28, 0x0100_0001); // DW7：PRDT off 0x100 | 1 条
            wr32(self.ucd, 0x0000_2001);       // 内存 01 20 00 tag（flags W）
            wr32(self.ucd + 12, bytes.swap_bytes()); // exp_len BE32
            wr8(self.ucd + 16, 0x2A);          // CDB[0] WRITE(10)
            wr8(self.ucd + 17, 0x08);          // CDB[1] FUA
            let lba_be = lba.to_be_bytes();
            wr8(self.ucd + 18, lba_be[0]);     // CDB[2..5] LBA BE
            wr8(self.ucd + 19, lba_be[1]);
            wr8(self.ucd + 20, lba_be[2]);
            wr8(self.ucd + 21, lba_be[3]);
            let blk_be = blocks.to_be_bytes();
            wr8(self.ucd + 23, blk_be[0]);     // CDB[7..8] 块数 BE
            wr8(self.ucd + 24, blk_be[1]);
            wr64(self.ucd + 0x400, buf as u64); // PRDT base
            wr32(self.ucd + 0x40C, bytes - 1);  // size = 字节−1
            match self.issue() {
                Ok(()) => return Ok(()),
                Err(Fail::Check(6)) if retries < 2 => retries += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// READ(10)：lba（32 位内）、blocks（4096B 块）、buf 先清后读。
    /// UA(key 6) 内部重试 ≤2（u4j read 同款）。
    pub fn read10(&mut self, lba: u32, blocks: u16, buf: usize) -> Result<(), Fail> {
        let bytes = (blocks as u32) << 12;
        let mut retries = 0u8;
        loop {
            self.prep();
            for off in (0..bytes as usize).step_by(8) {
                wr64(buf + off, 0);
            }
            wr32(self.utrd, 0x1400_0000);      // DW0：type1|DD 读
            wr32(self.utrd + 28, 0x0100_0001); // DW7：PRDT off 0x100 | 1 条
            wr32(self.ucd, 0x0000_4001);       // 内存 01 40 00 tag（tag 由 issue 补）
            wr32(self.ucd + 12, bytes.swap_bytes()); // exp_len BE32（rev 律）
            wr8(self.ucd + 16, 0x28);          // CDB[0] READ(10)
            let lba_be = lba.to_be_bytes();
            wr8(self.ucd + 18, lba_be[0]);     // CDB[2..5] LBA BE
            wr8(self.ucd + 19, lba_be[1]);
            wr8(self.ucd + 20, lba_be[2]);
            wr8(self.ucd + 21, lba_be[3]);
            let blk_be = blocks.to_be_bytes();
            wr8(self.ucd + 23, blk_be[0]);     // CDB[7..8] 块数 BE
            wr8(self.ucd + 24, blk_be[1]);
            wr64(self.ucd + 0x400, buf as u64); // PRDT base
            wr32(self.ucd + 0x40C, bytes - 1);  // size = 字节−1
            match self.issue() {
                Ok(()) => return Ok(()),
                Err(Fail::Check(6)) if retries < 2 => retries += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// READ(10) 指定 LUN（UPIU 头第 3 字节）。LUN0 = 默认 read10。
    /// 冲 EFS：modemst/fsg 在 sdf（常见 LUN5），不在 sda。
    pub fn read10_lun(&mut self, lun: u8, lba: u32, blocks: u16, buf: usize) -> Result<(), Fail> {
        let bytes = (blocks as u32) << 12;
        let mut retries = 0u8;
        loop {
            self.prep();
            for off in (0..bytes as usize).step_by(8) {
                wr64(buf + off, 0);
            }
            wr32(self.utrd, 0x1400_0000);
            wr32(self.utrd + 28, 0x0100_0001);
            // 01 40 <lun> tag —— LE 字：tag<<24 | lun<<16 | 0x40<<8 | 0x01
            wr32(self.ucd, 0x0000_4001 | ((lun as u32) << 16));
            wr32(self.ucd + 12, bytes.swap_bytes());
            wr8(self.ucd + 16, 0x28);
            let lba_be = lba.to_be_bytes();
            wr8(self.ucd + 18, lba_be[0]);
            wr8(self.ucd + 19, lba_be[1]);
            wr8(self.ucd + 20, lba_be[2]);
            wr8(self.ucd + 21, lba_be[3]);
            let blk_be = blocks.to_be_bytes();
            wr8(self.ucd + 23, blk_be[0]);
            wr8(self.ucd + 24, blk_be[1]);
            wr64(self.ucd + 0x400, buf as u64);
            wr32(self.ucd + 0x40C, bytes - 1);
            match self.issue() {
                Ok(()) => return Ok(()),
                Err(Fail::Check(6)) if retries < 2 => retries += 1,
                Err(e) => return Err(e),
            }
        }
    }
}
