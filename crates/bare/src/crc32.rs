//! CRC32（反射 0xEDB88320，GPT 用）。u4j 设备上算中实证。
//! 全局寄存器律在 Rust 下由编译器接管——踩 x3 一族事故不再可能。

const POLY: u32 = 0xEDB8_8320;

pub fn crc32(data: &[u8]) -> u32 {
    crc32_finish(crc32_update(crc32_start(), data))
}

/// 增量三件套（大文件分段算——刀上 64KB 缓冲滚算全文件）。
pub fn crc32_start() -> u32 {
    !0
}

/// 喂一段，返回中间态（外部不可直读——须过 finish）。
pub fn crc32_update(state: u32, data: &[u8]) -> u32 {
    let mut crc = state;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ POLY } else { crc >> 1 };
        }
    }
    crc
}

pub fn crc32_finish(state: u32) -> u32 {
    !state
}

#[cfg(test)]
mod tests {
    #[test]
    fn chained_equals_oneshot() {
        let big: Vec<u8> = (0..100_000u32).map(|i| (i * 7 + 3) as u8).collect();
        let a = super::crc32(&big);
        let mut st = super::crc32_start();
        for ch in big.chunks(977) {
            st = super::crc32_update(st, ch);
        }
        assert_eq!(super::crc32_finish(st), a);
    }
}
