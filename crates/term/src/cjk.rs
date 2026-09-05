// cjk — wide-glyph (Chinese/emoji) rendering for aginx-term (M38a).
//
// The 5x8 bitmap font in font.rs stays the ASCII fast path and the boot
// fallback; this module rasterizes codepoints that need more than 8 rows
// through ab_glyph against a Noto Sans Mono CJK SC subset baked into the
// rootfs at /usr/share/fonts/agterm-cjk.otf (OFL; produced on the host by
// scripts/subset-cjk-font.sh). If that file is missing — recovery boot,
// adb-pushed aginx-term without the rootfs — the terminal degrades to today's
// '?' fallback for CJK instead of failing to start.
//
// ab_glyph has no hinting; at terminal cell sizes (>= ~24 px) rasterizing
// at the exact pixel size and alpha-blending the coverage is enough
// (docs/DEVICE.md 软件侧 #3). Coverage is cached per (char, px) — the
// grid redraws cells constantly, rasterizing every frame would be silly.
// Cap the cache so a pathological scrollback of unique glyphs can't grow
// it without bound.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use ab_glyph::{Font, FontArc, Glyph, Point, PxScale};

const FONT_PATH: &str = "/usr/share/fonts/agterm-cjk.otf";
const CACHE_CAP: usize = 1024;

/// Override for the host/adb dev loop (`aginx-term --ppm`, pre-rebake pushes):
/// AGINX_TERM_CJK_FONT=/path/to/agterm-cjk.otf. On device the baked rootfs path
/// above is the truth.
fn font_path() -> String {
    std::env::var("AGINX_TERM_CJK_FONT").unwrap_or_else(|_| FONT_PATH.to_string())
}

/// Terminal cell width of `ch` in 6x8-cell units: 2 for CJK / fullwidth
/// forms / most emoji, 0 for combining marks, 1 otherwise. Hand-rolled
/// range table — the standard `unicode-width` rules minus the edge cases
/// a phone terminal does not meet (no BiDi, no grapheme joining).
pub fn char_width(ch: char) -> usize {
    let c = ch as u32;
    if c == 0 {
        return 0;
    }
    match c {
        0x0300..=0x036F | 0x200B..=0x200F | 0xFE00..=0xFE0F => 0,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F680..=0x1F6FF
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

fn font() -> Option<&'static FontArc> {
    static FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    FONT.get_or_init(|| std::fs::read(font_path()).ok().and_then(|b| FontArc::try_from_vec(b).ok()))
        .as_ref()
}

#[derive(Clone)]
struct Raster {
    w: usize,
    h: usize,
    cov: Vec<u8>,
}

static CACHE: OnceLock<Mutex<HashMap<(char, u32), Raster>>> = OnceLock::new();

fn raster(ch: char, px: f32) -> Option<Raster> {
    let font = font()?;
    let gid = font.glyph_id(ch);
    if gid == ab_glyph::GlyphId(0) {
        return None; // not in the subset — caller falls back to '?'
    }
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (ch, px.to_bits());
    if let Some(r) = cache.lock().unwrap().get(&key) {
        return Some(r.clone());
    }
    // ab_glyph's PxScale is pixels of LINE HEIGHT (ascent+descent+gap), not
    // per-em (see Font::pt_to_px_scale). This font's line is 1.448 em tall, so
    // an unadjusted PxScale renders glyphs ~0.69x. Convert our em-sized `px`
    // into the line-height space; the baseline offset stays ascent/em-scaled.
    let upem = font.units_per_em().unwrap_or(1000.0);
    let line_h = font.height_unscaled();
    let scale = px * line_h / upem;
    let ascent = font.ascent_unscaled() * px / upem;
    let glyph = Glyph {
        id: gid,
        scale: PxScale { x: scale, y: scale },
        position: Point { x: 0.0, y: ascent },
    };
    let og = font.outline_glyph(glyph)?;
    // og.draw emits (x, y) already relative to px_bounds.min (the rasterizer
    // is sized to the bounds); size cov from the bounds and index directly.
    let b = og.px_bounds();
    let rw = b.width().max(0.0) as usize;
    let rh = b.height().max(0.0) as usize;
    let mut cov = vec![0u8; rw * rh];
    og.draw(|x, y, c| {
        if (x as usize) < rw && (y as usize) < rh {
            cov[y as usize * rw + x as usize] = (c.clamp(0.0, 1.0) * 255.0) as u8;
        }
    });
    let r = Raster { w: rw, h: rh, cov };
    let mut cache = cache.lock().unwrap();
    if cache.len() >= CACHE_CAP {
        cache.clear(); // simple bound; glyph mixes are small and re-rasterize cheap
    }
    cache.insert(key, r.clone());
    Some(r)
}

/// Blit `ch` into the cell box (x..x+box_w, y..y+box_h), ink centered in
/// the box, alpha-blended over whatever is already there (the row bg).
/// Returns false when the glyph can't render (no font / not in subset) —
/// the caller then draws the bitmap '?' fallback.
pub fn draw(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    x: i32,
    y: i32,
    box_w: usize,
    box_h: usize,
    px: f32,
    ch: char,
    color: u32,
) -> bool {
    let Some(r) = raster(ch, px) else {
        return false;
    };
    let dx = x + (box_w as i32 - r.w as i32) / 2;
    let dy = y + (box_h as i32 - r.h as i32) / 2;
    let (fr, fg, fb) = ((color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF);
    for j in 0..r.h {
        let py = dy + j as i32;
        if py < 0 || py >= h as i32 {
            continue;
        }
        for i in 0..r.w {
            let pxx = dx + i as i32;
            if pxx < 0 || pxx >= w as i32 {
                continue;
            }
            let a = r.cov[j * r.w + i] as u32;
            if a == 0 {
                continue;
            }
            let idx = py as usize * pitch + pxx as usize;
            let p = pix[idx];
            let (pr, pg, pb) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
            let mix = |f: u32, b: u32| (b + ((f - b) * a) / 255) & 0xFF;
            pix[idx] = 0x0000_0000 | (mix(fr, pr) << 16) | (mix(fg, pg) << 8) | mix(fb, pb);
        }
    }
    true
}
