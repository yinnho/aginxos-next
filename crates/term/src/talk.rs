// Talk — the home face. Voice in. Hold anywhere to speak.
// The reply renders in aginxbrowser (term POSTs /open and yields the panel).
// No keyboard. 刀C: below the hint line, the cards band lists the scheduled
// products ({home}/cards); holding any non-Talk face drops a dialog from
// the top (those faces have no transcript of their own).

use crate::cards::Card;
use crate::draw_centered;
use crate::draw_text;
use crate::fill_rect;
use crate::home::HOME_BG;
use crate::text_w;
use crate::WHITE;

const INK: u32 = WHITE;
const DIM: u32 = 0x009AA09C;
const WAIT_R: i32 = 260;
const RESULT_R: i32 = 110;
const TEXT_SCALE: usize = 6;
/// 新造小兽 SCIS：主体白、角 #EB3300。近黑屏上黑描边会消失，
/// 轮廓用一圈浅灰，眼睛仍是黑的。无嘴、无耳。


#[derive(Clone, Copy, Debug)]
pub struct Disc {
    pub cx: i32,
    pub cy: i32,
    pub r: i32,
}

pub fn wait_disc(w: usize, h: usize) -> Disc {
    Disc {
        cx: w as i32 / 2,
        cy: (h as i32) * 52 / 100,
        r: WAIT_R,
    }
}

pub fn result_disc(w: usize, h: usize) -> Disc {
    Disc {
        cx: w as i32 / 2,
        cy: h as i32 - 240,
        r: RESULT_R,
    }
}

pub fn disc_hit(w: usize, h: usize, x: usize, y: usize, result: bool) -> bool {
    let d = if result { result_disc(w, h) } else { wait_disc(w, h) };
    let dx = x as i32 - d.cx;
    let dy = y as i32 - d.cy;
    dx * dx + dy * dy <= (d.r + 28) * (d.r + 28)
}

#[allow(dead_code)]
pub fn back_hit(w: usize, x: usize, y: usize) -> bool {
    let _ = (w, x, y);
    false
}

/// Home is the Talk face: hold anywhere to speak.
pub fn hold_hit(w: usize, x: usize, y: usize) -> bool {
    let _ = (w, x, y);
    true
}

/// 刀1 让位期右划回主页的净位移阈值（屏幕像素，约面板宽的 15%）。
pub const SWIPE_HOME_PX: isize = 160;

/// 刀1 让位期右划判据：净位移向右过阈值，且横向占优（竖向滚动不误触）。
/// 左划不触发（那是往后翻页的方向，归刀2 的 pager）。
pub fn swipe_home_hit(dx: isize, dy: isize) -> bool {
    dx > SWIPE_HOME_PX && dx > dy.abs() * 2
}



fn keyed(a: u32, b: u32) -> bool {
    let d = |sh: u32| {
        let ca = (a >> sh) & 0xff;
        let cb = (b >> sh) & 0xff;
        (ca as i32 - cb as i32).unsigned_abs()
    };
    d(16) < 36 && d(8) < 36 && d(0) < 36
}

fn beast_frames() -> &'static [aginx_img::Bitmap; 3] {
    use std::sync::OnceLock;
    static FRAMES: OnceLock<[aginx_img::Bitmap; 3]> = OnceLock::new();
    FRAMES.get_or_init(|| {
        let dec = |bytes: &[u8]| {
            aginx_img::decode_scaled(bytes, 640, 700).expect("beast frame")
        };
        [
            dec(include_bytes!("../assets/beast-idle.jpg")),
            dec(include_bytes!("../assets/beast-blink.jpg")),
            dec(include_bytes!("../assets/beast-hold.jpg")),
        ]
    })
}

