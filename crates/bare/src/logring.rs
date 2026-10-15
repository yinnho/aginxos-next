//! UFS 日志环记录格式（S0-2，NOLINUX-PLAN 规格：魔数/序号/戳/级别/文本）。
//!
//! 每条记录占一个 4096B 块；环形区由设备侧定位（分区名查 + 界检），
//! 本模块只管记录的组包与识别，不含任何机器事实：
//!
//! ```text
//! +0   u64  magic "AGXLOG\0\1"（小端）
//! +8   u64  seq（单调递增，跨启动接续——环扫描取 max+1）
//! +16  u64  stamp（CNTVCT 戳）
//! +24  u32  level
//! +28  u32  text_len
//! +32  text（\0 结尾，最长 4063）
//! ```

/// 级别：0=boot 自检、1=info 心跳、2=warn、3=err。
pub const LVL_BOOT: u32 = 0;
pub const LVL_INFO: u32 = 1;
pub const LVL_WARN: u32 = 2;
pub const LVL_ERR: u32 = 3;

pub const MAGIC: u64 = u64::from_le_bytes(*b"AGXLOG\0\x01");
const TEXT_OFF: usize = 32;

/// 组一条记录进 4096B 块（全清后铺头 + 文本截断拷贝）。
pub fn write_record(buf: &mut [u8], seq: u64, stamp: u64, level: u32, text: &str) {
    buf.fill(0);
    buf[0..8].copy_from_slice(&MAGIC.to_le_bytes());
    buf[8..16].copy_from_slice(&seq.to_le_bytes());
    buf[16..24].copy_from_slice(&stamp.to_le_bytes());
    buf[24..28].copy_from_slice(&level.to_le_bytes());
    let t = text.as_bytes();
    let n = t.len().min(buf.len() - TEXT_OFF - 1);
    buf[TEXT_OFF..TEXT_OFF + n].copy_from_slice(&t[..n]);
    buf[TEXT_OFF + n] = 0;
    let tl = (n + 1) as u32;
    buf[28..32].copy_from_slice(&tl.to_le_bytes());
}

/// 块识别：magic 对则返 (seq, stamp, level)。空块/异魔数/截断头返 None。
/// （文本合法性由解析侧判——扫描只信序号。）
pub fn parse(block: &[u8]) -> Option<(u64, u64, u32)> {
    if block.len() < TEXT_OFF {
        return None;
    }
    let mut m = [0u8; 8];
    m.copy_from_slice(&block[0..8]);
    if u64::from_le_bytes(m) != MAGIC {
        return None;
    }
    let mut seq = [0u8; 8];
    seq.copy_from_slice(&block[8..16]);
    let mut stamp = [0u8; 8];
    stamp.copy_from_slice(&block[16..24]);
    let lvl = block[24] as u32 | (block[25] as u32) << 8
        | (block[26] as u32) << 16 | (block[27] as u32) << 24;
    Some((u64::from_le_bytes(seq), u64::from_le_bytes(stamp), lvl))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = [0u8; 4096];
        write_record(&mut buf, 42, 0x1234_5678, LVL_INFO, "#42 heartbeat 1");
        assert_eq!(parse(&buf), Some((42, 0x1234_5678, LVL_INFO)));
        assert_eq!(&buf[32..47], b"#42 heartbeat 1");
        assert_eq!(buf[47], 0);
    }

    #[test]
    fn foreign_and_empty_blocks_rejected() {
        assert_eq!(parse(&[0u8; 4096]), None); // 全零块（首刷态）
        let mut u5a = [0u8; 4096]; // 遗留图样 AGXWRT01——非环魔数不扰扫描
        u5a[..8].copy_from_slice(b"AGXWRT01");
        assert_eq!(parse(&u5a), None);
        assert_eq!(parse(&[0u8; 16]), None); // 截断头
    }

    #[test]
    fn text_truncated_and_zero_terminated() {
        let mut buf = [0u8; 4096];
        let long: String = std::iter::repeat('x').take(5000).collect();
        write_record(&mut buf, 1, 0, LVL_ERR, &long);
        assert_eq!(parse(&buf), Some((1, 0, LVL_ERR)));
        assert!(buf[32..32 + 4063].iter().all(|&b| b == b'x'));
        assert_eq!(buf[32 + 4063], 0); // 尾 \0
    }
}
