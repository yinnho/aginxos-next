// On-screen keyboard, the panel's lower half. Input is evdev (type-B touch
// protocol); keys fire on finger-DOWN (a tap only read as complete at
// finger-up was a big chunk of the perceived typing lag). Drag-scroll of
// scrollback is armed only by touches starting ABOVE the keyboard, so
// dragging across keys no longer scrolls.
//
// M17: the keyboard is a key table (inputd shape) — every keycap is a
// KeyDef (label + Act), and hit tests return typed input::InputEvents,
// never raw pty bytes. Text keys (letters, symbols, space) come back as
// TextInputEvent, control keys as KeyEvent; encoding to bytes happens
// once, in the terminal layer (input::encode). Voice (M18) and the IME
// (M40) inject TextInputEvent through the same path in main.rs.
//
// M40b: the letter block follows the iPhone keyboard layout (user call,
// 2026-09-03) — three letter rows (10 / 9 centered / shift+7+delete) plus
// a bottom row of [123] [拼] [ space ] [。] [换行], with the 123 and #+=
// symbol pages switching through the bottom-left key like iOS. Pages
// LATCH (typing a digit stays on the page); only shift stays one-shot.
// Letters display lowercase, shift uppercases. What iOS does not have
// and a terminal needs stays in the slim extra-keys row above (Termux
// style): ESC TAB CTL and the arrows.

use crate::input::{Dir, InputEvent, KeyEvent};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::io::AsRawFd;

/// Whole-panel page, switched by the 123 / #+= / ABC keys (iOS behavior:
/// latching — a page persists across key presses).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Letters,
    Num,
    Sym,
}

/// What a keycap does. Grid rows (rows 0-1) are char grids because their
/// behavior is combinatorial (page x shift x ctrl); rows 2-3 spell their
/// keys out here. The vocabulary grows with the table — add a KeyDef,
/// not a match arm.
#[derive(Clone)]
pub enum Act {
    /// Fixed key: emits this event every tap (DEL, 换行, ESC, arrows).
    Ev(InputEvent),
    /// Letter key (row 2): composes with shift/ctrl at hit time.
    Letter(char),
    /// Literal char key: digits and punctuation — shift does not compose.
    Text(char),
    /// Space — a Text event, composed at hit time (" ".into() is not const).
    Space,
    /// 。 while 拼 is on, . while off (iOS Chinese convention).
    Period,
    /// One-shot shift (next letter uppercased).
    Shift,
    /// One-shot ctrl (letter -> KeyEvent::Ctrl).
    Ctrl,
    /// 拼 (M40): latching toggle for the pinyin IME. It stays on until
    /// pressed again — an IME is a mode you type in, not a one-shot.
    Pinyin,
    /// Switch the panel to this page.
    Page(Page),
}

pub struct KeyDef {
    pub label: &'static str,
    pub act: Act,
    /// Relative width within the row (letters weigh 2; shift/del/123/换行
    /// weigh 3; space weighs 8). Each row normalizes to the full span.
    pub w: usize,
}

// Extra-keys row (above the letter block), Termux style: ESC TAB CTL and
// the four arrows (拼 lives in the bottom row, the iOS globe slot). Arrows
// are font glyphs 0x10-0x13.
pub const EXTRA_KEYS: [KeyDef; 7] = [
    KeyDef { label: "ESC", act: Act::Ev(InputEvent::Key(KeyEvent::Esc)), w: 1 },
    KeyDef { label: "TAB", act: Act::Ev(InputEvent::Key(KeyEvent::Tab)), w: 1 },
    KeyDef { label: "CTL", act: Act::Ctrl, w: 1 },
    KeyDef { label: "\u{10}", act: Act::Ev(InputEvent::Key(KeyEvent::Arrow(Dir::Left))), w: 1 },
    KeyDef { label: "\u{11}", act: Act::Ev(InputEvent::Key(KeyEvent::Arrow(Dir::Down))), w: 1 },
    KeyDef { label: "\u{12}", act: Act::Ev(InputEvent::Key(KeyEvent::Arrow(Dir::Up))), w: 1 },
    KeyDef { label: "\u{13}", act: Act::Ev(InputEvent::Key(KeyEvent::Arrow(Dir::Right))), w: 1 },
];

