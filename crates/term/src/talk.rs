// Talk — the home face. Voice in. Hold anywhere to speak.
// The reply renders in aginxbrowser (term POSTs /open and yields the panel).
// No keyboard. 刀C: below the hint line, the cards band lists the scheduled
// products ({home}/cards); holding any non-Talk face drops a dialog from
// the top (those faces have no transcript of their own).

use crate::cards::Card;
use crate::draw_centered;
use crate::draw_text;
use crate::fill_rect;
use crate::home::{HOME_BG, WORDMARK, WORDMARK_SCALE};
use crate::MGREEN;
use crate::text_w;
use crate::WHITE;

const INK: u32 = WHITE;
const DIM: u32 = 0x009AA09C;
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

pub fn result_disc(w: usize, h: usize) -> Disc {
    Disc {
        cx: w as i32 / 2,
        cy: h as i32 - 240,
        r: RESULT_R,
    }
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


/// Waiting / listening / thinking — full canvas. 09-24 重设计（方案A 磷光
/// 终端·字标居中）：顶部状态行（呼吸点+名字 | 时钟）、AginxOS 大字标随
/// 呼吸微亮、`> 提示行 + 块光标`、识别句/答复行、底半会话卡带。小兽
/// 从首页退役——取景面的眼睛饰件（paint_eye_chrome）仍用。
pub const PROMPT_SCALE: usize = TEXT_SCALE;
/// 输入输出行（识别句/答复）与提示行同字号——09-24 二修用户裁决。
pub const CAPTION_SCALE: usize = PROMPT_SCALE;

/// 状态行内容：net=呼吸点绿/断网红，time=右上角时钟串。
pub struct StatusLine<'a> {
    pub net: bool,
    pub time: &'a str,
}

/// 呼吸亮度：level 1..16 → ~58%..100% 基色（通道线性压暗）。
fn glow(base: u32, level: u8) -> u32 {
    let f = 55u32 + (level as u32).min(16) * 45 / 16;
    let dim = |c: u32| c * f / 100;
    (dim((base >> 16) & 0xff) << 16) | (dim((base >> 8) & 0xff) << 8) | dim(base & 0xff)
}

/// 首页字标 y：09-24 二修——提到顶部，顶上留一个字标字高的距离（状态
/// 行住在那段），不再压 30% 中腰。
pub fn wordmark_home_y(_h: usize) -> i32 {
    (8 * WORDMARK_SCALE) as i32
}

