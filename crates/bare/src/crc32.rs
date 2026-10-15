//! CRC32（反射 0xEDB88320，GPT 用）。u4j 设备上算中实证。
//! 全局寄存器律在 Rust 下由编译器接管——踩 x3 一族事故不再可能。

const POLY: u32 = 0xEDB8_8320;

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = !0;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ POLY } else { crc >> 1 };
        }
    }
    !crc
}