// Grid rows (uniform cells): 10 keys, then 9 centered — iOS letter block,
// lowercase labels (shift shows caps). Same shape on every page.
const GRID: [&str; 2] = ["qwertyuiop", "asdfghjkl"];
const GRID_NUM: [&str; 2] = ["1234567890", "-/:;()$&@\""];
const GRID_SYM: [&str; 2] = ["[]{}#%^*+=", "_\\|~<>`'\""];

// Row 2: shift-slot + keys + delete (iOS). Letters page: ⇧ z x c v b n m ⌫.
const R2: [KeyDef; 9] = [
    KeyDef { label: "SHF", act: Act::Shift, w: 3 },
    KeyDef { label: "z", act: Act::Letter('z'), w: 2 },
    KeyDef { label: "x", act: Act::Letter('x'), w: 2 },
    KeyDef { label: "c", act: Act::Letter('c'), w: 2 },
    KeyDef { label: "v", act: Act::Letter('v'), w: 2 },
    KeyDef { label: "b", act: Act::Letter('b'), w: 2 },
    KeyDef { label: "n", act: Act::Letter('n'), w: 2 },
    KeyDef { label: "m", act: Act::Letter('m'), w: 2 },
    KeyDef { label: "DEL", act: Act::Ev(InputEvent::Key(KeyEvent::Backspace)), w: 3 },
];
// 123 page row 2: #+= . , ? ! ' ⌫ (the shift slot becomes the #+= switch).
const R2_NUM: [KeyDef; 7] = [
    KeyDef { label: "#+=", act: Act::Page(Page::Sym), w: 3 },
    KeyDef { label: ".", act: Act::Text('.'), w: 2 },
    KeyDef { label: ",", act: Act::Text(','), w: 2 },
    KeyDef { label: "?", act: Act::Text('?'), w: 2 },
    KeyDef { label: "!", act: Act::Text('!'), w: 2 },
    KeyDef { label: "'", act: Act::Text('\''), w: 2 },
    KeyDef { label: "DEL", act: Act::Ev(InputEvent::Key(KeyEvent::Backspace)), w: 3 },
];
// #+= page row 2: 123 . , : ; ! ? ⌫ (terminal-tuned where iOS shows
// currency the font subset cannot render).
const R2_SYM: [KeyDef; 8] = [
    KeyDef { label: "123", act: Act::Page(Page::Num), w: 3 },
    KeyDef { label: ".", act: Act::Text('.'), w: 2 },
    KeyDef { label: ",", act: Act::Text(','), w: 2 },
    KeyDef { label: ":", act: Act::Text(':'), w: 2 },
    KeyDef { label: ";", act: Act::Text(';'), w: 2 },
    KeyDef { label: "!", act: Act::Text('!'), w: 2 },
    KeyDef { label: "?", act: Act::Text('?'), w: 2 },
    KeyDef { label: "DEL", act: Act::Ev(InputEvent::Key(KeyEvent::Backspace)), w: 3 },
];

// Bottom row (same on every page except the left key): [123/ABC] 拼 ——
// space —— 。/。 换行. 拼 sits in the iOS globe slot; the period shows 。
// while the IME is on.
const R3: [KeyDef; 5] = [
    KeyDef { label: "123", act: Act::Page(Page::Num), w: 3 },
    KeyDef { label: "拼", act: Act::Pinyin, w: 2 },
    KeyDef { label: "", act: Act::Space, w: 8 },
    KeyDef { label: ".", act: Act::Period, w: 2 },
    KeyDef { label: "换行", act: Act::Ev(InputEvent::Key(KeyEvent::Enter)), w: 3 },
];
const R3_NUM: [KeyDef; 5] = [
    KeyDef { label: "ABC", act: Act::Page(Page::Letters), w: 3 },
    KeyDef { label: "拼", act: Act::Pinyin, w: 2 },
    KeyDef { label: "", act: Act::Space, w: 8 },
    KeyDef { label: ".", act: Act::Period, w: 2 },
    KeyDef { label: "换行", act: Act::Ev(InputEvent::Key(KeyEvent::Enter)), w: 3 },
];
const R3_SYM: [KeyDef; 5] = R3_NUM;