fn blit_beast(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    bm: &aginx_img::Bitmap,
    cx: i32,
    cy: i32,
    dest_h: i32,
) {
    if bm.w == 0 || bm.h == 0 || dest_h <= 0 {
        return;
    }
    let dest_w = dest_h * bm.w as i32 / bm.h as i32;
    let x0 = cx - dest_w / 2;
    let y0 = cy - dest_h / 2;
    let key = bm.pix[0] & 0x00ff_ffff;
    for dy in 0..dest_h {
        let sy = dy * bm.h as i32 / dest_h;
        let py = y0 + dy;
        if py < 0 || py >= h as i32 {
            continue;
        }
        for dx in 0..dest_w {
            let sx = dx * bm.w as i32 / dest_w;
            let px = x0 + dx;
            if px < 0 || px >= w as i32 {
                continue;
            }
            let c = bm.pix[sy as usize * bm.w as usize + sx as usize] & 0x00ff_ffff;
            if keyed(c, key) {
                continue;
            }
            pix[py as usize * pitch + px as usize] = c;
        }
    }
}

/// 屏上的小兽来自 SCIS 设计稿的图生图帧，不再用几何圆去描。
fn paint_beast(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    d: Disc,
    holding: bool,
    breath: u8,
    thinking: bool,
) {
    let frames = beast_frames();
    let idx = if holding {
        2
    } else if !thinking && breath <= 1 {
        1
    } else {
        0
    };
    let bob = if holding {
        0
    } else {
        (breath as i32 - 8) * (d.r / 80).max(1)
    };
    let idle_h = (d.r * 2 * 96 / 100).max(48);
    // 跑姿比坐着略大，但底边必须停在提示字上方。
    let mut dest_h = if holding { idle_h * 112 / 100 } else { idle_h };
    let mut cy = d.cy + bob / 4;
    if holding {
        let text_top = d.cy + d.r + 40;
        let room = (text_top - 24).max(48);
        if dest_h > room {
            dest_h = room;
        }
        let bottom = cy + dest_h / 2;
        if bottom > text_top {
            cy -= bottom - text_top;
        }
    }
    blit_beast(pix, pitch, w, h, &frames[idx], d.cx, cy, dest_h);
}


/// Waiting / listening / thinking — full canvas.
pub fn paint_wait(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    font: &[[u8; 8]; 128],
    breath: u8,
    holding: bool,
    thinking: bool,
    caption: Option<&str>,
    hint: &str,
) {
    fill_rect(pix, pitch, w, h, 0, 0, w as i32, h as i32, HOME_BG);
    let d = wait_disc(w, h);
    if let Some(c) = caption {
        if !c.is_empty() {
            let y = d.cy - d.r - 100;
            draw_centered(pix, pitch, w, h, font, y, c, TEXT_SCALE, DIM);
        }
    }
    paint_beast(pix, pitch, w, h, d, holding, breath, thinking);
    draw_centered(
        pix,
        pitch,
        w,
        h,
        font,
        d.cy + d.r + 48,
        hint,
        TEXT_SCALE,
        INK,
    );
}

/// Gemini eye on the same face: viewfinder already fills the panel;
/// keep a small orb so the OS language does not become a camera app.
pub fn paint_eye_chrome(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    font: &[[u8; 8]; 128],
    holding: bool,
    breath: u8,
) {
    let d = result_disc(w, h);
    paint_beast(pix, pitch, w, h, d, holding, breath, false);
    draw_centered(
        pix,
        pitch,
        w,
        h,
        font,
        d.cy - d.r - 72,
        "镜头开着",
        TEXT_SCALE,
        INK,
    );
}

pub fn hint(voice_alive: bool, holding: bool, thinking: bool) -> &'static str {
    if !voice_alive {
        "语音还没起来"
    } else if holding {
        "正在听"
    } else if thinking {
        "正在想"
    } else {
        "按住屏幕说话"
    }
}

// ---- 刀C 卡片带 + 按住对话框 ----

pub const CARD_SIDE: i32 = 90; // 侧边距（launch Geom m 同款）
pub const CARD_ROW_H: i32 = 120;
pub const CARD_GAP: i32 = 24;
/// 行底色：键帽同一档的暗绿——黑底绿白字惯例，HOME_BG 上一档可辨。
const CARD_ROW_BG: u32 = 0x000A1410;