/// 提示行 y：字标底下再留一个字标字高（09-24 二修）。
pub fn prompt_y(h: usize) -> i32 {
    wordmark_home_y(h) + 2 * (8 * WORDMARK_SCALE) as i32
}

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
    status: &StatusLine<'_>,
) {
    fill_rect(pix, pitch, w, h, 0, 0, w as i32, h as i32, HOME_BG);
    // 状态行：左=呼吸点+名字（断网点钉红），右=时钟
    let dot = if status.net {
        glow(MGREEN, breath)
    } else {
        crate::WARN_RED
    };
    fill_rect(pix, pitch, w, h, CARD_SIDE, 66, 18, 18, dot);
    let _ = draw_text(pix, pitch, w, h, font, CARD_SIDE + 40, 64, "aginx", 3, DIM);
    let tw = text_w(status.time, 3) as i32;
    let _ = draw_text(
        pix,
        pitch,
        w,
        h,
        font,
        w as i32 - CARD_SIDE - tw,
        64,
        status.time,
        3,
        DIM,
    );
    // 磷光字标：随呼吸微亮；按住说话时压暗让位给顶部落下的对话框
    let wm = glow(MGREEN, if holding { 1 } else { breath });
    draw_centered(
        pix,
        pitch,
        w,
        h,
        font,
        wordmark_home_y(h),
        WORDMARK,
        WORDMARK_SCALE,
        wm,
    );
    // 提示行：> {hint} + 终端块光标（呼吸闪烁；听/想时常亮）
    let prompt = format!("> {hint}");
    let py = prompt_y(h);
    let _ = draw_text(pix, pitch, w, h, font, CARD_SIDE, py, &prompt, PROMPT_SCALE, INK);
    if holding || thinking || breath >= 9 {
        let cw = text_w(&prompt, PROMPT_SCALE) as i32;
        fill_rect(
            pix,
            pitch,
            w,
            h,
            CARD_SIDE + cw + 24,
            py + 6,
            26,
            (8 * PROMPT_SCALE) as i32 - 12,
            crate::GREEN,
        );
    }
    // 输入输出行（问句常驻）——钉在卡带上方居中（09-24 二修），超宽截尾
    if let Some(c) = caption.map(str::trim).filter(|s| !s.is_empty()) {
        let c = clip_to_width(c, CAPTION_SCALE, w as i32 - 2 * CARD_SIDE);
        draw_centered(
            pix,
            pitch,
            w,
            h,
            font,
            cards_top(w, h) - (8 * CAPTION_SCALE) as i32 - 48,
            &c,
            CAPTION_SCALE,
            DIM,
        );
    }
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

/// 带顶：半屏起排（09-24 二修）——上半屏留给字标/提示行/输入输出行。
pub fn cards_top(_w: usize, h: usize) -> i32 {
    h as i32 / 2
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
            fill_rect(pix, pitch, w, h, x0, row_y, 6, CARD_ROW_H, crate::GREEN);
            if row_y >= top && y1 <= bottom {
                // 09-24 二修：卡带第一行（问句）与提示行同字号
                let title = clip_to_width(&c.title, PROMPT_SCALE, x1 - x0 - 96);
                let _ = draw_text(pix, pitch, w, h, font, x0 + 30, row_y + 12, &title, PROMPT_SCALE, INK);
                let _ = draw_text(pix, pitch, w, h, font, x1 - 48, row_y + 12, ">", PROMPT_SCALE, DIM);
                let meta = if c.source.is_empty() {
                    c.template.clone()
                } else {
                    format!("{} · {}", c.source, c.template)
                };
                let meta = clip_to_width(&meta, 3, x1 - x0 - 96);
                let _ = draw_text(pix, pitch, w, h, font, x0 + 30, row_y + 76, &meta, 3, DIM);
            }
        }
        i += 1;
        row_y += stride;
    }
    if cards.is_empty() && err.is_none() {
        // 空态：带中央一句自举（按住带内空区也成军说话）
        draw_centered(
            pix,
            pitch,
            w,
            h,
            font,
            (top + bottom) / 2 - 12,
            "按住屏幕任意处，开始一段新对话",
            3,
            DIM,
        );
    }
    if let Some(e) = err {
        fill_rect(pix, pitch, w, h, x0, top, x1 - x0, 64, crate::WARN_RED);
        let e = clip_to_width(e, 3, x1 - x0 - 32);
        let _ = draw_text(pix, pitch, w, h, font, x0 + 16, top + 16, &e, 3, WHITE);
    }
}