pub const KB_M: usize = 28; // side margin (px)
pub const KB_B: usize = 24; // bottom margin (px)
// Rows in the letter block (grid, grid, weighted, bottom) — the M40b iOS
// layout; the pre-iOS layout had a fifth specials row.
const KB_ROWS: usize = 4;
// Row height cap: keeps the terminal area identical to the pre-grid layout
// on redfin — the h/2 height budget would allow far taller caps.
const KB_ROW_H: usize = 118;

pub struct Kb {
    shift: bool,
    ctrl: bool,
    pinyin: bool,
    page: Page,
    /// Key under the finger right now: (area, row, index) — the renderer
    /// lights that one keycap for touch feedback (iOS keys darken while
    /// pressed; ours go bright). Lives from finger-DOWN to any lift.
    pressed: Option<(u8, u8, u8)>,
}

/// pressed-slot areas (see Kb::pressed).
pub const AREA_EXTRA: u8 = 0;
pub const AREA_PANEL: u8 = 1;

pub struct KeyGeom {
    pub panel_y: usize, // letter panel top edge (px)
    pub extra_y: usize, // extra-keys row top edge (px)
    pub extra_h: usize,
    pub x_off: usize, // side margin
    pub gap: usize, // uniform keycap gap, H+V (≈0.8% of span)
    pub label_scale: usize, // letter labels: ~half the cap, not edge-to-edge
    pub span: usize,
    pub cell_w: usize,
    pub cell_h: usize,
}

impl Kb {
    pub fn new() -> Kb {
        Kb { shift: false, ctrl: false, pinyin: false, page: Page::Letters, pressed: None }
    }

    pub fn geom(w: usize, h: usize) -> KeyGeom {
        let span = w - 2 * KB_M;
        let cell_w = span / 10;
        let cell_h = (h / 2 / 5).min(KB_ROW_H);
        let gap = span / 128; // ≈0.8% of span: 8 px on the 1080 panel
        // letter labels: ~half the keycap so rows read as separate keys
        let label_scale = ((cell_w - 24) / 6).min((cell_h - 24) / 8).max(2);
        let panel_y = h - KB_B - cell_h * KB_ROWS;
        let extra_h = cell_h * 3 / 4;
        KeyGeom {
            panel_y,
            extra_y: panel_y - extra_h - 8,
            extra_h,
            x_off: KB_M,
            gap,
            label_scale,
            span,
            cell_w,
            cell_h,
        }
    }

    /// Extra-keys row hit test. y in [extra_y, panel_y).
    pub fn extra_key_at(&mut self, g: &KeyGeom, x: usize, y: usize) -> Option<InputEvent> {
        if y < g.extra_y || y >= g.panel_y || x < g.x_off {
            return None;
        }
        let kw = g.span / EXTRA_KEYS.len();
        let k = ((x - g.x_off) / kw).min(EXTRA_KEYS.len() - 1);
        match &EXTRA_KEYS[k].act {
            Act::Ctrl => {
                self.ctrl = !self.ctrl;
                Some(InputEvent::Text(String::new())) // consumed, no output
            }
            Act::Ev(ev) => Some(ev.clone()),
            _ => None,
        }
    }

    pub fn page(&self) -> Page {
        self.page
    }

    /// Grid rows 0-1 for the current page (uniform cells, 10 then 9 keys).
    pub fn grids(&self) -> [&'static str; 2] {
        match self.page {
            Page::Letters => GRID,
            Page::Num => GRID_NUM,
            Page::Sym => GRID_SYM,
        }
    }

