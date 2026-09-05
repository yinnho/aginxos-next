// agimg — JPEG decode for aterm's photo viewer (M39).
//
// SM7250 has no hardware JPEG *decoder* (camss cam_jpeg is camera-pipeline
// encode-only, Venus has no JPEG capability — probed from the vendor module
// inventory 2026-09-03), and stock Android on this very device decodes
// photos on the CPU too. So this is libjpeg-turbo — NEON intrinsics, the
// same library normal phones use — vendored at ../vendor and built by
// build.rs. The decode side is what a photo-open needs; capture-side
// hardware JPEG encode (camss cam_jpeg) is separate M19-line work.
//
// Layout: all jpeglib struct knowledge stays in C (agimg_shim.c); the FFI
// boundary is one function returning malloc'd XRGB pixels. DCT-scaled
// decompression picks the largest 1/N scale fitting the caller's box, so a
// 12MP shot decodes straight to screen size instead of decoding full and
// downscaling after.

use std::os::raw::{c_uchar, c_uint, c_ulong, c_void};

extern "C" {
    fn agimg_decode(
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
/// the exact layout of aterm's DRM dumb-buffer framebuffers, so the viewer
/// blits without conversion.
pub struct Bitmap {
    pub w: u32,
    pub h: u32,
    pub pix: Vec<u32>,
}

/// Decode `jpeg`, requesting the largest decoder scale whose output fits in
/// `max_w`×`max_h` (aspect preserved — the bitmap may be smaller than the
/// box; center it). Returns None on corrupt input or decoder error.
pub fn decode_scaled(jpeg: &[u8], max_w: u32, max_h: u32) -> Option<Bitmap> {
    let (mut w, mut h) = (0u32, 0u32);
    let p = unsafe {
        agimg_decode(
            jpeg.as_ptr(),
            jpeg.len() as c_ulong,
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
}