/// 按住任意面（含首页/Talk——09-24 解禁）时顶部落下的对话框：hint 行 +
/// 当前识别句/回复行（活更新）。黑底绿白字，底边一条绿线把对话框和下面
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
    fn home_layout_stacks_status_wordmark_prompt_cards() {
        let (w, h) = panel();
        let font_h = (8 * WORDMARK_SCALE) as i32;
        // 09-24 二修：字标提到顶部（顶上留一个字标字高），提示行再隔一个
        // 字标字高，卡带半屏起排
        assert_eq!(wordmark_home_y(h), font_h, "top margin = one wordmark font height");
        assert!(wordmark_home_y(h) > 90, "wordmark clear of the status line");
        assert_eq!(prompt_y(h), wordmark_home_y(h) + 2 * font_h, "prompt one font below the wordmark");
        assert_eq!(cards_top(w, h), h as i32 / 2, "cards start at half screen");
        assert!(cards_top(w, h) > prompt_y(h) + (8 * PROMPT_SCALE) as i32);
        assert!(cards_bottom(h) < h as i32);
        assert!(cards_hit(w, h, (cards_top(w, h) + 10) as usize));
        assert!(!cards_hit(w, h, (cards_top(w, h) - 1) as usize));
        assert!(hold_hit(w, 10, 200));
        assert!(hold_hit(w, 800, 20));
    }

    #[test]
    fn result_disc_sits_in_the_bottom_band() {
        let (w, h) = panel();
        let d = result_disc(w, h);
        assert!(d.cy > h as i32 * 3 / 4);
    }

    // ---- 刀C 卡片带 ----

    #[test]
    fn cards_band_sits_below_hint_and_maps_rows() {
        let (w, h) = panel();
        let top = cards_top(w, h);
        assert!(top > prompt_y(h) + (8 * PROMPT_SCALE) as i32, "band starts below the prompt line");
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
    fn wait_face_paints_wordmark_and_status() {
        let font = font::font_init();
        let (w, h) = panel();
        let mut pix = vec![0u32; w * h];
        paint_wait(
            &mut pix,
            w,
            w,
            h,
            &font,
            8,
            false,
            false,
            None,
            "按住屏幕说话",
            &StatusLine { net: true, time: "9:41" },
        );
        let at = |p: &[u32], x: usize, y: usize| p[y * w + x];
        // 状态点在网=绿色系（G 通道占优），断网=红
        let dot = at(&pix, (CARD_SIDE + 9) as usize, 75);
        let (r, g, _) = ((dot >> 16) & 0xff, (dot >> 8) & 0xff, dot & 0xff);
        assert!(g > r, "net dot greenish, got {dot:08x}");
        // 字标行有磷光绿墨（G 通道远超 R/B）
        let wy = wordmark_home_y(h) as usize + 40;
        assert!(
            (0..w).any(|x| {
                let p = at(&pix, x, wy);
                ((p >> 8) & 0xff) > 90 && ((p >> 16) & 0xff) < 90 && (p & 0xff) < 90
            }),
            "wordmark phosphor ink present"
        );
        // 提示行有白墨；块光标在呼吸亮半程常亮（breath=8 < 9 → 只验提示行）
        let py = prompt_y(h) as usize + 20;
        assert!(
            (0..w).any(|x| at(&pix, x, py) != HOME_BG),
            "prompt ink present"
        );
        // 断网点钉红
        paint_wait(
            &mut pix,
            w,
            w,
            h,
            &font,
            8,
            false,
            false,
            None,
            "按住屏幕说话",
            &StatusLine { net: false, time: "9:41" },
        );
        let dot = at(&pix, (CARD_SIDE + 9) as usize, 75);
        assert_eq!(dot, crate::WARN_RED);
        // 输入输出行钉在卡带上方（09-24 二修），与提示行同字号
        let mut pix3 = vec![0u32; w * h];
        paint_wait(
            &mut pix3,
            w,
            w,
            h,
            &font,
            8,
            false,
            false,
            Some("今天下午的日程"),
            "按住屏幕说话",
            &StatusLine { net: true, time: "9:41" },
        );
        let cy = (cards_top(w, h) - 48 - 24) as usize;
        assert!(
            (0..w).any(|x| at(&pix3, x, cy) != HOME_BG),
            "caption ink just above the cards band"
        );
        // 光标亮半程出现（breath=12 ≥ 9）
        let mut pix2 = vec![0u32; w * h];
        paint_wait(
            &mut pix2,
            w,
            w,
            h,
            &font,
            12,
            false,
            false,
            None,
            "按住屏幕说话",
            &StatusLine { net: true, time: "9:41" },
        );
        let py2 = prompt_y(h) as usize + 30;
        assert!(
            (0..w).any(|x| at(&pix2, x, py2) == crate::GREEN),
            "cursor block painted on the bright half"
        );
    }
}