    /// Weighted row 2 (shift-slot + keys + delete) for the current page.
    pub fn row2(&self) -> &'static [KeyDef] {
        match self.page {
            Page::Letters => &R2,
            Page::Num => &R2_NUM,
            Page::Sym => &R2_SYM,
        }
    }

    /// Bottom row for the current page.
    pub fn row3(&self) -> &'static [KeyDef] {
        match self.page {
            Page::Letters => &R3,
            Page::Num => &R3_NUM,
            Page::Sym => &R3_SYM,
        }
    }

    /// Key at panel coords -> input event. Text keys (page/shift/ctrl
    /// compositing) come back as TextInputEvent; DEL/换行 as KeyEvent.
    /// Modifier toggles and page switches return empty Text (consumed).
    pub fn key_at(&mut self, g: &KeyGeom, x: usize, y: usize) -> Option<InputEvent> {
        if y < g.panel_y || x < g.x_off {
            return None;
        }
        let xr = x - g.x_off;
        let row = (y - g.panel_y) / g.cell_h;
        match row {
            0 | 1 => {
                let s = self.grids()[row];
                let n = s.len();
                let col = (xr / (g.span / n)).min(n - 1);
                let ch = s.as_bytes()[col] as char;
                self.compose_letter(ch)
            }
            2 => self.row_act(self.row2(), g, xr),
            3 => self.row_act(self.row3(), g, xr),
            _ => None,
        }
    }

    /// A letter under one-shot shift/ctrl (grid rows and row-2 letters).
    fn compose_letter(&mut self, ch: char) -> Option<InputEvent> {
        let ctrl = self.ctrl;
        self.ctrl = false;
        if ctrl && ch.is_ascii_alphabetic() {
            return Some(InputEvent::Key(KeyEvent::Ctrl(ch)));
        }
        let mut c = ch;
        if self.shift && self.page == Page::Letters {
            c = c.to_ascii_uppercase();
        }
        self.shift = false;
        Some(InputEvent::Text(c.to_string()))
    }

    fn row_act(&mut self, row: &'static [KeyDef], g: &KeyGeom, xr: usize) -> Option<InputEvent> {
        let kd = weighted_at(row, g, xr);
        match kd.act.clone() {
            Act::Letter(ch) => self.compose_letter(ch),
            Act::Text(ch) => Some(InputEvent::Text(ch.to_string())),
            Act::Period => Some(InputEvent::Text(if self.pinyin { "。" } else { "." }.into())),
            Act::Space => Some(InputEvent::Text(" ".into())),
            Act::Shift => {
                self.shift = !self.shift;
                Some(InputEvent::Text(String::new()))
            }
            Act::Ctrl => {
                self.ctrl = !self.ctrl;
                Some(InputEvent::Text(String::new()))
            }
            Act::Pinyin => {
                self.pinyin = !self.pinyin;
                Some(InputEvent::Text(String::new()))
            }
            Act::Page(p) => {
                self.page = p;
                Some(InputEvent::Text(String::new()))
            }
            Act::Ev(ev) => Some(ev),
        }
    }

    pub fn shift_on(&self) -> bool {
        self.shift
    }

    /// Remember which key the finger just landed on (touch feedback).
    /// Call from the Down path; any lift clears it.
    pub fn press_locate(&mut self, g: &KeyGeom, x: usize, y: usize) {
        self.pressed = self.locate(g, x, y);
    }

    /// Drop the highlight on finger lift; true if a repaint is needed.
    pub fn clear_pressed_if_any(&mut self) -> bool {
        self.pressed.take().is_some()
    }

    pub fn is_pressed(&self, area: u8, row: u8, idx: u8) -> bool {
        self.pressed == Some((area, row, idx))
    }

    /// (area, row, index) of the key at panel coords — the same walks
    /// key_at uses, index instead of event.
    fn locate(&self, g: &KeyGeom, x: usize, y: usize) -> Option<(u8, u8, u8)> {
        if x < g.x_off {
            return None;
        }
        if y >= g.extra_y && y < g.panel_y {
            let kw = g.span / EXTRA_KEYS.len();
            let k = ((x - g.x_off) / kw).min(EXTRA_KEYS.len() - 1);
            return Some((AREA_EXTRA, 0, k as u8));
        }
        if y < g.panel_y {
            return None;
        }
        let row = (y - g.panel_y) / g.cell_h;
        let xr = x - g.x_off;
        match row {
            0 | 1 => {
                let s = self.grids()[row];
                let n = s.len();
                let col = (xr / (g.span / n)).min(n - 1);
                Some((AREA_PANEL, row as u8, col as u8))
            }
            2 | 3 => {
                let defs = if row == 2 { self.row2() } else { self.row3() };
                let units: usize = defs.iter().map(|k| k.w).sum();
                let mut acc = 0usize;
                for (i, k) in defs.iter().enumerate() {
                    acc += k.w;
                    if xr < g.span * acc / units {
                        return Some((AREA_PANEL, row as u8, i as u8));
                    }
                }
                Some((AREA_PANEL, row as u8, defs.len() as u8 - 1))
            }
            _ => None,
        }
    }

    pub fn ctrl_on(&self) -> bool {
        self.ctrl
    }

    pub fn pinyin_on(&self) -> bool {
        self.pinyin
    }

    /// Host --ppm demo path (AGINX_TERM_IME_DEMO): latch 拼 without a touch.
    pub fn set_pinyin(&mut self, on: bool) {
        self.pinyin = on;
    }
}

