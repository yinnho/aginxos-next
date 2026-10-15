//! GPT 头自检（u4j §② 判据原样，纯切片——地址由设备侧给）。
//! 判序：magic → HeaderSize → SizeOfEntry → 条目数（含 ≤4096B 上限，
//! 0xB0003000 只容 1 块）→ PartitionEntryLBA（32 位内、非零）。

/// 头检通过的产出：CRC 计算长度、数组字节数、数组所在 LBA。
pub struct Header {
    /// CRC 计算长度（92..=4096，含头 CRC 字段本身）
    pub header_size: usize,
    /// NumberOfEntries × 128（≤4096）
    pub array_bytes: usize,
    /// PartitionEntryLBA（已验 32 位内、非零）
    pub entry_lba: u32,
}

/// 头不健全分类（S0-1 一律映射 92，分型留给日志环 S0-2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bad {
    Magic,
    HeaderSize,
    EntrySize,
    Entries,
    EntryLba,
}

fn le32(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | (b[o + 1] as u32) << 8 | (b[o + 2] as u32) << 16 | (b[o + 3] as u32) << 24
}

fn le64(b: &[u8], o: usize) -> u64 {
    le32(b, o) as u64 | (le32(b, o + 4) as u64) << 32
}

pub fn check(hdr: &[u8]) -> Result<Header, Bad> {
    if hdr.len() < 92 {
        return Err(Bad::HeaderSize);
    }
    if le64(hdr, 0) != 0x5452_4150_2049_4645 {
        // "EFI PART"
        return Err(Bad::Magic);
    }
    let header_size = le32(hdr, 12) as usize;
    if !(92..=4096).contains(&header_size) || header_size > hdr.len() {
        return Err(Bad::HeaderSize);
    }
    if le32(hdr, 84) != 128 {
        return Err(Bad::EntrySize);
    }
    let entries = le32(hdr, 80) as usize;
    if entries == 0 || entries > 512 || (entries << 7) > 4096 {
        return Err(Bad::Entries);
    }
    let elba = le64(hdr, 72);
    if elba == 0 || elba > u32::MAX as u64 {
        return Err(Bad::EntryLba);
    }
    Ok(Header { header_size, array_bytes: entries << 7, entry_lba: elba as u32 })
}

/// 头 CRC：期望值@16，零化该字段后算 header_size 字节（原地改头——
/// 一次性缓冲，无回读需求）。
pub fn header_crc_ok(hdr: &mut [u8], h: &Header) -> bool {
    let expected = le32(hdr, 16);
    hdr[16..20].fill(0);
    crate::crc32::crc32(&hdr[..h.header_size]) == expected
}

/// 数组 CRC：期望值@88（头缓冲只读），数组切片取 array_bytes。
pub fn array_crc_ok(hdr: &[u8], array: &[u8], h: &Header) -> bool {
    let expected = le32(hdr, 88);
    array.len() >= h.array_bytes && crate::crc32::crc32(&array[..h.array_bytes]) == expected
}
