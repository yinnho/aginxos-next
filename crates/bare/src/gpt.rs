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

/// 分区条目产出：起止 LBA（GPT 原始 64 位——越 32 位由调用者判）。
pub struct Part {
    pub start: u64,
    pub end: u64,
}

/// 在已校验条目数组按名找分区（u4k/u5a 契约）。名字限 ASCII，
/// 按 UTF-16LE 与条目名区（+56，72B，\0 填充）全量比对——
/// 比 u5a 的前 16 字节更严（防 boot_ax 误中）。找不到返 None。
pub fn find(array: &[u8], entries: usize, name: &str) -> Option<Part> {
    for i in 0..entries {
        let e = &array[i * 128..];
        if e.len() < 128 {
            return None;
        }
        if !name_matches(&e[56..128], name) {
            continue;
        }
        return Some(Part { start: le64(e, 32), end: le64(e, 40) });
    }
    None
}

/// 名区比对：UTF-16LE(name) + 0x0000 填充到区满（含 \0 终止符）。
fn name_matches(field: &[u8], name: &str) -> bool {
    let mut c = field.chunks_exact(2);
    for b in name.bytes() {
        match c.next() {
            Some(u) if u[0] == b && u[1] == 0 => {}
            _ => return false,
        }
    }
    // 名字后必须紧跟 \0 终止，且余下全 0
    match c.next() {
        Some(u) if u[0] == 0 && u[1] == 0 => {}
        _ => return false,
    }
    c.all(|u| u[0] == 0 && u[1] == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, start: u64, end: u64) -> [u8; 128] {
        let mut e = [0u8; 128];
        e[32..40].copy_from_slice(&start.to_le_bytes());
        e[40..48].copy_from_slice(&end.to_le_bytes());
        for (i, u) in name.encode_utf16().enumerate() {
            e[56 + i * 2..58 + i * 2].copy_from_slice(&u.to_le_bytes());
        }
        e
    }

    #[test]
    fn finds_target_and_rejects_neighbors() {
        let mut array = [0u8; 4096];
        array[..128].copy_from_slice(&entry("boot_b", 33288, 40000));
        array[128..256].copy_from_slice(&entry("boot_a", 8712, 33287));
        array[256..384].copy_from_slice(&entry("boot_ab", 1, 2)); // 前缀陷阱
        let p = find(&array, 3, "boot_a").unwrap();
        assert_eq!((p.start, p.end), (8712, 33287));
        assert!(find(&array, 3, "boot").is_none()); // 短名不命中长名槽
        assert!(find(&array, 1, "boot_a").is_none()); // 条目数截断（只看 boot_b）
    }
}