/// The KeyDef covering x-offset `xr` inside a weighted row (each row
/// normalizes its weights to the full span).
pub fn weighted_at(row: &'static [KeyDef], g: &KeyGeom, xr: usize) -> &'static KeyDef {
    let units: usize = row.iter().map(|k| k.w).sum();
    let mut acc = 0usize;
    for k in row {
        acc += k.w;
        if xr < g.span * acc / units {
            return k;
        }
    }
    row.last().unwrap()
}

// ---------------- evdev reader ----------------

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct input_event {
    sec: i64,
    usec: i64,
    type_: u16,
    code: u16,
    value: i32,
}

const EV_SYN: u16 = 0;
const EV_ABS: u16 = 3;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;

pub enum Touch {
    Down(usize, usize), // finger landed (keys fire here, not at lift)
    Tap(usize, usize),  // finger lifted without a drag
    Up,                 // finger lifted after a drag (still ends any hold)
    Drag(isize),        // signed pixel delta (positive = finger moved down)
    None,
}

pub struct TouchReader {
    fd: std::fs::File,
    sx: f32,
    sy: f32,
    raw_x: i32,
    raw_y: i32,
    down: bool,
    pending_down: bool,
    /// The y of THIS touch hasn't been seen yet (tracking-id came first):
    /// the first ABS_MT_POSITION_Y anchors start_y, instead of inheriting
    /// the previous touch's position and instantly reading as a 30 px
    /// "drag". Keeps both event orders working — firmware that reports
    /// positions before tracking-id and synthetic frames after it.
    fresh: bool,
    /// A POSITION_Y was already read in the current frame (before its
    /// tracking-id — real firmware order). Lets the tracking-id handler
    /// tell "position seen" (anchor now) from "position pending" (anchor
    /// at the next y, i.e. synthetic-frame order).
    y_in_frame: bool,
    start_y: i32,
    last_y: i32,
    dragged: bool,
    screen_w: i32,
    screen_h: i32,
}

impl TouchReader {
    pub fn open(path: &str, screen_w: i32, screen_h: i32) -> Option<TouchReader> {
        let fd = OpenOptions::new().read(true).open(path).ok()?;
        // Kernel 4.19 reports the panel-native ranges for both axes; scale
        // to actual fb size.
        Some(TouchReader {
            fd,
            sx: screen_w as f32 / 1080.0,
            sy: screen_h as f32 / 2340.0,
            raw_x: 0,
            raw_y: 0,
            down: false,
            pending_down: false,
            fresh: false,
            y_in_frame: false,
            start_y: 0,
            last_y: 0,
            dragged: false,
            screen_w,
            screen_h,
        })
    }

    pub fn raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }

    pub fn poll(&mut self) -> Touch {
        let mut buf = [0u8; 24 * 8];
        let n = match self.fd.read(&mut buf) {
            Ok(n) => n,
            Err(_) => return Touch::None,
        };
        let mut out = Touch::None;
        for chunk in buf[..n].chunks_exact(24) {
            let ev = unsafe { std::ptr::read_unaligned(chunk.as_ptr() as *const input_event) };
            match (ev.type_, ev.code) {
                (EV_ABS, ABS_MT_SLOT) => {
                    if ev.value != 0 {
                        return Touch::None; // multi-touch: bail on this frame
                    }
                }
                (EV_ABS, ABS_MT_TRACKING_ID) => {
                    if ev.value == -1 {
                        if std::env::var("AGINX_TERM_DEBUG").is_ok() {
                            eprintln!("aginx-term: lift: down={} dragged={}", self.down, self.dragged);
                        }
                        // finger up — ALWAYS reported (hold-repeat cleanup
                        // depends on it, even when the gesture was a drag)
                        self.pending_down = false;
                        if self.down && !self.dragged {
                            let x = (self.raw_x as f32 * self.sx) as usize;
                            let y = (self.raw_y as f32 * self.sy) as usize;
                            out = Touch::Tap(
                                x.min(self.screen_w as usize - 1),
                                y.min(self.screen_h as usize - 1),
                            );
                        } else if self.down {
                            out = Touch::Up;
                        }
                        self.down = false;
                        self.dragged = false;
                    } else {
                        self.down = true;
                        self.dragged = false;
                        self.pending_down = true;
                        // Anchor start_y for the drag threshold. Real
                        // firmware reports position before tracking-id, so
                        // raw_y is this touch's; synthetic frames put
                        // tracking-id first and raw_y is the PREVIOUS
                        // touch's position — anchoring from it would read
                        // as an instant 30 px drag and kill the tap.
                        if self.y_in_frame {
                            self.start_y = self.raw_y;
                            self.last_y = self.raw_y;
                            self.fresh = false;
                        } else {
                            self.fresh = true; // first y anchors
                        }
                    }
                }
                (EV_ABS, ABS_MT_POSITION_X) => self.raw_x = ev.value,
                (EV_ABS, ABS_MT_POSITION_Y) => {
                    self.raw_y = ev.value;
                    self.y_in_frame = true;
                    if self.fresh {
                        // first y of this touch: anchor, no drag judgment
                        self.fresh = false;
                        self.start_y = ev.value;
                        self.last_y = ev.value;
                    } else if self.down {
                        let dy = self.raw_y - self.last_y;
                        if (self.raw_y - self.start_y).abs() > 30 {
                            self.dragged = true;
                        }
                        if self.dragged && dy != 0 {
                            out = Touch::Drag((dy as f32 * self.sy) as isize);
                            self.last_y = self.raw_y;
                        }
                    }
                }
                (EV_SYN, _) => {
                    // coords for this frame are settled: report the press
                    if self.pending_down && self.down {
                        self.pending_down = false;
                        let x = (self.raw_x as f32 * self.sx) as usize;
                        let y = (self.raw_y as f32 * self.sy) as usize;
                        out = Touch::Down(
                            x.min(self.screen_w as usize - 1),
                            y.min(self.screen_h as usize - 1),
                        );
                    }
                    self.y_in_frame = false; // frame boundary
                }
                _ => {}
            }
        }
        out
    }
}