/// 带顶：hint 文字行（d.cy+d.r+48 起、8*TEXT_SCALE 高）底下留 36px。
pub fn cards_top(w: usize, h: usize) -> i32 {
    let d = wait_disc(w, h);
    d.cy + d.r + 48 + (8 * TEXT_SCALE) as i32 + 36
}

pub fn cards_bottom(h: usize) -> i32 {
    h as i32 - 36
}

pub fn cards_hit(w: usize, h: usize, y: usize) -> bool {
    let y = y as i32;
    y >= cards_top(w, h) && y < cards_bottom(h)
}

/// 带内 y → 列表行号（带滚动偏移）。行间缝与带外都 None。
pub fn card_row_at(w: usize, h: usize, scroll: usize, y: usize) -> Option<usize> {
    if !cards_hit(w, h, y) {
        return None;
    }
    let rel = y as i32 - cards_top(w, h) + scroll as i32;
    let stride = CARD_ROW_H + CARD_GAP;
    if rel < 0 {
        return None;
    }
    if rel % stride >= CARD_ROW_H {
        return None; // 行间缝
    }
    Some((rel / stride) as usize)
}

/// 列表内容不足一带时没得滚（saturating，卡片空列也是 0）。
pub fn cards_max_scroll(w: usize, h: usize, n: usize) -> usize {
    let band = (cards_bottom(h) - cards_top(w, h)).max(0) as usize;
    let total = n * (CARD_ROW_H + CARD_GAP) as usize;
    total.saturating_sub(band + CARD_GAP as usize)
}

/// 截到带内可显示宽（超出裁掉——draw_text 不裁带，宁可截行不越带）。
fn clip_to_width(s: &str, scale: usize, max_w: i32) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        let mut t = out.clone();
        t.push(ch);
        if text_w(&t, scale) as i32 > max_w {
            break;
        }
        out = t;
    }
    out
}

/// 卡片带：hint 下方到底边的横条小框列表。部分可见行只铺底色（文字
/// 不越带），错误行钉在带顶（4s 自消，main 侧计时）。
pub fn paint_cards(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    font: &[[u8; 8]; 128],
    cards: &[Card],
    scroll: usize,
    err: Option<&str>,
) {
    let top = cards_top(w, h);
    let bottom = cards_bottom(h);
    let stride = CARD_ROW_H + CARD_GAP;
    let x0 = CARD_SIDE;
    let x1 = w as i32 - CARD_SIDE;
    let mut i = (scroll as i32) / stride;
    let mut row_y = top + i * stride - (scroll as i32 % stride);
    while row_y < bottom && i < cards.len() as i32 {
        let c = &cards[i as usize];
        let y1 = row_y + CARD_ROW_H;
        if y1 > top && row_y < bottom {
            fill_rect(
                pix,
                pitch,
                w,
                h,
                x0,
                row_y.max(top),
                x1 - x0,
                y1.min(bottom) - row_y.max(top),
                CARD_ROW_BG,
            );
            if row_y >= top && y1 <= bottom {
                let title = clip_to_width(&c.title, 5, x1 - x0 - 48);
                let _ = draw_text(pix, pitch, w, h, font, x0 + 24, row_y + 16, &title, 5, INK);
                let meta = if c.source.is_empty() {
                    c.template.clone()
                } else {
                    format!("{} · {}", c.source, c.template)
                };
                let meta = clip_to_width(&meta, 3, x1 - x0 - 48);
                let _ = draw_text(pix, pitch, w, h, font, x0 + 24, row_y + 76, &meta, 3, DIM);
            }
        }
        i += 1;
        row_y += stride;
    }
    if let Some(e) = err {
        fill_rect(pix, pitch, w, h, x0, top, x1 - x0, 64, crate::WARN_RED);
        let e = clip_to_width(e, 3, x1 - x0 - 32);
        let _ = draw_text(pix, pitch, w, h, font, x0 + 16, top + 16, &e, 3, WHITE);
    }
}

