// Home — the phone face after boot. Status bar + clock + four app icons.
// Geometry is relative to the panel so both committed machine profiles
// share one layout.

use crate::draw_centered;
use crate::draw_text;
use crate::fill_disk;
use crate::fill_rect;
use crate::launch;
use crate::text_w;
use crate::GREEN;
use crate::IDLE_BG;
use crate::MGREEN;
use crate::WHITE;

/// Near-black OLED, same as the boot wordmark so the handoff does not flash
/// a different black.
pub const HOME_BG: u32 = IDLE_BG;
const WELL: u32 = 0x001C2420;
const INK: u32 = WHITE;
pub const WORDMARK: &str = "AginxOS";
pub const WORDMARK_SCALE: usize = 13;
pub const ICON_R: i32 = 70;
pub const STATUS_H: usize = 72;
pub const CLOCK_SCALE: usize = 12;
pub const DATE_SCALE: usize = 4;
pub const LABEL_SCALE: usize = 3;
pub const STATUS_SCALE: usize = 3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum App {
    Camera,
    Photos,
    Talk,
    Settings,
}

pub const APPS: [App; 4] = [App::Camera, App::Photos, App::Talk, App::Settings];

pub fn app_label(app: App) -> &'static str {
    match app {
        App::Camera => "相机",
        App::Photos => "相册",
        App::Talk => "对话",
        App::Settings => "设置",
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Icon {
    pub app: App,
    pub cx: i32,
    pub cy: i32,
    pub r: i32,
}

pub fn icons(w: usize, h: usize) -> [Icon; 4] {
    let r = ICON_R;
    let cy = (h as i32) * 78 / 100;
    let mut out = [Icon {
        app: App::Camera,
        cx: 0,
        cy,
        r,
    }; 4];
    for (i, app) in APPS.iter().copied().enumerate() {
        let cx = 80 + (w as i32 - 160) * (2 * i as i32 + 1) / 8;
        out[i] = Icon { app, cx, cy, r };
    }
    out
}

/// Hit includes the well plus the CJK label band under it.
pub fn hit(w: usize, h: usize, x: usize, y: usize) -> Option<App> {
    let x = x as i32;
    let y = y as i32;
    for ic in icons(w, h) {
        let dx = x - ic.cx;
        let dy = y - ic.cy;
        if dx.abs() <= ic.r + 36 && dy >= -ic.r - 16 && dy <= ic.r + 88 {
            return Some(ic.app);
        }
    }
    None
}

pub fn wordmark_y(h: usize) -> i32 {
    (h * 45 / 100) as i32
}

pub struct Clock {
    pub time: String,
    pub date: String,
}

pub fn clock_from_parts(hour: u32, minute: u32, month: u32, day: u32, wday: u32) -> Clock {
    let wd = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
    let w = wd[(wday as usize) % 7];
    Clock {
        time: format!("{hour}:{minute:02}"),
        date: format!("{month}月{day}日 {w}"),
    }
}

pub fn read_clock(tz: &str) -> Clock {
    let out = std::process::Command::new("date")
        .args(["+%H %M %m %d %w"])
        .env("TZ", tz)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());
    if let Some(s) = out {
        let mut it = s.split_whitespace();
        if let (Some(h), Some(mi), Some(mo), Some(d), Some(w)) =
            (it.next(), it.next(), it.next(), it.next(), it.next())
        {
            if let (Ok(h), Ok(mi), Ok(mo), Ok(d), Ok(w)) =
                (h.parse(), mi.parse(), mo.parse(), d.parse(), w.parse())
            {
                return clock_from_parts(h, mi, mo, d, w);
            }
        }
    }
    Clock {
        time: "--:--".into(),
        date: String::new(),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Setting {
    Pair,
    Install,
    Shell,
    PowerOff,
}

pub const SETTINGS: [(&'static str, Setting); 4] = [
    ("连接网络", Setting::Pair),
    ("软件清单", Setting::Install),
    ("终端", Setting::Shell),
    ("关机", Setting::PowerOff),
];

pub fn settings_hit(g: &launch::Geom, x: usize, y: usize) -> Option<Setting> {
    g.button_at(x, y, SETTINGS.len())
        .and_then(|i| SETTINGS.get(i).map(|(_, s)| *s))
}

pub fn paint_wordmark(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    font: &[[u8; 8]; 128],
) {
    fill_rect(pix, pitch, w, h, 0, 0, w as i32, h as i32, HOME_BG);
    draw_centered(
        pix,
        pitch,
        w,
        h,
        font,
        wordmark_y(h),
        WORDMARK,
        WORDMARK_SCALE,
        MGREEN,
    );
}

pub fn paint(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    font: &[[u8; 8]; 128],
    clock: &Clock,
    net: bool,
    unpaired: bool,
    status: Option<&str>,
) {
    fill_rect(pix, pitch, w, h, 0, 0, w as i32, h as i32, HOME_BG);
    // status bar
    let ty = (STATUS_H as i32 - 8 * STATUS_SCALE as i32) / 2;
    draw_text(
        pix, pitch, w, h, font, 40, ty, &clock.time, STATUS_SCALE, INK,
    );
    if net {
        paint_signal(pix, pitch, w, h, w as i32 - 40, STATUS_H as i32 / 2);
    }
    // large clock
    let time_y = (h as i32) * 24 / 100;
    draw_centered(pix, pitch, w, h, font, time_y, &clock.time, CLOCK_SCALE, INK);
    let date_y = time_y + (8 * CLOCK_SCALE as i32) + 28;
    if !clock.date.is_empty() {
        draw_centered(pix, pitch, w, h, font, date_y, &clock.date, DATE_SCALE, INK);
    }
    if let Some(s) = status {
        let sy = date_y + (8 * DATE_SCALE as i32) + 36;
        draw_centered(pix, pitch, w, h, font, sy, s, 3, INK);
    }
    for ic in icons(w, h) {
        paint_icon(pix, pitch, w, h, font, ic, unpaired && ic.app == App::Settings);
    }
}

/// 📶 — four rising bars, bottom-aligned. CJK subset has no emoji.
fn paint_signal(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    right: i32,
    mid_y: i32,
) {
    let bar_w = 7i32;
    let gap = 5i32;
    let n = 4i32;
    let max_h = 26i32;
    let total = n * bar_w + (n - 1) * gap;
    let x0 = right - total;
    let y1 = mid_y + max_h / 2;
    for i in 0..n {
        let bh = max_h * (i + 1) / n;
        let x = x0 + i * (bar_w + gap);
        fill_rect(pix, pitch, w, h, x, y1 - bh, bar_w, bh, INK);
    }
}

fn paint_icon(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    font: &[[u8; 8]; 128],
    ic: Icon,
    dot: bool,
) {
    fill_disk(pix, pitch, w, h, ic.cx, ic.cy, ic.r, WELL);
    match ic.app {
        App::Camera => icon_camera(pix, pitch, w, h, ic),
        App::Photos => icon_photos(pix, pitch, w, h, ic),
        App::Talk => icon_talk(pix, pitch, w, h, ic),
        App::Settings => icon_settings(pix, pitch, w, h, ic),
    }
    if dot {
        fill_disk(
            pix,
            pitch,
            w,
            h,
            ic.cx + ic.r * 2 / 3,
            ic.cy - ic.r * 2 / 3,
            12,
            GREEN,
        );
    }
    let label = app_label(ic.app);
    let tw = text_w(label, LABEL_SCALE) as i32;
    let ly = ic.cy + ic.r + 22;
    draw_text(
        pix,
        pitch,
        w,
        h,
        font,
        ic.cx - tw / 2,
        ly,
        label,
        LABEL_SCALE,
        INK,
    );
}

fn icon_camera(pix: &mut [u32], pitch: usize, w: usize, h: usize, ic: Icon) {
    // lens: ring + pupil
    fill_disk(pix, pitch, w, h, ic.cx, ic.cy + 4, ic.r * 42 / 100, INK);
    fill_disk(pix, pitch, w, h, ic.cx, ic.cy + 4, ic.r * 28 / 100, WELL);
    fill_disk(pix, pitch, w, h, ic.cx, ic.cy + 4, ic.r * 10 / 100, INK);
    // flash
    fill_disk(
        pix,
        pitch,
        w,
        h,
        ic.cx + ic.r * 38 / 100,
        ic.cy - ic.r * 32 / 100,
        ic.r * 10 / 100,
        INK,
    );
}

fn icon_photos(pix: &mut [u32], pitch: usize, w: usize, h: usize, ic: Icon) {
    // stacked frames
    let w0 = ic.r * 90 / 100;
    let h0 = ic.r * 70 / 100;
    fill_rect(
        pix,
        pitch,
        w,
        h,
        ic.cx - w0 / 2 + 8,
        ic.cy - h0 / 2 - 8,
        w0,
        h0,
        INK,
    );
    fill_rect(
        pix,
        pitch,
        w,
        h,
        ic.cx - w0 / 2 + 12,
        ic.cy - h0 / 2 - 4,
        w0 - 8,
        h0 - 8,
        WELL,
    );
    fill_rect(
        pix,
        pitch,
        w,
        h,
        ic.cx - w0 / 2,
        ic.cy - h0 / 2 + 4,
        w0,
        h0,
        INK,
    );
    fill_rect(
        pix,
        pitch,
        w,
        h,
        ic.cx - w0 / 2 + 6,
        ic.cy - h0 / 2 + 10,
        w0 - 12,
        h0 - 12,
        WELL,
    );
}

fn icon_talk(pix: &mut [u32], pitch: usize, w: usize, h: usize, ic: Icon) {
    let bw = ic.r * 96 / 100;
    let bh = ic.r * 62 / 100;
    let x0 = ic.cx - bw / 2;
    let y0 = ic.cy - bh / 2 - 6;
    fill_disk(pix, pitch, w, h, x0 + bh / 2, y0 + bh / 2, bh / 2, INK);
    fill_disk(pix, pitch, w, h, x0 + bw - bh / 2, y0 + bh / 2, bh / 2, INK);
    fill_rect(pix, pitch, w, h, x0 + bh / 2, y0, bw - bh, bh, INK);
    // tail
    fill_rect(
        pix,
        pitch,
        w,
        h,
        ic.cx - ic.r * 28 / 100,
        ic.cy + bh / 2 - 10,
        ic.r * 16 / 100,
        ic.r * 22 / 100,
        INK,
    );
}

fn icon_settings(pix: &mut [u32], pitch: usize, w: usize, h: usize, ic: Icon) {
    let r = ic.r * 38 / 100;
    fill_disk(pix, pitch, w, h, ic.cx, ic.cy, r + 8, INK);
    // four teeth
    let t = ic.r * 14 / 100;
    let reach = r + 16;
    fill_rect(pix, pitch, w, h, ic.cx - t / 2, ic.cy - reach, t, reach * 2, INK);
    fill_rect(pix, pitch, w, h, ic.cx - reach, ic.cy - t / 2, reach * 2, t, INK);
    fill_disk(pix, pitch, w, h, ic.cx, ic.cy, r - 6, WELL);
    fill_disk(pix, pitch, w, h, ic.cx, ic.cy, r * 28 / 100, INK);
}

pub fn paint_settings(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    font: &[[u8; 8]; 128],
    g: &launch::Geom,
    unpaired: bool,
    status: Option<&str>,
) {
    fill_rect(pix, pitch, w, h, 0, 0, w as i32, h as i32, crate::BG);
    // toolbar drawn by caller so BACK geometry stays one place
    draw_centered(
        pix,
        pitch,
        w,
        h,
        font,
        g.toolbar_h as i32 + 14,
        "设置",
        5,
        GREEN,
    );
    for (i, (label, setting)) in SETTINGS.iter().enumerate() {
        let y0 = (g.by0 + i * (g.bh + g.gap)) as i32;
        fill_rect(pix, pitch, w, h, g.bx as i32, y0, g.bw as i32, 3, crate::DIM);
        fill_rect(
            pix,
            pitch,
            w,
            h,
            g.bx as i32,
            y0 + g.bh as i32 - 3,
            g.bw as i32,
            3,
            crate::DIM,
        );
        fill_rect(pix, pitch, w, h, g.bx as i32, y0, 3, g.bh as i32, crate::DIM);
        fill_rect(
            pix,
            pitch,
            w,
            h,
            (g.bx + g.bw - 3) as i32,
            y0,
            3,
            g.bh as i32,
            crate::DIM,
        );
        let ty = y0 + (g.bh as i32 - 8 * 5) / 2;
        draw_text(
            pix,
            pitch,
            w,
            h,
            font,
            g.bx as i32 + 30,
            ty,
            label,
            5,
            GREEN,
        );
        if *setting == Setting::Pair && unpaired {
            fill_disk(
                pix,
                pitch,
                w,
                h,
                (g.bx + g.bw - 70) as i32,
                y0 + g.bh as i32 / 2,
                12,
                GREEN,
            );
        }
    }
    if let Some(s) = status {
        draw_centered(
            pix,
            pitch,
            w,
            h,
            font,
            g.kb_panel_y as i32 - 40,
            s,
            3,
            GREEN,
        );
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
    fn four_icons_hit_and_miss() {
        let (w, h) = panel();
        let ics = icons(w, h);
        assert_eq!(ics.len(), 4);
        assert_eq!(hit(w, h, ics[0].cx as usize, ics[0].cy as usize), Some(App::Camera));
        assert_eq!(hit(w, h, ics[1].cx as usize, ics[1].cy as usize), Some(App::Photos));
        assert_eq!(hit(w, h, ics[2].cx as usize, ics[2].cy as usize), Some(App::Talk));
        assert_eq!(hit(w, h, ics[3].cx as usize, ics[3].cy as usize), Some(App::Settings));
        // clock region is not an app
        assert_eq!(hit(w, h, w / 2, h * 24 / 100), None);
        // status bar is not an app
        assert_eq!(hit(w, h, 40, 20), None);
        // labels still hit
        let ic = ics[0];
        assert_eq!(
            hit(w, h, ic.cx as usize, (ic.cy + ic.r + 40) as usize),
            Some(App::Camera)
        );
        // icons are in a row, camera left of settings
        assert!(ics[0].cx < ics[1].cx && ics[1].cx < ics[2].cx && ics[2].cx < ics[3].cx);
        assert_eq!(ics[0].cy, ics[3].cy);
    }

    #[test]
    fn layout_scales_to_taller_panel() {
        let a = icons(1080, 2280); // D14-exempt: fixture panel geometry
        let b = icons(1080, 2340); // D14-exempt: taller fixture panel
        assert_eq!(a[0].cx, b[0].cx);
        assert!(b[0].cy > a[0].cy);
    }

    #[test]
    fn wordmark_lands_on_bootcard_anchor() {
        let font = font::font_init();
        let (w, h) = (1080usize, 2340usize); // D14-exempt: bootcard --ppm panel
        let mut pix = vec![0u32; w * h];
        paint_wordmark(&mut pix, w, w, h, &font);
        assert_eq!(pix[5 * w + 5], HOME_BG);
        // bootcard: y = h*45/100, scale 13, glyph rows 0-7
        let y0 = wordmark_y(h) as usize;
        let y1 = y0 + 8 * WORDMARK_SCALE;
        let mut green = 0usize;
        for y in y0..y1 {
            for x in 0..w {
                if pix[y * w + x] == MGREEN {
                    green += 1;
                }
            }
        }
        assert!(green > 800, "wordmark must paint AginxOS in MGREEN, got {green}");
        // punch-hole zone empty
        assert_eq!(pix[80 * w + 540], HOME_BG);
    }

    #[test]
    fn home_clock_and_unpaired_dot() {
        let font = font::font_init();
        let (w, h) = panel();
        let clock = clock_from_parts(9, 41, 9, 18, 5);
        assert_eq!(clock.time, "9:41");
        assert_eq!(clock.date, "9月18日 周五");
        let mut pix = vec![0u32; w * h];
        paint(&mut pix, w, w, h, &font, &clock, false, true, None);
        // unpaired dot sits on the settings well
        let ic = icons(w, h)[3];
        let dx = ic.cx + ic.r * 2 / 3;
        let dy = ic.cy - ic.r * 2 / 3;
        assert_eq!(pix[dy as usize * w + dx as usize], GREEN);
        // no signal bars when net=false: right status corner stays home bg
        let ty = (STATUS_H as usize / 2) * w + (w - 50);
        assert_eq!(pix[ty], HOME_BG);
        // time glyph uses the bitmap font — a colon cell is on
        let time_y = h * 24 / 100;
        let mut ink = 0usize;
        for y in time_y..time_y + 8 * CLOCK_SCALE {
            for x in 400..680 {
                if pix[y * w + x] == INK {
                    ink += 1;
                }
            }
        }
        assert!(ink > 400, "big clock 9:41 must paint, got {ink} ink px");
    }

    #[test]
    fn home_net_marker_right() {
        let font = font::font_init();
        let (w, h) = panel();
        let clock = clock_from_parts(9, 41, 9, 18, 5);
        let mut off = vec![0u32; w * h];
        paint(&mut off, w, w, h, &font, &clock, false, false, None);
        let mut on = vec![0u32; w * h];
        paint(&mut on, w, w, h, &font, &clock, true, false, None);
        let y = STATUS_H / 2;
        let x = w - 40 - 4; // inside the tallest bar
        assert_eq!(off[y * w + x], HOME_BG, "no bars when net is down");
        assert_eq!(on[y * w + x], INK, "📶 bars when net is up");
    }

    #[test]
    fn settings_rows_hit() {
        let kg_y = 1700usize; // D14-exempt: fake kb panel
        let g = launch::Geom::new(1080, 2280, kg_y, SETTINGS.len()); // D14-exempt: fixture panel
        let y0 = g.by0 + g.bh / 2;
        assert_eq!(settings_hit(&g, g.bx + 40, y0), Some(Setting::Pair));
        let y1 = g.by0 + (g.bh + g.gap) + g.bh / 2;
        assert_eq!(settings_hit(&g, g.bx + 40, y1), Some(Setting::Install));
        let y3 = g.by0 + 3 * (g.bh + g.gap) + g.bh / 2;
        assert_eq!(settings_hit(&g, g.bx + 40, y3), Some(Setting::PowerOff));
        assert!(settings_hit(&g, 10, y0).is_none());
    }
}