// ---------------- key events (power / volume) ----------------

const EV_KEY: u16 = 1;
pub const KEY_POWER: u16 = 116;

/// Non-touch key events from one evdev node. qpnp_pon (/dev/input/event1)
/// carries power + volume-down on redfin; we only act on KEY_POWER, whose
/// presence in the node's KEY bitmap was confirmed via /proc/bus/input
/// (2026-08-31). qpnp_pon has no EV_REP, so every event is a clean
/// press (1) or release (0) — value 2 autorepeat never appears.
pub struct KeyReader {
    fd: std::fs::File,
}

impl KeyReader {
    pub fn open(path: &str) -> Option<KeyReader> {
        Some(KeyReader {
            fd: OpenOptions::new().read(true).open(path).ok()?,
        })
    }

    pub fn raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }

    pub fn poll(&mut self) -> Vec<(u16, bool)> {
        let mut buf = [0u8; 24 * 8];
        let n = match self.fd.read(&mut buf) {
            Ok(n) => n,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        for chunk in buf[..n].chunks_exact(24) {
            let ev = unsafe { std::ptr::read_unaligned(chunk.as_ptr() as *const input_event) };
            if ev.type_ == EV_KEY && (ev.value == 0 || ev.value == 1) {
                out.push((ev.code, ev.value == 1));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> KeyGeom {
        Kb::geom(1080, 2340)
    }

    fn text_of(ev: Option<InputEvent>) -> String {
        match ev {
            Some(InputEvent::Text(s)) => s,
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn letter_block_is_ios() {
        let g = g();
        let mut kb = Kb::new();
        // row 0: 10 uniform keys, q at the left edge
        assert_eq!(text_of(kb.key_at(&g, g.x_off + 10, g.panel_y + 10)), "q");
        assert_eq!(text_of(kb.key_at(&g, g.x_off + g.span - 10, g.panel_y + 10)), "p");
        // row 1: 9 keys centered — 'a' is inset from the left edge
        let cw1 = g.span / 9;
        assert_eq!(text_of(kb.key_at(&g, g.x_off + cw1 / 2, g.panel_y + g.cell_h + 10)), "a");
        // row 2: shift (3/20) then z x c v b n m then DEL
        let z_x = g.x_off + g.span * 3 / 20 + g.span / 10 / 2;
        assert_eq!(text_of(kb.key_at(&g, z_x, g.panel_y + 2 * g.cell_h + 10)), "z");
        // DEL at the far right emits the Backspace key event
        assert!(matches!(
            kb.key_at(&g, g.x_off + g.span - 10, g.panel_y + 2 * g.cell_h + 10),
            Some(InputEvent::Key(KeyEvent::Backspace))
        ));
        // bottom row center is space
        assert_eq!(text_of(kb.key_at(&g, 540, g.panel_y + 3 * g.cell_h + 10)), " ");
        // 4 rows in the block (the pre-iOS layout had 5)
        assert_eq!(g.panel_y + 4 * g.cell_h + KB_B, 2340);
    }

    #[test]
    fn shift_uppercases_one_shot() {
        let g = g();
        let mut kb = Kb::new();
        // SHF toggles (consumed, empty Text), next letter is caps, then back
        assert!(matches!(
            kb.key_at(&g, g.x_off + g.span / 20, g.panel_y + 2 * g.cell_h + 10),
            Some(InputEvent::Text(ref s)) if s.is_empty()
        ));
        assert!(kb.shift_on());
        assert_eq!(text_of(kb.key_at(&g, g.x_off + 10, g.panel_y + 10)), "Q");
        assert!(!kb.shift_on());
        assert_eq!(text_of(kb.key_at(&g, g.x_off + 10, g.panel_y + 10)), "q");
    }

    #[test]
    fn pages_latch_and_switch() {
        let g = g();
        let mut kb = Kb::new();
        let bottom_left = (g.x_off + g.span / 12, g.panel_y + 3 * g.cell_h + 10);
        // 123 -> digits, and typing a digit STAYS on the page (iOS latch)
        assert!(matches!(
            kb.key_at(&g, bottom_left.0, bottom_left.1),
            Some(InputEvent::Text(ref s)) if s.is_empty()
        ));
        assert_eq!(kb.page(), Page::Num);
        assert_eq!(text_of(kb.key_at(&g, g.x_off + 10, g.panel_y + 10)), "1");
        assert_eq!(text_of(kb.key_at(&g, g.x_off + 10, g.panel_y + 10)), "1");
        // #+= from the 123 page's shift slot, then ABC back to letters
        let eq_key = (g.x_off + g.span * 3 / 32, g.panel_y + 2 * g.cell_h + 10);
        kb.key_at(&g, eq_key.0, eq_key.1);
        assert_eq!(kb.page(), Page::Sym);
        assert_eq!(text_of(kb.key_at(&g, g.x_off + 10, g.panel_y + 10)), "[");
        kb.key_at(&g, bottom_left.0, bottom_left.1); // ABC
        assert_eq!(kb.page(), Page::Letters);
        assert_eq!(text_of(kb.key_at(&g, g.x_off + 10, g.panel_y + 10)), "q");
    }

    #[test]
    fn period_follows_ime_state() {
        let g = g();
        let mut kb = Kb::new();
        let period_x = g.x_off + g.span * 13 / 18; // after space (3+2+8 of 18)
        let y = g.panel_y + 3 * g.cell_h + 10;
        assert_eq!(text_of(kb.key_at(&g, period_x, y)), ".");
        kb.set_pinyin(true);
        assert_eq!(text_of(kb.key_at(&g, period_x, y)), "。");
    }

    #[test]
    fn ctrl_chords_and_arrows() {
        let g = g();
        let mut kb = Kb::new();
        // CTL from the extra row, then 'c' (row 2, center of the c key) -> Ctrl('c')
        let c_x = g.x_off + g.span * 8 / 20;
        kb.extra_key_at(&g, g.x_off + g.span * 2 / 7 + 5, g.extra_y + 5); // CTL
        assert!(matches!(
            kb.key_at(&g, c_x, g.panel_y + 2 * g.cell_h + 10),
            Some(InputEvent::Key(KeyEvent::Ctrl(c))) if c == 'c'
        ));
        // arrows in the extra row are plain key events
        assert!(matches!(
            kb.extra_key_at(&g, g.x_off + g.span - 5, g.extra_y + 5),
            Some(InputEvent::Key(KeyEvent::Arrow(Dir::Right)))
        ));
    }
}