/// 按住任意面时顶部落下的对话框（只画在非 Talk 面上）：hint 行 + 当前
/// 识别句/回复行（活更新）。黑底绿白字，底边一条绿线把对话框和下面
/// 的面分开。
pub fn paint_dialog(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    font: &[[u8; 8]; 128],
    hint: &str,
    line: Option<&str>,
) {
    const DLG_H: i32 = 300;
    fill_rect(pix, pitch, w, h, 0, 0, w as i32, DLG_H, HOME_BG);
    fill_rect(pix, pitch, w, h, 0, DLG_H - 6, w as i32, 6, crate::GREEN);
    let _ = draw_text(pix, pitch, w, h, font, CARD_SIDE, 64, hint, TEXT_SCALE, INK);
    if let Some(l) = line.map(str::trim).filter(|s| !s.is_empty()) {
        let l = clip_to_width(l, TEXT_SCALE, w as i32 - 2 * CARD_SIDE);
        let _ = draw_text(pix, pitch, w, h, font, CARD_SIDE, 160, &l, TEXT_SCALE, DIM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font;

    fn panel() -> (usize, usize) {
        (1080usize, 2280usize) // D14-exempt: fixture panel geometry
    }

    #[test]
    fn hint_hold_think_and_idle() {
        assert_eq!(hint(false, false, false), "语音还没起来");
        assert_eq!(hint(true, true, false), "正在听");
        assert_eq!(hint(true, false, true), "正在想");
        assert_eq!(hint(true, false, false), "按住屏幕说话");
    }

    #[test]
    fn swipe_home_gesture() {
        assert!(swipe_home_hit(200, 20)); // 清晰右划
        assert!(swipe_home_hit(SWIPE_HOME_PX + 1, 0)); // 刚过阈值
        assert!(!swipe_home_hit(SWIPE_HOME_PX, 0)); // 阈值本身不触发
        assert!(!swipe_home_hit(100, 0)); // 太短（页面内小拖动）
        assert!(!swipe_home_hit(-300, 0)); // 左划 = 翻页方向（刀2）
        assert!(!swipe_home_hit(300, 200)); // 斜拖，竖滚主导
        assert!(!swipe_home_hit(300, -250)); // 斜拖，向上滚主导
    }

    #[test]
    fn wait_disc_is_centerish_and_hittable() {
        let (w, h) = panel();
        let d = wait_disc(w, h);
        assert_eq!(d.cx, w as i32 / 2);
        assert!(disc_hit(w, h, d.cx as usize, d.cy as usize, false));
        assert!(hold_hit(w, 10, 200));
        assert!(hold_hit(w, 800, 20));
        assert!(hold_hit(w, d.cx as usize, d.cy as usize));
        assert!(hold_hit(w, 50, 30));
        assert!(!back_hit(w, 50, 30));
        assert!(!back_hit(w, 50, 200));
    }

    #[test]
    fn result_disc_sits_in_the_bottom_band() {
        let (w, h) = panel();
        let d = result_disc(w, h);
        assert!(d.cy > h as i32 * 3 / 4);
        assert!(disc_hit(w, h, d.cx as usize, d.cy as usize, true));
        assert!(!disc_hit(w, h, d.cx as usize, d.cy as usize, false));
    }

    // ---- 刀C 卡片带 ----

    #[test]
    fn cards_band_sits_below_hint_and_maps_rows() {
        let (w, h) = panel();
        let top = cards_top(w, h);
        let d = wait_disc(w, h);
        assert!(top > d.cy + d.r + 48, "band starts below the hint line");
        assert!(cards_bottom(h) < h as i32);
        assert!(cards_hit(w, h, (top + 10) as usize));
        assert!(!cards_hit(w, h, (top - 1) as usize));
        // 行 0 在带顶；行间缝不算；缝后是行 1
        assert_eq!(card_row_at(w, h, 0, (top + 10) as usize), Some(0));
        assert_eq!(card_row_at(w, h, 0, (top + CARD_ROW_H + 5) as usize), None);
        assert_eq!(
            card_row_at(w, h, 0, (top + CARD_ROW_H + CARD_GAP + 5) as usize),
            Some(1)
        );
        // 滚一整行后同一 y 指向下一行
        let stride = (CARD_ROW_H + CARD_GAP) as usize;
        assert_eq!(card_row_at(w, h, stride, (top + 10) as usize), Some(1));
        // 列表短于一带 → 没得滚
        assert_eq!(cards_max_scroll(w, h, 1), 0);
        assert_eq!(cards_max_scroll(w, h, 0), 0);
        assert!(cards_max_scroll(w, h, 12) > 0);
    }

    #[test]
    fn cards_paint_rows_with_error_pin() {
        let font = font::font_init();
        let (w, h) = panel();
        let mut pix = vec![0u32; w * h];
        let cards = vec![
            Card {
                path: "/tmp/a.json".into(),
                title: "晨报".into(),
                template: "morning".into(),
                source: "cron".into(),
                session: "".into(),
            },
            Card {
                path: "/tmp/b.json".into(),
                title: "出行".into(),
                template: "trip".into(),
                source: "cron".into(),
                session: "me".into(),
            },
        ];
        paint_cards(&mut pix, w, w, h, &font, &cards, 0, None);
        let top = cards_top(w, h) as usize;
        assert_ne!(pix[top * w + w / 2], HOME_BG, "row 0 painted");
        let row2 = top + (CARD_ROW_H + CARD_GAP) as usize;
        assert_ne!(pix[row2 * w + w / 2], HOME_BG, "row 1 painted");
        // 错误行钉在带顶
        paint_cards(&mut pix, w, w, h, &font, &cards, 0, Some("浏览器拒了：404"));
        assert_ne!(pix[(top + 4) * w + w / 2], HOME_BG);
    }

    /// 按住对话框：黑底 + 底边绿线 + 两行字；空 line 只画 hint。
    #[test]
    fn dialog_paints_hint_and_line() {
        let font = font::font_init();
        let (w, h) = panel();
        let mut pix = vec![0u32; w * h];
        paint_dialog(&mut pix, w, w, h, &font, "正在听", Some("帮我看下今天下午的日程"));
        let at = |p: &[u32], x: usize, y: usize| p[y * w + x];
        assert_eq!(at(&pix, w / 2, 20), HOME_BG, "black top");
        assert_eq!(at(&pix, w / 2, 297), crate::GREEN, "green divider (rows 294..300)");
        assert_eq!(at(&pix, w / 2, 300), 0, "first row below the bar untouched");
        // 单像素探针会落在笔画间隙——hint 带整行扫一遍找墨
        assert!(
            (0..w).any(|x| at(&pix, x, 72) != HOME_BG),
            "hint ink present"
        );
        assert!(
            (0..w).any(|x| at(&pix, x, 170) != HOME_BG),
            "recognized line ink present"
        );
        paint_dialog(&mut pix, w, w, h, &font, "正在听", None);
        assert_eq!(at(&pix, w / 2, 297), crate::GREEN);
    }

    #[test]
    fn wait_face_paints_orb() {
        let font = font::font_init();
        let (w, h) = panel();
        let mut pix = vec![0u32; w * h];
        paint_wait(&mut pix, w, w, h, &font, 8, false, false, None, "按住屏幕说话");
        let d = wait_disc(w, h);
        let mut red = false;
        let mut white = false;
        let x0 = (d.cx - d.r).max(0) as usize;
        let x1 = (d.cx + d.r).min(w as i32 - 1) as usize;
        let y0 = (d.cy - d.r).max(0) as usize;
        let y1 = (d.cy + d.r).min(h as i32 - 1) as usize;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = pix[y * w + x];
                let (r, g, b) = ((p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff);
                if r > 170 && g < 100 && b < 100 {
                    red = true;
                }
                if r > 240 && g > 240 && b > 240 {
                    white = true;
                }
            }
        }
        assert!(red && white, "design sprite: red horns and white face");
        assert_ne!(pix[d.cy as usize * w + d.cx as usize], HOME_BG);
        assert!(d.r >= 200);
        assert_eq!(hint(false, false, false), "语音还没起来");
        assert_eq!(hint(true, true, false), "正在听");
        assert_eq!(hint(true, false, true), "正在想");
    }
}
