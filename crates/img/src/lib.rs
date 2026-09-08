// aginx-img — JPEG decode for aginx-term's photo viewer (M39).
//
// SM7250 has no hardware JPEG *decoder* (camss cam_jpeg is camera-pipeline
// encode-only, Venus has no JPEG capability — probed from the vendor module
// inventory 2026-09-03), and stock Android on this very device decodes
// photos on the CPU too. So this is libjpeg-turbo — NEON intrinsics, the
// same library normal phones use — vendored at ../vendor and built by
// build.rs. The decode side is what a photo-open needs; capture-side
// hardware JPEG encode (camss cam_jpeg) is separate M19-line work.
//
// Layout: all jpeglib struct knowledge stays in C (img_shim.c); the FFI
// boundary is one function returning malloc'd XRGB pixels. DCT-scaled
// decompression picks the largest 1/N scale fitting the caller's box, so a
// 12MP shot decodes straight to screen size instead of decoding full and
// downscaling after.

use std::io::Cursor;
use std::os::raw::{c_uchar, c_uint, c_ulong, c_void};

extern "C" {
    fn aginx_img_decode(
        data: *const c_uchar,
        len: c_ulong,
        max_w: c_uint,
        max_h: c_uint,
        out_w: *mut c_uint,
        out_h: *mut c_uint,
    ) -> *mut c_uint;
    fn free(p: *mut c_void);
}

/// Decoded image: XRGB8888 pixels (0x00RRGGBB), row-major, `w*h` entries —
/// the exact layout of aginx-term's DRM dumb-buffer framebuffers, so the viewer
/// blits without conversion.
pub struct Bitmap {
    pub w: u32,
    pub h: u32,
    pub pix: Vec<u32>,
}

/// Decode `bytes` (JPEG or PNG — sniffed by magic, not by file name),
/// requesting the largest decoder scale whose output fits in `max_w`×`max_h`
/// (aspect preserved — the bitmap may be smaller than the box; center it).
/// Returns None on corrupt input or decoder error.
pub fn decode_scaled(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Bitmap> {
    // aginxbrowser session 截图只出 PNG（engine "Always png for now"）——
    // result.img 的Content 就是它；JPEG 路径保持 libjpeg-turbo 原样。
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return decode_png_scaled(bytes, max_w, max_h);
    }
    let (mut w, mut h) = (0u32, 0u32);
    let p = unsafe {
        aginx_img_decode(
            bytes.as_ptr(),
            bytes.len() as c_ulong,
            max_w,
            max_h,
            &mut w,
            &mut h,
        )
    };
    if p.is_null() || w == 0 || h == 0 {
        return None;
    }
    let n = w as usize * h as usize;
    let pix = unsafe { std::slice::from_raw_parts(p, n) }.to_vec();
    unsafe { free(p.cast()) };
    Some(Bitmap { w, h, pix })
}

