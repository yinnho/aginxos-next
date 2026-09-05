//! Host-only fixture maker for the n5 acceptance suite. Generates the
//! deterministic QR image at rootfs/usr/share/aginx/n5-qr.jpg so the
//! device suite can prove the absorbed aginx-qr end-to-end (jpeg decode
//! face included) against a known payload.
//!
//! Run on the host (macOS, needs sips):
//!   N5_WRITE_FIXTURE=1 cargo test -p aginx-qr --test n5-fixture -- --ignored
//!   sips -s format jpeg -s formatOptions 90 rootfs/usr/share/aginx/n5-qr.bmp \
//!        --out rootfs/usr/share/aginx/n5-qr.jpg && rm n5-qr.bmp
//! Then commit the .jpg. Round-trip proof:
//!   cargo run -p aginx-qr -- rootfs/usr/share/aginx/n5-qr.jpg

#![cfg(test)]

use qrcodegen::{QrCode, QrCodeEcc};

/// The payload n5.sh pins byte-for-byte. WIFI: shape on purpose — it is
/// the payload class voice's parse_wifi_payload consumes, so the fixture
/// exercises the real product path, not a toy string.
pub const N5_PAYLOAD: &str = "WIFI:T:WPA;S:aginx-n5;P:fixture;;";

/// 24-bit BMP writer (bottom-up, BGR, rows padded to 4 bytes) — the one
/// lossless format trivially writable by hand; sips turns it into the
/// JPEG the device decoder eats.
fn write_bmp(path: &std::path::Path, modules: &QrCode, scale: i32, quiet: i32) {
    let n = modules.size(); // i32 in qrcodegen 1.x
    let dim = (n + 2 * quiet) * scale;
    let row_bytes = ((dim * 3 + 3) & !3) as usize;
    let data_len = row_bytes * dim as usize;
    let file_len = 54 + data_len;

    let mut out = Vec::with_capacity(file_len);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_len as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(dim as i32).to_le_bytes());
    out.extend_from_slice(&(dim as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for y in (0..dim).rev() {
        for x in 0..dim {
            let my = y / scale - quiet;
            let mx = x / scale - quiet;
            let dark = (0..n).contains(&my) && (0..n).contains(&mx)
                && modules.get_module(mx, my);
            let v: u8 = if dark { 0x00 } else { 0xFF };
            out.extend_from_slice(&[v, v, v]); // BGR, grayscale
        }
        out.extend(std::iter::repeat_n(0u8, row_bytes - (dim * 3) as usize));
    }
    std::fs::write(path, &out).unwrap();
}

#[test]
#[ignore = "host fixture maker — opt in with N5_WRITE_FIXTURE=1"]
fn make_n5_fixture() {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let bmp = std::path::Path::new(&root).join("../../rootfs/usr/share/aginx/n5-qr.bmp");
    if std::env::var_os("N5_WRITE_FIXTURE").is_none() {
        panic!("refusing to write without N5_WRITE_FIXTURE=1 (deterministic committed artifact)");
    }
    let code = QrCode::encode_text(N5_PAYLOAD, QrCodeEcc::Medium).unwrap();
    write_bmp(&bmp, &code, 8, 4);
    eprintln!("wrote {}", bmp.display());
}
