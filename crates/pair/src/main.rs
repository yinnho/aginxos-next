//! aginx-pair — AGINXPAIR1 配对码铸码工具（M42f⑤，host-only）。
//!
//! 一张码 = 全部身份（用户拍板 2026-09-05）：WiFi 进网 + brain 钥匙 +
//! 网关身份 + relay 密。字段校验复用 aginx-qr 的 PairBundle（空段/非
//! ASCII/含 `|` 拒铸）——铸端与解端同一真源，码必可解。
//!
//! 用法：
//!   aginx-pair --ssid S --psk P --brain-key K --gateway-id G \
//!              --relay-secret R -o pair.jpg [--scale N]
//!
//! 输出 JPEG（灰度 q85）而非 PNG：设备端 aginx-qr 只解 JPEG（decode_jpeg
//! 是唯一入口），铸出的文件既能推上设备直解、也能屏显拍摄走同一条解码路。
//!
//! 秘密卫生：五件套永不回显——stdout/stderr 只报文件名、字节量与码尺寸。
//! scale 默认 12 px/module（45 模块的码 ≈ 640px，满窗展示给 M42b 拍屏
//! 配方正合适）。
use std::fs;
use std::io::BufWriter;
use std::process::exit;

use jpeg_encoder::{ColorType, Encoder};
use qrcodegen::QrCode;

const QUIET_MODULES: usize = 4;
const DEFAULT_SCALE: usize = 12;

fn die(msg: &str) -> ! {
    eprintln!("aginx-pair: {msg}");
    exit(1);
}

fn main() {
    let mut ssid = String::new();
    let mut psk = String::new();
    let mut brain_key = String::new();
    let mut gateway_id = String::new();
    let mut relay_secret = String::new();
    let mut out = String::new();
    let mut scale = DEFAULT_SCALE;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        let (flag, val) = match (args[i].as_str(), args.get(i + 1)) {
            (f, Some(v)) => (f, v.clone()),
            (f, None) => die(&format!("{f} needs a value")),
        };
        match flag {
            "--ssid" => ssid = val,
            "--psk" => psk = val,
            "--brain-key" => brain_key = val,
            "--gateway-id" => gateway_id = val,
            "--relay-secret" => relay_secret = val,
            "-o" => out = val,
            "--scale" => {
                scale = val.parse().unwrap_or_else(|_| die("--scale wants a number"));
                if !(4..=40).contains(&scale) {
                    die("--scale must be 4..=40");
                }
            }
            other => die(&format!("unknown flag {other}")),
        }
        i += 2;
    }
    if out.is_empty() {
        eprintln!("usage: aginx-pair --ssid S --psk P --brain-key K --gateway-id G --relay-secret R -o pair.jpg [--scale N]");
        exit(2);
    }

    let bundle = aginx_qr::PairBundle::try_new(
        &ssid,
        &psk,
        &brain_key,
        &gateway_id,
        &relay_secret,
    )
    .unwrap_or_else(|| {
        die("fields must be non-empty ASCII without '|'/newline (empty segment = half an identity)")
    });
    let payload = bundle.payload();
    let bytes = render_jpeg(&payload, scale).unwrap_or_else(|e| die(&e));
    fs::write(&out, &bytes).unwrap_or_else(|e| die(&format!("write {out}: {e}")));

    // 只报形状不报内容：码尺寸让人知道展示该开多大窗，字段一个不出。
    let qr = QrCode::encode_text(&payload, qrcodegen::QrCodeEcc::Medium)
        .unwrap_or_else(|e| die(&format!("qr encode: {e}")));
    println!(
        "wrote {out} ({} bytes, {} modules, payload {} chars, scale {scale})",
        bytes.len(),
        qr.size(),
        payload.len()
    );
}

/// payload → 灰度 JPEG（白底黑码 + 4 module 静区，q85）。
fn render_jpeg(payload: &str, scale: usize) -> Result<Vec<u8>, String> {
    let qr = QrCode::encode_text(payload, qrcodegen::QrCodeEcc::Medium)
        .map_err(|e| format!("qr encode: {e}"))?;
    let size = qr.size() as usize;
    let dim = (size + QUIET_MODULES * 2) * scale;
    let mut img = vec![255u8; dim * dim];
    for y in 0..size {
        for x in 0..size {
            if qr.get_module(x as i32, y as i32) {
                for dy in 0..scale {
                    let py = (y + QUIET_MODULES) * scale + dy;
                    for dx in 0..scale {
                        img[py * dim + (x + QUIET_MODULES) * scale + dx] = 0;
                    }
                }
            }
        }
    }
    let mut buf = Vec::new();
    let enc = Encoder::new(BufWriter::new(&mut buf), 85);
    enc.encode(&img, dim as u16, dim as u16, ColorType::Luma)
        .map_err(|e| format!("jpeg encode: {e}"))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SSID: &str = "Legrand AP";
    const PSK: &str = "p4ss w0rd!";
    const KEY: &str = "sk-1234567890abcdef1234567890abcdef";
    const GW: &str = "cf49973e";
    const SEC: &str = "relay-secret-9f8e7d6c";

    fn bundle() -> aginx_qr::PairBundle {
        aginx_qr::PairBundle::try_new(SSID, PSK, KEY, GW, SEC).unwrap()
    }

    #[test]
    fn payload_roundtrips_through_device_parser() {
        let p = bundle().payload();
        let back = aginx_qr::parse_pair_payload(&p).expect("device-side parse");
        assert_eq!(back, bundle());
        assert!(p.starts_with("AGINXPAIR1|"));
    }

    #[test]
    fn rejects_bad_fields() {
        // 与设备解端同一法：空段/非 ASCII/分隔符拒收
        assert!(aginx_qr::PairBundle::try_new("", PSK, KEY, GW, SEC).is_none());
        assert!(aginx_qr::PairBundle::try_new("局域网", PSK, KEY, GW, SEC).is_none());
        assert!(aginx_qr::PairBundle::try_new("a|b", PSK, KEY, GW, SEC).is_none());
        assert!(aginx_qr::PairBundle::try_new("ok\n", PSK, KEY, GW, SEC).is_none());
    }

    #[test]
    fn jpeg_decodes_back_through_the_device_chain() {
        // 全链：payload → 铸码 JPEG → 设备同款 decode_jpeg → payload → 解析
        let payload = bundle().payload();
        let bytes = render_jpeg(&payload, DEFAULT_SCALE).unwrap();
        assert_eq!(&bytes[..2], b"\xff\xd8"); // SOI
        assert_eq!(&bytes[bytes.len() - 2..], b"\xff\xd9"); // EOI
        let decoded = aginx_qr::decode_jpeg(&bytes).expect("decode own output");
        assert_eq!(decoded, vec![payload.clone()]);
        assert_eq!(aginx_qr::parse_pair_payload(&decoded[0]), Some(bundle()));
    }
}