/// PNG 路径（纯 Rust png crate）：全尺寸解码 → 与 JPEG 路同合同（不超箱、
/// 保比、可小于箱由调用方居中）。引擎全尺寸截图进同尺寸面板 = n=1 原样，
/// 缩放只为兜底。
fn decode_png_scaled(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Bitmap> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    // EXPAND：调色板/灰度都抬到 RGB(A)8，下面的 match 才只有两形
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let buf_size = reader.output_buffer_size();
    let mut frame = vec![0u8; buf_size];
    let out = reader.next_frame(&mut frame).ok()?;
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    let written = out.buffer_size();
    let pix: Vec<u32> = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgb, png::BitDepth::Eight) => frame[..written]
            .chunks_exact(3)
            .map(|c| 0xFF000000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32)
            .collect(),
        (png::ColorType::Rgba, png::BitDepth::Eight) => frame[..written]
            .chunks_exact(4)
            .map(|c| 0xFF000000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32)
            .collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => frame[..written]
            .iter()
            .map(|g| 0xFF000000 | (*g as u32) << 16 | (*g as u32) << 8 | *g as u32)
            .collect(),
        _ => return None,
    };
    // 最小整数 n 使 1/n 缩放装进箱（n=1 = 原样；同 JPEG「装进箱的最大
    // 输出」精神）。引擎截图进同尺寸面板永远 n=1，缩放只为兜底。
    let mut n = 1u32;
    while (w / n > max_w || h / n > max_h) && (w / n > 1 || h / n > 1) {
        n += 1;
    }
    if n == 1 {
        return Some(Bitmap { w, h, pix });
    }
    let (nw, nh) = (w / n, h / n);
    let mut out_pix = Vec::with_capacity(nw as usize * nh as usize);
    for y in 0..nh {
        for x in 0..nw {
            out_pix.push(pix[(y * n) as usize * w as usize + (x * n) as usize]);
        }
    }
    Some(Bitmap {
        w: nw,
        h: nh,
        pix: out_pix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIX: &[u8] = include_bytes!("../tests/fixtures/grad.jpg");

    #[test]
    fn full_size_decode() {
        let b = decode_scaled(&FIX[..], 64, 48).expect("decode");
        assert_eq!((b.w, b.h), (64, 48));
        assert_eq!(b.pix.len(), 64 * 48);
    }

    #[test]
    fn dct_scaled_decode() {
        // 64x48 through a 16x12 box = exactly the 1/4 DCT scale
        let b = decode_scaled(&FIX[..], 16, 16).expect("decode");
        assert_eq!((b.w, b.h), (16, 12));
    }

    #[test]
    fn gradient_survives_round_trip() {
        // fixture is a horizontal blue→red gradient; JPEG is lossy, so the
        // ends only need to land in the right quarter of the cube. (The X
        // byte of JCS_EXT_BGRX is undefined — 0xff here — and DRM XRGB8888
        // scanout ignores it, so we only ever mask RGB out of the u32.)
        let b = decode_scaled(&FIX[..], 64, 48).expect("decode");
        let px = |x: u32, y: u32| b.pix[y as usize * b.w as usize + x as usize];
        let (l, r) = (px(2, 24), px(61, 24));
        assert!(l & 0xFF > 180, "left should be blue-ish: {l:08x}");
        assert!((l >> 16) & 0xFF < 80, "left should not be red: {l:08x}");
        assert!((r >> 16) & 0xFF > 180, "right should be red-ish: {r:08x}");
        assert!(r & 0xFF < 80, "right should not be blue: {r:08x}");
    }

    #[test]
    fn garbage_returns_none() {
        assert!(decode_scaled(b"not a jpeg at all, really", 100, 100).is_none());
        assert!(decode_scaled(&[], 100, 100).is_none());
    }

    fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(rgba).unwrap();
        out
    }

    #[test]
    fn png_round_trip_full_size() {
        // 2x2: 左红右绿
        let png = encode_png(
            2,
            2,
            &[0xFF, 0, 0, 0xFF, 0, 0xFF, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0, 0xFF, 0, 0xFF],
        );
        let b = decode_scaled(&png, 2, 2).expect("png decode");
        assert_eq!((b.w, b.h), (2, 2));
        assert_eq!(b.pix[0], 0xFFFF0000);
        assert_eq!(b.pix[1], 0xFF00FF00);
    }

    #[test]
    fn png_identity_at_panel_size() {
        // 引擎场景：等宽截图进等宽箱 = 原样不缩（尺寸中性——同合同
        // 与面板无关，n=1 恒等路径的覆盖）
        let w = 960u32;
        let h = 8u32;
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for x in 0..w {
            for _ in 0..h {
                rgba.extend_from_slice(&[x as u8, 0, 0, 0xFF]);
            }
        }
        let png = encode_png(w, h, &rgba);
        let b = decode_scaled(&png, w, h * 2).expect("png decode");
        assert_eq!((b.w, b.h), (w, h));
        assert_eq!(b.pix.len(), (w * h) as usize);
    }

    #[test]
    fn png_downscales_to_fit_box() {
        // 100x100 进 50x50 箱 = 1/2 采样
        let mut rgba = vec![0u8; 100 * 100 * 4];
        for y in 0..100usize {
            for x in 0..100usize {
                let i = (y * 100 + x) * 4;
                rgba[i] = 0x11;
                rgba[i + 3] = 0xFF;
            }
        }
        let png = encode_png(100, 100, &rgba);
        let b = decode_scaled(&png, 50, 50).expect("png decode");
        assert_eq!((b.w, b.h), (50, 50));
        assert_eq!(b.pix.len(), 50 * 50);
        assert_eq!(b.pix[0], 0xFF110000);
    }

    #[test]
    fn png_garbage_returns_none() {
        // PNG magic 开头但内容坏 → 不落 JPEG 路，PNG 路自己返回 None
        assert!(decode_scaled(b"\x89PNG\r\n\x1a\n broken body", 100, 100).is_none());
    }
}
