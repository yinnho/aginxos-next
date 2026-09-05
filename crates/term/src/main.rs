// aginx-term — AginxOS on-device terminal (M11 aterm; N4③b 改姓).
//
// bootcard's DRM path + 5x8 font, a vte-parsed cell grid (black bg, green /
// white text — the fixed phosphor palette), an openpty child (sh / codex /
// grok / aclone), an evdev on-screen keyboard (tap = key, drag = scrollback),
// and a launcher (clone / codex / grok / sh). Started by rcS's aginx-term-handoff.
// once boot finishes: bootcard never exits on its own and holds DRM master
// forever, so the handoff kills it by /run/bootcard.pid and takes the panel.
//
// M15 power management: the qpnp_pon power key (event1) blanks the panel
// (connector DPMS off — the same path that darkened the screen when a DRM
// master dropped), a second short press or any touch wakes it, 60 s idle
// blanks too, holding the key ~1.2 s (or the launcher's POWER OFF / RESTART
// buttons) runs `aginx-reboot poweroff|reboot`.
//
// M17 input split: the keyboard hit tests return typed InputEvents
// (KeyEvent vs TextInputEvent, input.rs) and EVERY write to the pty goes
// through inject() — the same entry point M18's voice input will call
// with recognized text. AGINX_TERM_INJECT=1 watches /run/aginx-term.inject: any
// process drops text there, it types into the session verbatim (that's
// the voice path, testable without audio).
//
// Host verification: `aginx-term --ppm out.ppm` renders the launcher into a P6
// PPM without touching DRM (same pattern as bootcard --ppm).

mod cjk;
mod drm;
mod font;
mod input;
mod kb;
mod launch;
mod photos;
mod pinyin;
mod term;

use drm::Drm;
use input::InputEvent;
use kb::{Act, Kb, KeyDef, KeyGeom, KeyReader, Touch, TouchReader, KEY_POWER};
use std::io::Write as _;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::time::{Duration, Instant};
use term::{Style, Term};

const BG: u32 = 0x00000000;
const GREEN: u32 = 0x0034D399;
const WHITE: u32 = 0x00F5F7FA;
const DIM: u32 = 0x001E3A2E; // key outlines / separators
const ROW_GAP: usize = 8; // extra px between terminal text rows
const KEYCAP: u32 = 0x000A1410; // key fill
const UNAVAIL: u32 = 0x00115A3F; // dimmed green for missing apps

// M40 candidate strip: floats over the terminal's bottom rows while 拼
// is on — 8 slots (composing buffer, 6 candidates, page arrow).
const IME_STRIP_H: usize = 120;

// M15 power: short press (< POWER_HOLD) toggles blank; hold at or beyond it
// shuts down; IDLE_BLANK without input blanks the screen.
const POWER_HOLD: Duration = Duration::from_millis(1200);
const IDLE_BLANK: Duration = Duration::from_secs(60);

// M42a voice face: sole writer is the aginx-voice daemon (atomic tmp+rename);
// aginx-term only polls mtime and renders. Display-only modality.
const VOICE_FACE: &str = "/run/aginx-voice/face";
// M42g eye viewfinder frame: same writer, same atomic rename, same poll
// pattern. When face.eye is set this is the view's main area — the screen
// is the result canvas, not a chat log, so the live frame takes the body
// and dialog lines demote to a bottom strip.
const VOICE_EYE: &str = "/run/aginx-voice/eye.jpg";
// M47⑤c raw fast path: cam-shot --raw-out publishes RGB565 every frame;
// term blits it with no JPEG decode (the encode+decode round trip stays
// only for QR, which reads eye.jpg at 2 Hz). Preferred when present.
const VOICE_EYE_RAW: &str = "/run/aginx-voice/eye.raw";

fn fill_rect(pix: &mut [u32], pitch: usize, w: usize, h: usize, x: i32, y: i32, rw: i32, rh: i32, c: u32) {
    let (mut x, mut y, mut rw, mut rh) = (x, y, rw, rh);
    if rw <= 0 || rh <= 0 {
        return;
    }
    if x < 0 {
        rw += x;
        x = 0;
    }
    if y < 0 {
        rh += y;
        y = 0;
    }
    if x + rw > w as i32 {
        rw = w as i32 - x;
    }
    if y + rh > h as i32 {
        rh = h as i32 - y;
    }
    if rw <= 0 || rh <= 0 {
        return;
    }
    for j in 0..rh as usize {
        let row = (y as usize + j) * pitch + x as usize;
        for i in 0..rw as usize {
            pix[row + i] = c;
        }
    }
}

// Glyph lookup for terminal cells. The built-in font is 7-bit ASCII only,
// but the TUIs we host (grok, codex) draw borders/spinners with Unicode
// box-drawing, blocks and braille. Render those procedurally in the same
// 5x8 bitmap format instead of truncating the codepoint to a random ASCII
// glyph. Anything else non-ASCII falls back to '?'.
fn glyph(font: &[[u8; 8]; 128], ch: char) -> [u8; 8] {
    const V: u8 = 0x04; // center column
    const H: u8 = 0x1F; // full row
    const L: u8 = 0x1C; // row, left of center
    const R: u8 = 0x07; // row, right of center
    match ch {
        c if (c as u32) < 128 => font[c as usize],
        '─' | '╌' | '┄' => [0, 0, 0, H, 0, 0, 0, 0],
        '━' => [0, 0, 0, H, H, 0, 0, 0],
        '│' | '┆' | '┊' => [V; 8],
        '┃' => [0x0C; 8],
        '┌' | '╭' => [0, 0, 0, R, V, V, V, V],
        '┐' | '╮' => [0, 0, 0, L, V, V, V, V],
        '└' | '╰' => [V, V, V, R, 0, 0, 0, 0],
        '┘' | '╯' => [V, V, V, L, 0, 0, 0, 0],
        '├' => [V, V, V, R, V, V, V, V],
        '┤' => [V, V, V, L, V, V, V, V],
        '┬' => [0, 0, 0, H, V, V, V, V],
        '┴' => [V, V, V, H, 0, 0, 0, 0],
        '┼' => [V, V, V, H, V, V, V, V],
        '═' => [0, 0, H, 0, H, 0, 0, 0],
        '║' => [0x0A; 8],
        '╔' => [0, 0, 0x0E, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A],
        '╗' => [0, 0, 0x1A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A],
        '╚' => [0x0A, 0x0A, 0x0A, 0x0A, 0x0E, 0, 0, 0],
        '╝' => [0x0A, 0x0A, 0x0A, 0x0A, 0x1A, 0, 0, 0],
        '█' => [H; 8],
        '▀' => [H, H, H, H, 0, 0, 0, 0],
        '▄' => [0, 0, 0, 0, H, H, H, H],
        '▌' => [L; 8],
        '▐' => [R; 8],
        '░' => [0x11, 0, 0x04, 0, 0x11, 0, 0x04, 0],
        '▒' => [0x15, 0x0A, 0x15, 0x0A, 0x15, 0x0A, 0x15, 0x0A],
        '▪' | '▫' | '•' | '·' => [0, 0, 0, 0x06, 0x06, 0, 0, 0],
        '❯' | '›' => [0x10, 0x08, 0x04, 0x02, 0x04, 0x08, 0x10, 0],
        '✓' => [0, 0x01, 0x01, 0x0A, 0x0A, 0x04, 0, 0],
        '✗' | '×' => [0x11, 0x0A, 0x04, 0x04, 0x0A, 0x11, 0, 0],
        '…' => [0, 0, 0, 0, 0, 0, 0x15, 0],
        '→' => [0, 0, 0x04, 0x02, H, 0x02, 0x04, 0],
        '←' => [0, 0, 0x04, 0x08, H, 0x08, 0x04, 0],
        '↑' => [0x04, 0x0E, 0x15, 0x04, 0x04, 0x04, 0x04, 0],
        '↓' => [0x04, 0x04, 0x04, 0x04, 0x15, 0x0E, 0x04, 0],
        // Braille patterns: 2x4 dot matrix encoded in the low byte.
        c @ '\u{2800}'..='\u{28FF}' => {
            let b = c as u32 - 0x2800;
            let mut g = [0u8; 8];
            if b & 0x01 != 0 { g[1] |= 0x08; }
            if b & 0x02 != 0 { g[3] |= 0x08; }
            if b & 0x04 != 0 { g[5] |= 0x08; }
            if b & 0x40 != 0 { g[7] |= 0x08; }
            if b & 0x08 != 0 { g[1] |= 0x02; }
            if b & 0x10 != 0 { g[3] |= 0x02; }
            if b & 0x20 != 0 { g[5] |= 0x02; }
            if b & 0x80 != 0 { g[7] |= 0x02; }
            g
        }
        _ => font['?' as usize],
    }
}

// M38a: iterate CHARS, not bytes — a UTF-8 hanzi used to truncate to four
// garbage ASCII cells. Wide chars (CJK etc.) render through the ab_glyph
// path spanning two cells; ASCII keeps the 5x8 bitmap.
fn draw_text(pix: &mut [u32], pitch: usize, w: usize, h: usize, font: &[[u8; 8]; 128], x: i32, y: i32, s: &str, scale: usize, c: u32) -> i32 {
    let mut cx = x;
    for ch in s.chars() {
        if cjk::char_width(ch) == 2 {
            let box_w = 12 * scale;
            let box_h = 8 * scale;
            if !cjk::draw(pix, pitch, w, h, cx, y, box_w, box_h, box_h as f32, ch, c) {
                let g = font['?' as usize];
                for r in 0..8 {
                    for col in 0..5 {
                        if g[r] & (0x10 >> col) != 0 {
                            fill_rect(
                                pix,
                                pitch,
                                w,
                                h,
                                cx + (col * scale) as i32,
                                y + (r * scale) as i32,
                                scale as i32,
                                scale as i32,
                                c,
                            );
                        }
                    }
                }
            }
            cx += (12 * scale) as i32;
            continue;
        }
        if (ch as u32) >= 0x80
            && cjk::draw(pix, pitch, w, h, cx, y, 6 * scale, 8 * scale, (8 * scale) as f32 * 0.8, ch, c)
        {
            // narrow non-ASCII (—, ·, …) from the CJK subset; bitmap is ASCII-only
            cx += (6 * scale) as i32;
            continue;
        }
        let g = glyph(font, ch);
        for r in 0..8 {
            for col in 0..5 {
                if g[r] & (0x10 >> col) != 0 {
                    fill_rect(
                        pix,
                        pitch,
                        w,
                        h,
                        cx + (col * scale) as i32,
                        y + (r * scale) as i32,
                        scale as i32,
                        scale as i32,
                        c,
                    );
                }
            }
        }
        cx += (6 * scale) as i32;
    }
    cx
}

fn text_w(s: &str, scale: usize) -> usize {
    s.chars()
        .map(|ch| if cjk::char_width(ch) == 2 { 12 } else { 6 })
        .sum::<usize>()
        * scale
}

fn draw_centered(pix: &mut [u32], pitch: usize, w: usize, h: usize, font: &[[u8; 8]; 128], y: i32, s: &str, scale: usize, c: u32) {
    let tw = text_w(s, scale) as i32;
    draw_text(pix, pitch, w, h, font, (w as i32 - tw) / 2, y, s, scale, c);
}

/// Truncate a string to `cols` display columns (CJK counts 2, matching
/// text_w) — voice-face strings come from ASR/SSID scans and can be long.
fn clip_cols(s: &mut String, cols: usize) {
    let mut used = 0usize;
    let mut cut = s.len();
    for (i, ch) in s.char_indices() {
        used += if cjk::char_width(ch) == 2 { 2 } else { 1 };
        if used > cols {
            cut = i;
            break;
        }
    }
    if cut < s.len() {
        s.truncate(cut);
        s.push('…');
    }
}

// ---------------- pty ----------------

struct Child {
    master: std::fs::File,
    pid: libc::pid_t,
}

/// `aginx-pkg available` — optional packages not yet installed, capped at 12
/// (picker row geometry is unsigned arithmetic; scrolling is later).
fn read_available() -> Vec<String> {
    std::process::Command::new(launch::BIN_AGINX_PKG)
        .arg("available")
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .take(12)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn spawn_shell(cols: u16, rows: u16, argv: &[&str]) -> Result<Child, String> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut ws,
        )
    };
    if rc != 0 {
        return Err(format!("openpty: {}", std::io::Error::last_os_error()));
    }
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err("fork failed".into());
    }
    if pid == 0 {
        unsafe {
            libc::setsid();
            libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
            libc::dup2(slave, 0);
            libc::dup2(slave, 1);
            libc::dup2(slave, 2);
            libc::close(master);
            if slave > 2 {
                libc::close(slave);
            }
            // SIG_IGN survives exec, and aginx-term's own ancestry carries one:
            // rcS's busybox sh ignores HUP+INT (observed SigIgn 0x1006 on
            // device, 2026-08-31), adbd ignores INT for adb-run instances.
            // Without this reset every terminal job is immune to ^C — the
            // bytes reach the ldisc, kill_pgrp fires, the disposition
            // discards the signal. Rust std's ignored SIGPIPE is also
            // inherited; shells want the default back.
            for sig in [
                libc::SIGHUP,
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTERM,
                libc::SIGTSTP,
                libc::SIGTTIN,
                libc::SIGTTOU,
                libc::SIGPIPE,
            ] {
                libc::signal(sig, libc::SIG_DFL);
            }
            let empty: libc::sigset_t = std::mem::zeroed();
            libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
            libc::setenv(
                b"TERM\0".as_ptr() as *const _,
                b"xterm-256color\0".as_ptr() as *const _,
                1,
            );
            libc::setenv(b"HOME\0".as_ptr() as *const _, b"/home\0".as_ptr() as *const _, 1);
            let prog = std::ffi::CString::new(argv[0]).unwrap();
            let owned: Vec<std::ffi::CString> =
                argv.iter().map(|a| std::ffi::CString::new(*a).unwrap()).collect();
            let mut cargv: Vec<*const libc::c_char> =
                owned.iter().map(|c| c.as_ptr()).collect();
            cargv.push(std::ptr::null());
            libc::execv(prog.as_ptr(), cargv.as_ptr());
            // exec failed — say so on the pty, then die
            let msg = b"aginx-term: exec failed\r\n";
            libc::write(1, msg.as_ptr() as *const _, msg.len());
            libc::_exit(127);
        }
    }
    unsafe {
        libc::close(slave);
        let flags = libc::fcntl(master, libc::F_GETFL);
        libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    Ok(Child {
        master: unsafe { std::fs::File::from_raw_fd(master) },
        pid,
    })
}

fn child_exited(pid: libc::pid_t) -> bool {
    let mut status: libc::c_int = 0;
    let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    r == pid
}

/// The one input path: encode a KeyEvent/TextInputEvent for the child's
/// current terminal mode and write it to the pty, pulling the echo into
/// the same render frame (the keystroke fast-path from M11). The on-screen
/// keyboard, hold-repeat and — from M18 — voice ASR all come through
/// here; nothing else writes typed input to the pty.
fn inject(mode: &mut Mode, term: &mut Term, parser: &mut vte::Parser, ev: &InputEvent) {
    let bytes = input::encode(ev, term.app_cursor);
    if std::env::var("AGINX_TERM_DEBUG").is_ok() {
        eprintln!("aginx-term: inject {:?} appcur={} -> {} bytes {:?}", ev, term.app_cursor, bytes.len(), String::from_utf8_lossy(&bytes));
    }
    if bytes.is_empty() {
        return; // modifier toggle — consumed by the keyboard, no output
    }
    if let Mode::Running(c) = mode {
        let _ = c.master.write_all(&bytes);
        let mut pfd = libc::pollfd {
            fd: c.master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut pfd, 1, 15) } > 0 {
            let mut buf2 = [0u8; 8192];
            if let Ok(n) = std::io::Read::read(&mut c.master, &mut buf2) {
                for &b in &buf2[..n] {
                    parser.advance(term, b);
                }
                term.jump_live();
            }
        }
    }
}

// ---------------- modes ----------------

enum Mode {
    Launcher,
    Running(Child),
    /// Optional-package picker (launcher "+" tile): rows come from
    /// `aginx-pkg available`; a tap runs `aginx-pkg opt-in <name>` synchronously
    /// (INSTALLING frame drawn first) and refreshes both lists.
    Picker,
    /// Photo viewer (launcher PHOTOS tile, M39): list screen of
    /// /home/photos, then a full-frame view with tap-sides paging.
    /// Decode is libjpeg-turbo (no JPEG decode hardware on SM7250).
    Photos(photos::Photos),
    /// Voice dialog face (launcher VOICE tile, M42a): display-only
    /// rendering of /run/aginx-voice/face, written by the aginx-voice daemon.
    /// No pty, no keyboard — PTT (volume-down) is the input path.
    Voice,
}

// ---------------- voice face ----------------

/// M42a: the JSON aginx-voice writes to /run/aginx-voice/face. Every field defaults
/// so a partially-written doc never kills the renderer; `alive` lives on
/// VoiceView, not here — it means "the file read+parsed at least once".
#[derive(serde::Deserialize, Default)]
struct FaceDoc {
    state: String,
    #[serde(default)]
    listening: bool,
    #[serde(default)]
    busy: bool,
    #[serde(default)]
    /// M42g: viewfinder on — main area renders eye.jpg, lines demote to a
    /// bottom strip. Defaults false so face docs from an older aginx-voice
    /// still render as before.
    eye: bool,
    #[serde(default)]
    lines: Vec<(bool, String)>,
    #[serde(default)]
    hint: String,
}

#[derive(Default)]
struct VoiceView {
    doc: FaceDoc,
    mtime: Option<std::time::SystemTime>,
    alive: bool,
    /// M42g viewfinder frame cache. `poll_eye` gates on eye.jpg mtime; the
    /// decode itself blocks the event loop for a frame the same way the
    /// photo viewer does (DCT-scaled to the box, ~tens of ms; M47⑤ cam-shot
    /// runs resident at ~10fps, so the loop picks up every other frame).
    eye_mtime: Option<std::time::SystemTime>,
    /// M47⑤c: raw-frame mtime gate (the preferred source; eye_mtime/JPEG is
    /// the fallback when cam-shot runs without --raw-out).
    raw_mtime: Option<std::time::SystemTime>,
    eye_img: Option<aginx_img::Bitmap>,
    /// M47⑤f: raw frames no longer build a Bitmap at poll time — the render
    /// pass blits 565→888 fused with the upscale straight into the back
    /// buffer. `raw_dirty` marks a fresh frame; `raw_buf` is the reused
    /// 2.2 MB file buffer (clear + read_to_end keeps the capacity).
    raw_dirty: bool,
    raw_buf: Vec<u8>,
}

impl VoiceView {
    /// mtime-gated poll (same pattern as the /run/aginx-term.inject watch):
    /// stat is one syscall per loop pass, parse only on change. Returns
    /// true when the view changed and needs a repaint. Setting mtime to
    /// None forces the next poll to re-read (mode entry).
    fn poll(&mut self) -> bool {
        let mtime = std::fs::metadata(VOICE_FACE)
            .and_then(|m| m.modified())
            .ok();
        if mtime == self.mtime {
            return false;
        }
        self.mtime = mtime;
        match mtime {
            Some(_) => {
                if let Ok(s) = std::fs::read_to_string(VOICE_FACE) {
                    if let Ok(d) = serde_json::from_str::<FaceDoc>(&s) {
                        self.doc = d;
                        self.alive = true;
                        return true;
                    }
                }
                false
            }
            None => {
                // aginx-voice never wrote / went away — keep the last frame's
                // content but flag it dead
                self.alive = false;
                true
            }
        }
    }

    /// M42g: poll the viewfinder frame. eye=false → drop the cached bitmap
    /// (one repaint so the dialog view comes back clean); eye=true → stat
    /// the frame file and flag a change. Returns true when a repaint is
    /// due; the pixel work itself happens at render time (M47⑤f: the raw
    /// path blits fused straight into the back buffer — a 45 fps publish
    /// must not pay Bitmap-build + canvas detour per present).
    fn poll_eye(&mut self, max_w: u32, max_h: u32) -> bool {
        if !self.doc.eye {
            self.eye_mtime = None;
            self.raw_mtime = None;
            self.raw_dirty = false;
            return self.eye_img.take().is_some();
        }
        // M47⑤c: prefer the raw RGB565 frame (published every frame); the
        // JPEG is the fallback when cam-shot runs without --raw-out.
        let mtime = std::fs::metadata(VOICE_EYE_RAW)
            .and_then(|m| m.modified())
            .ok();
        if let Some(t) = mtime {
            if Some(t) != self.raw_mtime {
                self.raw_mtime = Some(t);
                self.raw_dirty = true;
                return true;
            }
            return false;
        }
        let mtime = std::fs::metadata(VOICE_EYE)
            .and_then(|m| m.modified())
            .ok();
        if mtime.is_none() || mtime == self.eye_mtime {
            return false;
        }
        self.eye_mtime = mtime;
        if let Ok(bytes) = std::fs::read(VOICE_EYE) {
            if let Some(b) = aginx_img::decode_scaled(&bytes, max_w, max_h) {
                self.eye_img = Some(b);
                return true;
            }
        }
        false
    }

    /// M47⑤f: the raw-frame fast path — read eye.raw into the reused
    /// buffer, then 565→888 expand fused with the bilinear upscale
    /// straight into `pix` (the DRM back buffer; every dst pixel written,
    /// so no BG clear needed). Returns false when there is no fresh frame
    /// (or a bad one — the next mtime change retries); the caller then
    /// runs the normal canvas render (dialog / 取景中 / JPEG fallback).
    fn blit_eye_raw(&mut self, pix: &mut [u32], pitch: usize, dw: usize, dh: usize) -> bool {
        if !self.raw_dirty {
            return false;
        }
        self.raw_dirty = false;
        self.raw_buf.clear();
        let ok = std::fs::File::open(VOICE_EYE_RAW)
            .and_then(|mut f| std::io::Read::read_to_end(&mut f, &mut self.raw_buf));
        if ok.is_err() {
            return false;
        }
        let bytes = &self.raw_buf;
        if bytes.len() < 12 {
            return false;
        }
        let rd = |r: std::ops::Range<usize>| -> [u8; 4] { bytes[r].try_into().unwrap() };
        let magic = u32::from_le_bytes(rd(0..4));
        if magic != 0x31574752 {
            return false; // "RGW1"
        }
        let sw = u32::from_le_bytes(rd(4..8)) as usize;
        let sh = u32::from_le_bytes(rd(8..12)) as usize;
        if sw == 0 || sh == 0 || bytes.len() < 12 + sw * sh * 2 || dw == 0 || dh == 0 {
            return false;
        }
        upscale565(pix, pitch, dw, dh, bytes, sw, sh);
        true
    }
}

/// 565→888 bit-replication table, built once (M47⑤f device probe: the
/// scalar shift chain cost ~35 ms CPU per present at 14.9 presents/s —
/// a 256 KB LUT turns the inner loop into two loads and a store).
fn lut565() -> &'static [u32; 65536] {
    static LUT: std::sync::OnceLock<Box<[u32; 65536]>> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let v: Vec<u32> = (0..=u16::MAX)
            .map(|p5| {
                let r = ((p5 >> 11) & 0x1f) as u32;
                let g = ((p5 >> 5) & 0x3f) as u32;
                let b = (p5 & 0x1f) as u32;
                // 5/6-bit → 8-bit replication (31→255, 63→255)
                (((r << 3) | (r >> 2)) << 16) | (((g << 2) | (g >> 4)) << 8) | ((b << 3) | (b >> 2))
            })
            .collect();
        match v.into_boxed_slice().try_into() {
            Ok(b) => b,
            Err(_) => unreachable!("65536 entries"),
        }
    })
}

/// Fused 565→888 expand + bilinear upscale (⑤l: nearest left the live view
/// as a 720-pixel-wide mosaic on the 1080-wide panel — the user saw
/// 「像素超级低」). All resampling happens AFTER the LUT expand: lerping 565
/// codes directly would blend code space (5/6-bit bands), not color.
/// Source-center phase ((i+0.5)·scale − 0.5, Q8 weights); edges replicate
/// (x1 clamps to the last source column/row). The Q8·Q8 corner products are
/// Q16 — each shifts to Q6 so the four-term per-channel sum (≤64·255) stays
/// inside a u16 lane; channels ride split u32 lanes ((b | r<<16) and g) so
/// packed-RGB math never crosses a byte. The 4-px NEON store discipline is
/// kept: the back buffer is write-combined scanout memory, store width is
/// the present budget (M47⑤f device probe 2026-09-05).
fn upscale565(pix: &mut [u32], pitch: usize, dw: usize, dh: usize, src: &[u8], sw: usize, sh: usize) {
    let lut = lut565();
    // per-dst-column taps: x0/x1 source columns and the Q8 weight on x1
    let mut x0 = vec![0usize; dw];
    let mut x1 = vec![0usize; dw];
    let mut wx = vec![0u32; dw];
    for i in 0..dw {
        let fx = (i as f64 + 0.5) * sw as f64 / dw as f64 - 0.5;
        if fx <= 0.0 {
            continue;
        }
        let f = fx.floor();
        let c = (f as usize).min(sw - 1);
        x0[i] = c;
        x1[i] = (c + 1).min(sw - 1);
        wx[i] = (((fx - f) * 256.0) as u32).min(256);
    }
    for j in 0..dh {
        let fy = (j as f64 + 0.5) * sh as f64 / dh as f64 - 0.5;
        let (y0, y1, wy) = if fy <= 0.0 {
            (0usize, 0usize, 0u32)
        } else {
            let f = fy.floor();
            let r = (f as usize).min(sh - 1);
            (r, (r + 1).min(sh - 1), (((fy - f) * 256.0) as u32).min(256))
        };
        let r0 = 12 + y0 * sw * 2;
        let r1 = 12 + y1 * sw * 2;
        let dst = j * pitch;
        let mut i = 0;
        #[cfg(target_arch = "aarch64")]
        // every aarch64 intrinsic is #[target_feature] = unsafe to call;
        // the whole quad loop (closures included, edition 2021) rides one
        // unsafe context
        unsafe {
            use core::arch::aarch64::{
                uint32x4_t, vaddq_u32, vandq_u32, vdupq_n_u32, vld1q_u32,
                vmulq_u32, vorrq_u32, vshlq_n_u32, vshrq_n_u32, vst1q_u32,
            };
            let mask_rb = vdupq_n_u32(0x00FF_00FF);
            let mask_g = vdupq_n_u32(0x00FF_0000);
            let bias = vdupq_n_u32(0x0020_0020);
            // 4 px per iteration, stored with one 128-bit NEON store.
            while i + 4 <= dw {
                let mut q00 = [0u32; 4];
                let mut q01 = [0u32; 4];
                let mut q10 = [0u32; 4];
                let mut q11 = [0u32; 4];
                let mut wa = [0u32; 4];
                let mut wb = [0u32; 4];
                let mut wc = [0u32; 4];
                let mut wd = [0u32; 4];
                for k in 0..4 {
                    let c = i + k;
                    let t0 = x0[c] * 2;
                    let t1 = x1[c] * 2;
                    let s = |o: usize| {
                        lut[u16::from_le_bytes([src[o], src[o + 1]]) as usize]
                    };
                    q00[k] = s(r0 + t0);
                    q01[k] = s(r0 + t1);
                    q10[k] = s(r1 + t0);
                    q11[k] = s(r1 + t1);
                    let w = wx[c];
                    let iw = 256 - w;
                    // Q6 corner weights, sum exactly 64
                    wa[k] = (iw * (256 - wy)) >> 10;
                    wb[k] = (w * (256 - wy)) >> 10;
                    wc[k] = (iw * wy) >> 10;
                    wd[k] = (w * wy) >> 10;
                }
                let v00 = vld1q_u32(q00.as_ptr());
                let v01 = vld1q_u32(q01.as_ptr());
                let v10 = vld1q_u32(q10.as_ptr());
                let v11 = vld1q_u32(q11.as_ptr());
                let vwa = vld1q_u32(wa.as_ptr());
                let vwb = vld1q_u32(wb.as_ptr());
                let vwc = vld1q_u32(wc.as_ptr());
                let vwd = vld1q_u32(wd.as_ptr());
                // per 16-bit lane: (Σ p·weight + bias) >> 6 — the bias is 32
                // at EACH lane's own scale (0x20 at bits [5:0] for the b
                // lane, 0x20<<16 for the r/g lane at [21:16])
                let mix = |a: uint32x4_t, b: uint32x4_t, c: uint32x4_t,
                           d: uint32x4_t| {
                    vshrq_n_u32(
                        vaddq_u32(
                            vaddq_u32(vmulq_u32(a, vwa), vmulq_u32(b, vwb)),
                            vaddq_u32(
                                vaddq_u32(vmulq_u32(c, vwc), vmulq_u32(d, vwd)),
                                bias,
                            ),
                        ),
                        6,
                    )
                };
                let lo = |v: uint32x4_t| vandq_u32(v, mask_rb);
                // G sits at bits [15:8]; shift it up into lane 1 ([31:16])
                let hi = |v: uint32x4_t| vandq_u32(vshlq_n_u32(v, 8), mask_g);
                let rb = mix(lo(v00), lo(v01), lo(v10), lo(v11));
                let gb = mix(hi(v00), hi(v01), hi(v10), hi(v11));
                let quad = vorrq_u32(
                    vandq_u32(rb, mask_rb),
                    vandq_u32(vshrq_n_u32(gb, 8), vdupq_n_u32(0x0000_FF00)),
                );
                vst1q_u32(pix[dst + i..].as_mut_ptr(), quad);
                i += 4;
            }
        }
        while i < dw {
            let s = |o: usize| lut[u16::from_le_bytes([src[o], src[o + 1]]) as usize];
            pix[dst + i] = bilerp888(
                s(r0 + x0[i] * 2),
                s(r0 + x1[i] * 2),
                s(r1 + x0[i] * 2),
                s(r1 + x1[i] * 2),
                wx[i],
                wy,
            );
            i += 1;
        }
    }
}

/// scalar bilinear on expanded 888 pixels — the tail path (dw % 4) and the
/// host unit test's reference. Same lane-split Q6 math as the NEON quad.
#[inline]
fn bilerp888(p00: u32, p01: u32, p10: u32, p11: u32, wx: u32, wy: u32) -> u32 {
    let iw = 256 - wx;
    let jw = 256 - wy;
    let wa = (iw * jw) >> 10;
    let wb = (wx * jw) >> 10;
    let wc = (iw * wy) >> 10;
    let wd = (wx * wy) >> 10;
    let mix = |a: u32, b: u32, c: u32, d: u32| {
        ((a & 0x00FF_00FF) * wa
            + (b & 0x00FF_00FF) * wb
            + (c & 0x00FF_00FF) * wc
            + (d & 0x00FF_00FF) * wd
            + 0x0020_0020)
            >> 6
    };
    // G sits at bits [15:8]; shift it up into lane 1 ([31:16])
    let mixh = |a: u32, b: u32, c: u32, d: u32| {
        (((a << 8) & 0x00FF_0000) * wa
            + ((b << 8) & 0x00FF_0000) * wb
            + ((c << 8) & 0x00FF_0000) * wc
            + ((d << 8) & 0x00FF_0000) * wd
            + 0x0020_0020)
            >> 6
    };
    let rb = mix(p00, p01, p10, p11);
    let gb = mixh(p00, p01, p10, p11);
    (rb & 0x00FF_00FF) | ((gb >> 8) & 0x0000_FF00)
}

// ---------------- render ----------------

struct Render<'a> {
    font: &'a [[u8; 8]; 128],
    w: usize,
    h: usize,
    pitch: usize,
}

impl<'a> Render<'a> {
    fn launcher(&self, pix: &mut [u32], entries: &[launch::Entry], g: &launch::Geom) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.toolbar_h as i32 + 14, "AGINXOS", 5, GREEN);
        for (i, e) in entries.iter().enumerate() {
            let y0 = (g.by0 + i * (g.bh + g.gap)) as i32;
            let c = if e.avail { DIM } else { 0x000F1A14 };
            // button outline
            fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0, g.bw as i32, 3, c);
            fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0 + g.bh as i32 - 3, g.bw as i32, 3, c);
            fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0, 3, g.bh as i32, c);
            fill_rect(pix, self.pitch, self.w, self.h, (g.bx + g.bw - 3) as i32, y0, 3, g.bh as i32, c);
            let scale = 5;
            let tw = text_w(e.label.as_str(), scale) as i32;
            let ty = y0 + (g.bh as i32 - 8 * scale as i32) / 2;
            let tc = if e.avail { GREEN } else { UNAVAIL };
            draw_text(pix, self.pitch, self.w, self.h, self.font, g.bx as i32 + (g.bw as i32 - tw) / 2, ty, e.label.as_str(), scale, tc);
            if !e.avail {
                draw_centered(pix, self.pitch, self.w, self.h, self.font, y0 + g.bh as i32 - 30, "(NOT INSTALLED)", 2, UNAVAIL);
            }
        }
        // hint line
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.kb_panel_y as i32 - 40, "TAP TO START", 3, UNAVAIL);
    }

    /// Optional-package picker ("+" tile): same row geometry as the
    /// launcher. status_line is the last install result ("" = hint).
    /// The caller caps the list at 12 rows — Geom arithmetic is unsigned
    /// and a long list would underflow; scrolling is a later milestone.
    fn picker(&self, pix: &mut [u32], names: &[String], status_line: &str, g: &launch::Geom) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
        self.toolbar(pix, g.m, g.toolbar_h);
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.toolbar_h as i32 + 14, "SELECT PKGS", 5, GREEN);
        if names.is_empty() {
            draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 24) / 2, "(NONE AVAILABLE)", 3, UNAVAIL);
        }
        for (i, n) in names.iter().enumerate() {
            let y0 = (g.by0 + i * (g.bh + g.gap)) as i32;
            let c = DIM;
            fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0, g.bw as i32, 3, c);
            fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0 + g.bh as i32 - 3, g.bw as i32, 3, c);
            fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0, 3, g.bh as i32, c);
            fill_rect(pix, self.pitch, self.w, self.h, (g.bx + g.bw - 3) as i32, y0, 3, g.bh as i32, c);
            let tw = text_w(n.as_str(), 5) as i32;
            let ty = y0 + (g.bh as i32 - 8 * 5) / 2;
            draw_text(pix, self.pitch, self.w, self.h, self.font, g.bx as i32 + (g.bw as i32 - tw) / 2, ty, n.as_str(), 5, GREEN);
        }
        let line = if status_line.is_empty() { "TAP TO INSTALL" } else { status_line };
        let lc = if status_line.is_empty() { UNAVAIL } else { GREEN };
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.kb_panel_y as i32 - 40, line, 3, lc);
    }

    /// Full-cover frame shown while `aginx-pkg opt-in` runs (synchronous —
    /// the event loop is blocked, so this must be painted + presented
    /// before the Command).
    fn installing(&self, pix: &mut [u32], name: &str) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
        draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 8 * 5) / 2 - 60, "INSTALLING", 5, GREEN);
        draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 8 * 5) / 2 + 60, name, 5, WHITE);
    }

    /// LOADING frame while a JPEG decodes (same synchronous-block pattern
    /// as `installing` — paint, present, then block in libjpeg).
    fn loading(&self, pix: &mut [u32]) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
        draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 8 * 5) / 2, "LOADING", 5, GREEN);
    }

    /// Photo list screen (M39): picker-style rows of /home/photos
    /// basenames, newest first, capped at 12 rows like the picker (Geom
    /// arithmetic is unsigned; scrolling is a later milestone).
    fn photos_list(&self, pix: &mut [u32], p: &photos::Photos, g: &launch::Geom) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
        self.toolbar(pix, g.m, g.toolbar_h);
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.toolbar_h as i32 + 14, "PHOTOS", 5, GREEN);
        let names = p.names();
        if names.is_empty() {
            draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 24) / 2, "(NO PHOTOS)", 3, UNAVAIL);
            draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 24) / 2 + 60, "AG CAM-SHOT --JPEG-OUT /HOME/PHOTOS/...", 2, UNAVAIL);
        } else {
            for (i, n) in names.iter().take(12).enumerate() {
                let y0 = (g.by0 + i * (g.bh + g.gap)) as i32;
                fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0, g.bw as i32, 3, DIM);
                fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0 + g.bh as i32 - 3, g.bw as i32, 3, DIM);
                fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0, 3, g.bh as i32, DIM);
                fill_rect(pix, self.pitch, self.w, self.h, (g.bx + g.bw - 3) as i32, y0, 3, g.bh as i32, DIM);
                let n: String = n.chars().take(24).collect();
                let tw = text_w(&n, 5) as i32;
                let ty = y0 + (g.bh as i32 - 8 * 5) / 2;
                draw_text(pix, self.pitch, self.w, self.h, self.font, g.bx as i32 + (g.bw as i32 - tw) / 2, ty, &n, 5, GREEN);
            }
        }
        let total = names.len();
        let footer = if !p.err.is_empty() {
            p.err.clone()
        } else if total == 0 {
            "TAP BACK TO EXIT".to_string()
        } else if total > 12 {
            format!("{total} PHOTOS - NEWEST 12")
        } else {
            format!("{total} PHOTO{} - TAP TO VIEW", if total == 1 { "" } else { "S" })
        };
        let fc = if !p.err.is_empty() { GREEN } else { UNAVAIL };
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.kb_panel_y as i32 - 40, &footer, 3, fc);
    }

    /// Full-screen photo view: decoded bitmap blitted 1:1, centered under
    /// the toolbar (decode already DCT-scaled to fit the box), filename in
    /// the footer. Tap the right half for next, left for previous.
    fn photo_view(&self, pix: &mut [u32], p: &photos::Photos, g: &launch::Geom) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
        self.toolbar(pix, g.m, g.toolbar_h);
        if let Some(b) = &p.img {
            let dx = ((self.w - b.w as usize) / 2) as i32;
            let avail_h = self.h - g.toolbar_h as usize;
            let dy = (g.toolbar_h as i32 + ((avail_h - b.h as usize) / 2) as i32).max(g.toolbar_h as i32);
            for j in 0..b.h as usize {
                let py = dy + j as i32;
                if py < 0 || py >= self.h as i32 {
                    continue;
                }
                for i in 0..b.w as usize {
                    let px = dx + i as i32;
                    if px < 0 || px >= self.w as i32 {
                        continue;
                    }
                    pix[py as usize * self.pitch + px as usize] = b.pix[j * b.w as usize + i];
                }
            }
        }
        let name = p.names().get(p.sel).cloned().unwrap_or_default();
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.kb_panel_y as i32 - 36, &name, 3, WHITE);
        draw_centered(pix, self.pitch, self.w, self.h, self.font, self.h as i32 - 30, "< TAP TO PAGE >", 2, DIM);
    }

    /// Voice dialog face (M42a, launcher VOICE tile): pure rendering of
    /// the doc aginx-voice writes to /run/aginx-voice/face. Phosphor rules — agent
    /// lines green, user lines white (prefixed ">"), selected SSID white,
    /// psk shown verbatim (read-back confirmation needs to be visible).
    /// No touch targets below the BACK toolbar: the screen is a display.
    ///
    /// M42g eye=true: the screen is the result canvas, not a chat log — the
    /// live viewfinder frame takes the body, dialog lines demote to a
    /// bottom strip (scale 2), and the hint explains the eye keys.
    fn voice(&self, pix: &mut [u32], v: &VoiceView, g: &launch::Geom) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
        self.toolbar(pix, g.m, g.toolbar_h);
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.toolbar_h as i32 + 14, "VOICE", 5, GREEN);
        if !v.alive {
            draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 8 * 3) / 2, "(语音服务未运行)", 3, UNAVAIL);
            return;
        }
        let d = &v.doc;
        // status strip under the title
        if d.listening {
            draw_centered(pix, self.pitch, self.w, self.h, self.font, g.toolbar_h as i32 + 90, "正在听", 4, WHITE);
        } else if d.busy {
            draw_centered(pix, self.pitch, self.w, self.h, self.font, g.toolbar_h as i32 + 90, "处理中", 4, DIM);
        }
        // M47⑤b eye=true: fullscreen viewfinder (user receipt 2026-09-05
        // 「界面要做成全屏」) — the frame fills the whole panel (eye box =
        // (0,0,w,h)), covering toolbar/title/strips. Nothing else draws
        // while frames flow; the close keys are physical (音量+ toggles,
        // 音量下).
        if d.eye {
            fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
            if let Some(b) = &v.eye_img {
                let (_, _, bw, bh) = g.eye_box();
                if bh > 0 && b.w > 0 && b.h > 0 {
                    // aspect-FILL by nearest-neighbor upscale (decode_scaled
                    // only downscales; 720→1080 upscaling lives here). The
                    // frame's --aspect already matches the box.
                    let (dw, dh) = (bw, bh);
                    let (sw, sh) = (b.w as usize, b.h as usize);
                    let mut sx = vec![0usize; dw];
                    for (i, s) in sx.iter_mut().enumerate() {
                        *s = i * sw / dw;
                    }
                    for j in 0..dh {
                        let row = (j * sh / dh) * sw;
                        let dst = j * self.pitch;
                        for i in 0..dw {
                            pix[dst + i] = b.pix[row + sx[i]];
                        }
                    }
                }
            } else {
                // 第一帧在路上（cam-shot 3 帧曝光要 ~2s）
                draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 8 * 4) / 2, "取景中…", 4, GREEN);
            }
            return;
        }
        // fresh boot, nothing said yet: the one big affordance
        if d.state == "idle" && d.lines.is_empty() {
            draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 8 * 4) / 2, "按住音量下键说：连接无线网络", 4, GREEN);
        }
        let mut y = g.toolbar_h as i32 + 170;
        // dialog transcript: last 6 lines, user white / agent green (user
        // receipt 2026-09-04: 对话行太小 → scale 4)
        for (is_user, line) in d.lines.iter().rev().take(6).rev() {
            let (c, pfx) = if *is_user { (WHITE, ">") } else { (GREEN, "") };
            let mut s = format!("{pfx}{line}");
            clip_cols(&mut s, 48);
            draw_text(pix, self.pitch, self.w, self.h, self.font, g.m as i32, y, &s, 4, c);
            y += 72;
        }
        // hint line (aginx-voice's default: how to talk, how to bail)
        let hint = if d.hint.is_empty() { "按住音量下键说：连接无线网络" } else { d.hint.as_str() };
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.kb_panel_y as i32 - 40, hint, 3, UNAVAIL);
    }

    /// Header strip: [BACK] at the right, like the launcher header —
    /// nothing else, so the content below stays uncovered.
    fn toolbar(&self, pix: &mut [u32], m: usize, strip_h: usize) {
        let (w, h) = (self.w, self.h);
        fill_rect(pix, self.pitch, w, h, m as i32, strip_h as i32, (w - 2 * m) as i32, 2, DIM);
        let ty = (strip_h as i32 - 8 * 3) / 2;
        draw_text(pix, self.pitch, w, h, self.font, (w - m) as i32 - text_w("BACK", 3) as i32, ty, "BACK", 3, GREEN);
    }

    /// Row-damaged render: only rows the Term marked dirty are repainted
    /// (bg fill + glyphs + cursor). The full-screen fill is gone — the
    /// canvas in main() persists between frames.
    fn terminal(&self, pix: &mut [u32], t: &Term, area_top: usize, _area_h: usize, scale: usize, blink_on: bool, x_off: usize) {
        let (w, h) = (self.w, self.h);
        let cell_w = 6 * scale;
        let cell_h = 8 * scale;
        let stride = cell_h + ROW_GAP;
        for row in 0..t.rows {
            if !t.row_dirty()[row] {
                continue;
            }
            let y = area_top + row * stride;
            fill_rect(pix, self.pitch, w, h, x_off as i32, y as i32, (w - 2 * x_off) as i32, stride as i32, BG);
            let line = t.render_line(row);
            let mut x = x_off;
            for cell in &line {
                if cell.ch == term::WIDE_TAIL {
                    x += cell_w;
                    continue;
                }
                if cell.ch != ' ' {
                    let mut c = match cell.style {
                        Style::Normal => GREEN,
                        Style::Bright => WHITE,
                        Style::Inverse => GREEN,
                    };
                    if matches!(cell.style, Style::Inverse) {
                        let wcells = if cjk::char_width(cell.ch) == 2 { 2 * cell_w } else { cell_w };
                        fill_rect(pix, self.pitch, w, h, x as i32, y as i32, wcells as i32, cell_h as i32, GREEN);
                        c = BG;
                    }
                    if cjk::char_width(cell.ch) == 2
                        && cjk::draw(pix, self.pitch, w, h, x as i32, y as i32, 2 * cell_w, cell_h, cell_h as f32, cell.ch, c)
                    {
                        // rendered from the CJK subset
                    } else if (cell.ch as u32) >= 0x80
                        && cjk::draw(pix, self.pitch, w, h, x as i32, y as i32, cell_w, cell_h, cell_h as f32 * 0.8, cell.ch, c)
                    {
                        // narrow non-ASCII (—, ·, …, °): width-1 but only the
                        // CJK subset has the glyph — bitmap font is ASCII-only
                    } else {
                        let g = glyph(self.font, cell.ch);
                        for r in 0..8 {
                            for col in 0..5 {
                                if g[r] & (0x10 >> col) != 0 {
                                    fill_rect(
                                        pix,
                                        self.pitch,
                                        w,
                                        h,
                                        (x + col * scale) as i32,
                                        (y + r * scale) as i32,
                                        scale as i32,
                                        scale as i32,
                                        c,
                                    );
                                }
                            }
                        }
                    }
                }
                x += cell_w;
            }
            if row == t.cursor_y && t.cursor_visible && t.view_offset == 0 && blink_on {
                fill_rect(
                    pix,
                    self.pitch,
                    w,
                    h,
                    (x_off + t.cursor_x * cell_w) as i32,
                    (y + cell_h - 2) as i32,
                    cell_w as i32,
                    2,
                    GREEN,
                );
            }
        }
    }

    fn keyboard(&self, pix: &mut [u32], kg: &KeyGeom, kb: &Kb) {
        let (w, h) = (self.w, self.h);
        let m = kg.x_off;
        fill_rect(pix, self.pitch, w, h, m as i32, kg.extra_y as i32, (w - 2 * m) as i32, (h - kg.extra_y) as i32, 0x00050A08);
        fill_rect(pix, self.pitch, w, h, m as i32, kg.extra_y as i32 - 2, (w - 2 * m) as i32, 2, DIM);
        // One gap constant spaces every keycap on the keyboard (2026-09-02):
        // caps are cells inset by gap/2, so H and V seams are all gap wide.
        let gi = kg.gap / 2;
        // extra-keys row (Termux): ESC TAB CTL < v ^ > — labels from the
        // key table, arrows drawn bigger than text labels
        let ekw = (w - 2 * m) / kb::EXTRA_KEYS.len();
        for (i, kd) in kb::EXTRA_KEYS.iter().enumerate() {
            let x0 = m + i * ekw + gi;
            let y0 = kg.extra_y + gi;
            let active = self.mod_active(kd, kb) || kb.is_pressed(kb::AREA_EXTRA, 0, i as u8);
            let ks = if i >= 3 { 5 } else { 3 };
            self.keycap(pix, x0, y0, ekw - kg.gap, kg.extra_h - kg.gap, kd.label, ks, active);
        }
        // M40b iOS letter block. Grid rows 0-1: uniform cells (10 then 9
        // keys); lowercase labels, shift shows caps.
        let grids = kb.grids();
        for r in 0..2 {
            let s = grids[r];
            let n = s.len();
            let cw = (w - 2 * m) / n;
            for (col, ch) in s.chars().enumerate() {
                let lbl = if kb.shift_on() && kb.page() == kb::Page::Letters {
                    ch.to_ascii_uppercase().to_string()
                } else {
                    ch.to_string()
                };
                let x0 = m + col * cw + gi;
                let y0 = kg.panel_y + r * kg.cell_h + gi;
                let lit = kb.is_pressed(kb::AREA_PANEL, r as u8, col as u8);
                self.keycap(pix, x0, y0, cw - kg.gap, kg.cell_h - kg.gap, &lbl, kg.label_scale, lit);
            }
        }
        // Rows 2-3: weighted keys normalized to the span (shift+7+delete;
        // 123 拼 space 。 换行).
        for (r, row) in [kb.row2(), kb.row3()].iter().enumerate() {
            let units: usize = row.iter().map(|k| k.w).sum();
            let mut acc = 0usize;
            for (idx, kd) in row.iter().enumerate() {
                let x0 = m + kg.span * acc / units + gi;
                let kw = kg.span * kd.w / units - kg.gap;
                acc += kd.w;
                let y0 = kg.panel_y + (r + 2) * kg.cell_h + gi;
                let lit = kb.is_pressed(kb::AREA_PANEL, (r + 2) as u8, idx as u8);
                let (lbl, ls): (&str, usize) = match &kd.act {
                    kb::Act::Letter(c) => {
                        // owned labels for the shift case — draw and move on
                        let up = if kb.shift_on() && kb.page() == kb::Page::Letters {
                            c.to_ascii_uppercase().to_string()
                        } else {
                            c.to_string()
                        };
                        self.keycap(pix, x0, y0, kw, kg.cell_h - kg.gap, &up, kg.label_scale, lit);
                        continue;
                    }
                    // 拼/空格/换行 share one label size (user round-2 ②)
                    kb::Act::Period => (if kb.pinyin_on() { "。" } else { "." }, 4),
                    kb::Act::Space => (if kb.pinyin_on() { "空格" } else { "" }, 4),
                    _ => (kd.label, 4),
                };
                let active = self.mod_active(kd, kb) || lit;
                self.keycap(pix, x0, y0, kw, kg.cell_h - kg.gap, lbl, ls, active);
            }
        }
    }

    /// Modifier keycaps light up while their one-shot is armed.
    fn mod_active(&self, kd: &KeyDef, kb: &Kb) -> bool {
        match kd.act {
            Act::Ctrl => kb.ctrl_on(),
            Act::Shift => kb.shift_on(),
            Act::Pinyin => kb.pinyin_on(),
            Act::Page(_) | Act::Period | Act::Space | Act::Letter(_) | Act::Text(_) | Act::Ev(_) => false,
        }
    }

    /// M40 candidate strip, drawn every render pass while 拼 is on (it
    /// floats over terminal rows that repaint on blink — drawing it only
    /// on keyboard-dirty frames would let a cursor blink erase it).
    /// 8 slots of w/8: composing buffer | 6 candidates | page arrow.
    fn ime_strip(&self, pix: &mut [u32], ime: &pinyin::Ime, kg: &KeyGeom) {
        let (w, h) = (self.w, self.h);
        let y0 = kg.extra_y.saturating_sub(IME_STRIP_H);
        fill_rect(pix, self.pitch, w, h, 0, y0 as i32, w as i32, (IME_STRIP_H - 2) as i32, BG);
        fill_rect(pix, self.pitch, w, h, 0, y0 as i32, w as i32, 2, DIM);
        let sw = w / 8;
        for slot in 0..8 {
            let x0 = slot * sw;
            if slot > 0 {
                fill_rect(pix, self.pitch, w, h, x0 as i32, y0 as i32 + 10, 2, (IME_STRIP_H - 20) as i32, DIM);
            }
            match slot {
                // composing pinyin (dim 拼 hint while idle) — scale 3 fits
                // the longest syllable "zhuang" in the slot
                0 => {
                    let s: &str = if ime.buf.is_empty() { "拼" } else { &ime.buf };
                    let c = if ime.buf.is_empty() { DIM } else { WHITE };
                    let tw = text_w(s, 3) as i32;
                    draw_text(pix, self.pitch, w, h, self.font, x0 as i32 + (sw as i32 - tw) / 2, y0 as i32 + (IME_STRIP_H as i32 - 8 * 3) / 2, s, 3, c);
                }
                // page arrow — dim when everything fits on one page
                7 => {
                    let len = ime.candidates().len();
                    let pages = if len == 0 { 0 } else { (len + pinyin::PAGE - 1) / pinyin::PAGE };
                    let c = if pages > 1 { GREEN } else { DIM };
                    let tw = text_w("›", 5) as i32;
                    draw_text(pix, self.pitch, w, h, self.font, x0 as i32 + (sw as i32 - tw) / 2, y0 as i32 + (IME_STRIP_H as i32 - 8 * 5) / 2, "›", 5, c);
                }
                // candidate hanzi: wide-glyph path, ~80 px in the 120 px strip
                i => {
                    if let Some(ch) = ime.page_candidate(i - 1) {
                        let s = ch.to_string();
                        let tw = text_w(&s, 10) as i32;
                        draw_text(pix, self.pitch, w, h, self.font, x0 as i32 + (sw as i32 - tw) / 2, y0 as i32 + (IME_STRIP_H as i32 - 8 * 10) / 2, &s, 10, GREEN);
                    }
                }
            }
        }
    }

    fn keycap(&self, pix: &mut [u32], x0: usize, y0: usize, kw: usize, kh: usize, label: &str, scale: usize, active: bool) {
        let (w, h) = (self.w, self.h);
        let edge = if active { GREEN } else { DIM };
        fill_rect(pix, self.pitch, w, h, x0 as i32, y0 as i32, kw as i32, 2, edge);
        fill_rect(pix, self.pitch, w, h, x0 as i32, (y0 + kh) as i32 - 2, kw as i32, 2, edge);
        fill_rect(pix, self.pitch, w, h, x0 as i32, y0 as i32, 2, kh as i32, edge);
        fill_rect(pix, self.pitch, w, h, (x0 + kw) as i32 - 2, y0 as i32, 2, kh as i32, edge);
        fill_rect(pix, self.pitch, w, h, x0 as i32 + 2, y0 as i32 + 2, kw as i32 - 4, kh as i32 - 4, KEYCAP);
        let ls = scale;
        let tw = text_w(label, ls) as i32;
        let tc = if active { WHITE } else { GREEN };
        draw_text(
            pix,
            self.pitch,
            w,
            h,
            self.font,
            x0 as i32 + (kw as i32 - tw) / 2,
            y0 as i32 + (kh as i32 - 8 * ls as i32) / 2,
            label,
            ls,
            tc,
        );
    }
}

// ---------------- PPM host mode ----------------

fn ppm_dump(path: &str, pix: &[u32], w: usize, h: usize, pitch: usize) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{} {}\n255\n", w, h)?;
    let mut row = Vec::with_capacity(w * 3);
    for y in 0..h {
        row.clear();
        for x in 0..w {
            let p = pix[y * pitch + x];
            row.push(((p >> 16) & 0xff) as u8);
            row.push(((p >> 8) & 0xff) as u8);
            row.push((p & 0xff) as u8);
        }
        f.write_all(&row)?;
    }
    Ok(())
}

fn kb0() -> Kb {
    Kb::new()
}

/// M15 shutdown: draw a farewell frame, show it, then hand the machine to
/// `aginx-reboot poweroff` (sync + reboot(RB_POWER_OFF) — the PMIC cuts power).
/// Never returns.
fn power_off(d: &mut Drm, font: &[[u8; 8]; 128], canvas: &mut [u32], blanked: bool) {
    let (w, h, pitch) = (d.width as usize, d.height as usize, d.pitch_px());
    fill_rect(canvas, pitch, w, h, 0, 0, w as i32, h as i32, BG);
    draw_centered(canvas, pitch, w, h, font, (h as i32 - 8 * 5) / 2, "POWERING OFF", 5, GREEN);
    d.back_buf().copy_from_slice(canvas);
    if blanked {
        d.dpms(true); // relatch the farewell frame even if we were blanked
    } else {
        d.present();
    }
    let _ = std::process::Command::new(launch::BIN_AGINX_REBOOT).arg("poweroff").spawn();
    std::process::exit(0);
}

fn host_ppm(out: &str) {
    let font = font::font_init();
    let (w, h) = (1080usize, 2340usize);
    let pitch = w;
    let mut pix = vec![0u32; pitch * h];
    let kg = Kb::geom(w, h);
    let entries = launch::entries();
    let lg = launch::Geom::new(w, h, kg.extra_y, entries.len());
    let r = Render { font: &font, w, h, pitch };
    r.launcher(&mut pix, &entries, &lg);
    r.keyboard(&mut pix, &kg, &kb0());

    // second frame: terminal view with a fake session (M38a: includes a
    // UTF-8 Chinese line so the wide-cell put + ab_glyph render path is
    // exercised on the host — AGINX_TERM_CJK_FONT points at the subset)
    let area_top0 = lg.toolbar_h + 20;
    let area_h0 = kg.extra_y - area_top0;
    let sc0 = 6usize;
    let mut t = Term::new((w - 2 * kb::KB_M) / (6 * sc0), area_h0 / (8 * sc0));
    let mut parser = vte::Parser::new();
    let demo_owned = std::env::var("AGINX_TERM_PPM_DEMO").unwrap_or_else(|_| {
        "root@aginxos:~# uname -a\r\nLinux aginxos 5.4.61-android13 aarch64\r\nroot@aginxos:~# \x1b[1mecho '你好，世界'\x1b[0m\r\n你好，世界 — 化身·互联·记忆在线\r\nroot@aginxos:~# ".to_string()
    });
    let demo: &[u8] = demo_owned.as_bytes();
    for &b in demo {
        parser.advance(&mut t, b);
    }
    let mut pix2 = vec![0u32; pitch * h];
    fill_rect(&mut pix2, pitch, w, h, 0, 0, w as i32, h as i32, BG);
    r.toolbar(&mut pix2, kb::KB_M, lg.toolbar_h);
    r.terminal(&mut pix2, &t, area_top0, area_h0, sc0, true, kb::KB_M);
    r.keyboard(&mut pix2, &kg, &kb0());
    let term_path = format!("{}-term", out);
    if let Err(e) = ppm_dump(out, &pix, w, h, pitch) {
        eprintln!("ppm: {e}");
    }
    if let Err(e) = ppm_dump(&term_path, &pix2, w, h, pitch) {
        eprintln!("ppm: {e}");
    }

    // third frame (M39): photo view — AGINX_TERM_PHOTOS_DEMO=<file.jpg> decodes
    // through aginx-img (DCT-scaled to the panel box) and renders the real
    // viewer screen, so the decode+blit path is host-verifiable.
    if let Ok(demo) = std::env::var("AGINX_TERM_PHOTOS_DEMO") {
        let bytes = std::fs::read(&demo).unwrap_or_default();
        let mut p = photos::Photos {
            files: vec![demo.clone()],
            sel: 0,
            img: aginx_img::decode_scaled(&bytes, w as u32, (h - lg.toolbar_h) as u32),
            view: true,
            err: String::new(),
        };
        if p.img.is_none() {
            p.err = "DECODE FAILED".into();
            p.view = false;
        }
        let mut pix3 = vec![0u32; pitch * h];
        if p.view {
            r.photo_view(&mut pix3, &p, &lg);
        } else {
            r.photos_list(&mut pix3, &p, &lg);
        }
        let photo_path = format!("{}-photo", out);
        if let Err(e) = ppm_dump(&photo_path, &pix3, w, h, pitch) {
            eprintln!("ppm: {e}");
        }
        println!("wrote {photo_path}");
    }

    // fourth frame (M40): pinyin IME — AGINX_TERM_IME_DEMO=<syllable> latches
    // 拼 on, types the syllable into the buffer and renders the strip over
    // the demo session, so the candidate row is host-verifiable.
    if let Ok(syl) = std::env::var("AGINX_TERM_IME_DEMO") {
        let mut k = kb0();
        k.set_pinyin(true);
        let mut ime = pinyin::Ime::new();
        for c in syl.chars().filter(|c| c.is_ascii_lowercase()) {
            ime.feed(&InputEvent::Text(c.to_string()));
        }
        let mut pix4 = vec![0u32; pitch * h];
        fill_rect(&mut pix4, pitch, w, h, 0, 0, w as i32, h as i32, BG);
        r.toolbar(&mut pix4, kb::KB_M, lg.toolbar_h);
        r.terminal(&mut pix4, &t, area_top0, area_h0, sc0, true, kb::KB_M);
        r.keyboard(&mut pix4, &kg, &k);
        r.ime_strip(&mut pix4, &ime, &kg);
        let ime_path = format!("{}-ime", out);
        if let Err(e) = ppm_dump(&ime_path, &pix4, w, h, pitch) {
            eprintln!("ppm: {e}");
        }
        println!("wrote {ime_path}");
    }
    println!("wrote {out} and {out}-term");
}

// ---------------- M47⑤f frame-arrival watch ----------------
// cam-shot publishes eye.raw (and eye.jpg) by tmp+rename into
// /run/aginx-voice, so an IN_MOVED_TO watch on the directory fires exactly
// when a complete frame lands — the loop wakes on the frame itself instead
// of stat-polling on a timer whose 12 ms cadence misaligned with the 22 ms
// publish (device probe 2026-09-05). No-op off linux so host tests build.

#[cfg(target_os = "linux")]
fn ino_init() -> (libc::c_int, libc::c_int) {
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return (-1, -1);
    }
    (fd, ino_rearm(fd))
}

/// (Re)arm the watch — idempotent, safe to retry until the voice daemon
/// has created the directory (it may not exist when term starts at boot).
#[cfg(target_os = "linux")]
fn ino_rearm(fd: libc::c_int) -> libc::c_int {
    if fd < 0 {
        return -1;
    }
    unsafe {
        libc::inotify_add_watch(
            fd,
            b"/run/aginx-voice\0".as_ptr() as *const _,
            libc::IN_MOVED_TO,
        )
    }
}

/// Empty the queue — level-triggered poll stays readable until drained.
#[cfg(target_os = "linux")]
fn ino_drain(fd: libc::c_int) {
    let mut buf = [0u8; 1024];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n <= 0 {
            break; // EAGAIN — drained
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn ino_init() -> (libc::c_int, libc::c_int) {
    (-1, -1)
}
#[cfg(not(target_os = "linux"))]
fn ino_rearm(_fd: libc::c_int) -> libc::c_int {
    -1
}
#[cfg(not(target_os = "linux"))]
fn ino_drain(_fd: libc::c_int) {}

// ---------------- main ----------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--ppm" {
        host_ppm(args.get(2).map(|s| s.as_str()).unwrap_or("/tmp/aginx-term.ppm"));
        return;
    }

    let font = font::font_init();
    let mut d = match Drm::wait_up() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("aginx-term: {e}");
            std::process::exit(1);
        }
    };
    let (w, h) = (d.width as usize, d.height as usize);
    let pitch = d.pitch_px();

    let mut kb = Kb::new();
    let mut ime = pinyin::Ime::new(); // M40: 拼 buffer + candidate page
    let kg = Kb::geom(w, h);
    let mut entries = launch::entries();
    // Picker state ("+" tile): optional packages from `aginx-pkg available`
    // and the last install result line.
    let mut pkgs: Vec<String> = Vec::new();
    let mut pk_status = String::new();
    let lg = launch::Geom::new(w, h, kg.extra_y, entries.len());

    // Terminal geometry: glyph scale is per-app — sh keeps 5 (30x40 px
    // cells, 34 cols inside the 28 px side margins), the PC-designed TUIs
    // (codex/grok) get 3 (18x24 px, ~56 cols) so their 80-col layouts fit.
    let mut scale = 5usize;
    let area_top = lg.toolbar_h + 20;
    // Keyboard starts hidden; a tap in the terminal area summons/dismisses
    // it and the terminal rows grow/shrink to match (child gets SIGWINCH).
    let area_bottom = |vis: bool| if vis { kg.extra_y } else { h - 24 };
    let rows_for = |vis: bool, sc: usize| ((area_bottom(vis) - area_top) / (8 * sc + ROW_GAP)).max(4);
    let cols_for = |sc: usize| ((w - 2 * kb::KB_M) / (6 * sc)).max(20);
    let mut term_cols = cols_for(scale);
    let mut kb_visible = false;

    let mut term = Term::new(term_cols, rows_for(kb_visible, scale));
    let mut parser = vte::Parser::new();
    let mut mode = Mode::Launcher;
    // M42a: voice dialog face view (polled from /run/aginx-voice/face)
    let mut voice = VoiceView::default();
    // Debug/headless path: AGINX_TERM_START=<bin> skips the launcher and spawns
    // the program immediately (e.g. AGINX_TERM_START=/bin/sh).
    if let Ok(prog) = std::env::var("AGINX_TERM_START") {
        // leak: aginx-term is a forever-process
        let prog: &'static str = Box::leak(prog.into_boxed_str());
        scale = launch::scale_for(prog);
        term_cols = cols_for(scale);
        term = Term::new(term_cols, rows_for(kb_visible, scale));
        match spawn_shell(term_cols as u16, rows_for(kb_visible, scale) as u16, &[prog]) {
            Ok(c) => mode = Mode::Running(c),
            Err(e) => eprintln!("aginx-term: AGINX_TERM_START spawn: {e}"),
        }
    } else if !std::path::Path::new("/etc/wifi.conf").exists()
        && std::path::Path::new(launch::BIN_WIZARD).is_file()
    {
        // First boot / wiped userdata: no network credentials yet, so the
        // wizard is the setup UI (SYSTEM.md §9.2) instead of the launcher.
        scale = launch::scale_for(launch::BIN_WIZARD);
        term_cols = cols_for(scale);
        term = Term::new(term_cols, rows_for(kb_visible, scale));
        match spawn_shell(
            term_cols as u16,
            rows_for(kb_visible, scale) as u16,
            &[launch::BIN_WIZARD],
        ) {
            Ok(c) => mode = Mode::Running(c),
            Err(e) => eprintln!("aginx-term: wizard spawn: {e}"),
        }
    }
    let mut touch = TouchReader::open("/dev/input/event2", w as i32, h as i32);
    // M15: qpnp_pon keys (power + volume-down) on event1 — hardcoded like
    // the touch node, per HARDWARE.md.
    let mut pwr = KeyReader::open("/dev/input/event1");
    // M15 blank state
    let mut blanked = false;
    let mut last_input = Instant::now();
    let mut power_down: Option<Instant> = None;

    // Persistent canvas: renderers repaint only damaged rows into it, and
    // each present() memcpy's it into the back buffer (~10 MB, ~1 ms) so
    // double-buffer semantics survive partial redraws.
    let mut canvas = vec![0u32; pitch * h];
    // First frame BEFORE the mode set (panel snapshots at SETCRTC).
    {
        let r = Render { font: &font, w, h, pitch };
        let buf = &mut canvas[..];
        match &mode {
            Mode::Launcher => r.launcher(buf, &entries, &lg),
            Mode::Picker => r.picker(buf, &pkgs, &pk_status, &lg),
            Mode::Photos(p) => {
                if p.view {
                    r.photo_view(buf, p, &lg);
                } else {
                    r.photos_list(buf, p, &lg);
                }
            }
            Mode::Voice => r.voice(buf, &voice, &lg),
            Mode::Running(_) => {
                fill_rect(buf, pitch, w, h, 0, 0, w as i32, h as i32, BG);
                r.toolbar(buf, lg.m, lg.toolbar_h);
                r.terminal(buf, &term, area_top, area_bottom(kb_visible) - area_top, scale, true, lg.m);
            }
        }
        if kb_visible {
            r.keyboard(buf, &kg, &kb);
            if kb.pinyin_on() {
                r.ime_strip(buf, &ime, &kg);
            }
        }
        d.back_buf().copy_from_slice(&canvas);
    }
    if let Err(e) = d.initial_modeset() {
        eprintln!("aginx-term: modeset: {e}");
        std::process::exit(1);
    }

    let mut last_blink = Instant::now();
    let mut blink_on = false;
    let mut kb_dirty = true;
    // Hold-to-repeat (DEL / arrows), Termux-style: the event + next fire
    // deadline. Repeats go through inject() like every other input.
    let mut held: Option<(InputEvent, Instant)> = None;
    let mut down_y = 0usize; // where the current touch started
    // M17 debug/voice hook: AGINX_TERM_INJECT=1 watches /run/aginx-term.inject —
    // any process can drop text there and it types into the running
    // session as TextInputEvent, verbatim (\r included if written). This
    // is the exact path M18's ASR callback takes, testable without audio.
    let inject_file = std::env::var("AGINX_TERM_INJECT").ok().as_deref() == Some("1");
    // M47⑤f frame-arrival watch (armed lazily — the directory may not
    // exist yet when term starts at boot).
    let (ino_fd, mut ino_wd) = ino_init();

    loop {
        // drain pty output
        let mut redraw = false;
        if let Mode::Running(child) = &mut mode {
            let mut buf = [0u8; 8192];
            loop {
                match std::io::Read::read(&mut child.master, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        for &b in &buf[..n] {
                            parser.advance(&mut term, b);
                        }
                        term.jump_live(); // new output jumps to live
                        redraw = true;
                        // active output keeps the screen awake
                        last_input = Instant::now();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
            if child_exited(child.pid) {
                mode = Mode::Launcher;
                entries = launch::entries();
                kb_visible = false;
                scale = 5;
                term_cols = cols_for(scale);
                term = Term::new(term_cols, rows_for(false, scale));
                parser = vte::Parser::new();
                redraw = true;
            }
        }

        // input (touch / power key / pty)
        let mut fds = [libc::pollfd { fd: -1, events: libc::POLLIN, revents: 0 }; 4];
        let mut nfds = 0usize;
        if ino_fd >= 0 && ino_wd < 0 {
            ino_wd = ino_rearm(ino_fd);
        }
        if let Some(t) = touch.as_ref() {
            fds[nfds].fd = t.raw_fd();
            nfds += 1;
        }
        if let Some(p) = pwr.as_ref() {
            fds[nfds].fd = p.raw_fd();
            nfds += 1;
        }
        if let Mode::Running(c) = &mode {
            fds[nfds].fd = c.master.as_raw_fd();
            nfds += 1;
        }
        // M47⑤f: the frame-arrival watch rides the poll set while the
        // voice view is on screen — every eye.raw / eye.jpg / face publish
        // then wakes the loop the instant it lands.
        if ino_wd >= 0 && matches!(mode, Mode::Voice) {
            fds[nfds].fd = ino_fd;
            nfds += 1;
        }
        let timeout: libc::c_int = if redraw {
            0
        } else if held.is_some() || power_down.is_some() {
            30
        } else if matches!(mode, Mode::Voice) {
            // M47⑤b: the voice face polls files on this cadence — 400 ms
            // capped the viewfinder display at 2.5 fps even with cam-shot
            // publishing ~8 fps (user receipt 2026-09-05 「看起来很卡」).
            // M47⑤f: with the frame-arrival watch armed, IN_MOVED_TO wakes
            // the loop the instant a frame or face write lands — the timer
            // is only a 200 ms safety net. Unarmed (directory absent), the
            // stat cadence carries it: 12 ms while the eye is live, 30 ms
            // idle.
            if ino_wd >= 0 {
                200
            } else if voice.doc.eye { 12 } else { 30 }
        } else {
            400
        };
        let nready = unsafe { libc::poll(fds.as_mut_ptr(), nfds as libc::nfds_t, timeout) };
        if nready > 0 {
            let mut i = 0;
            if touch.is_some() {
                if fds[i].revents & libc::POLLIN != 0 {
                    let ev = touch.as_mut().unwrap().poll();
                    last_input = Instant::now();
                    if blanked {
                        // Any touch wakes the screen; the waking gesture
                        // itself is swallowed so it doesn't also type or
                        // scroll.
                        blanked = false;
                        d.dpms(true);
                        redraw = true;
                    } else {
                    match ev {
                        // Keys fire on finger-DOWN. Waiting for finger-up
                        // added the whole rest-of-finger time to every
                        // keystroke — the main source of "typing lag".
                        Touch::Down(x, y) => {
                            down_y = y;
                            if std::env::var("AGINX_TERM_DEBUG").is_ok() {
                                eprintln!("aginx-term: touch down {x},{y} kbvis={kb_visible} mode={}", matches!(mode, Mode::Running(_)));
                            }
                            if y < lg.toolbar_h {
                                // BACK fires on press, same as keys
                                if lg.toolbar_hit(x, y, matches!(mode, Mode::Running(_) | Mode::Picker | Mode::Photos(_) | Mode::Voice))
                                    == Some(launch::Toolbar::Back)
                                {
                                    if let Mode::Running(c) = &mode {
                                        unsafe { libc::kill(c.pid, libc::SIGHUP) };
                                    } else if matches!(mode, Mode::Picker) {
                                        mode = Mode::Launcher;
                                    } else if let Mode::Photos(p) = &mut mode {
                                        // view -> list -> launcher, one BACK each
                                        if p.view {
                                            p.view = false;
                                            p.img = None; // free the ~3 MB
                                        } else {
                                            mode = Mode::Launcher;
                                        }
                                    } else if matches!(mode, Mode::Voice) {
                                        mode = Mode::Launcher;
                                    }
                                    redraw = true;
                                }
                            } else if y < kg.extra_y {
                                // M40 candidate strip floats over this band
                                // while 拼 is on and a session runs: slot 0
                                // is the buffer (display only), 1-6 commit a
                                // hanzi, 7 pages the candidate list.
                                let strip_top = kg.extra_y.saturating_sub(IME_STRIP_H);
                                if kb_visible
                                    && kb.pinyin_on()
                                    && y >= strip_top
                                    && matches!(mode, Mode::Running(_))
                                {
                                    let slot = (x / (w / 8)).min(7);
                                    if slot == 7 {
                                        ime.next_page();
                                    } else if slot >= 1 {
                                        if let Some(ch) = ime.take_candidate(slot - 1) {
                                            inject(
                                                &mut mode,
                                                &mut term,
                                                &mut parser,
                                                &InputEvent::Text(ch.to_string()),
                                            );
                                        }
                                    }
                                    redraw = true;
                                } else if let Mode::Launcher = &mut mode {
                                    if let Some(i2) = lg.button_at(x, y, entries.len()) {
                                        if entries[i2].picker {
                                            pkgs = read_available();
                                            pk_status.clear();
                                            mode = Mode::Picker;
                                            redraw = true;
                                        } else if entries[i2].voice {
                                            // force the first face read (mtime
                                            // reset), then poll paints it; the
                                            // eye frame follows the same rule
                                            // (M42g) so an open viewfinder
                                            // paints its current frame at entry
                                            voice.mtime = None;
                                            voice.eye_mtime = None;
                                            voice.raw_mtime = None;
                                            mode = Mode::Voice;
                                            redraw = true;
                                        } else if entries[i2].photos {
                                            mode = Mode::Photos(photos::Photos::scan());
                                            redraw = true;
                                        } else if entries[i2].avail {
                                            let prog = entries[i2].bin.as_str();
                                            if prog == launch::BIN_AGINX_REBOOT {
                                                // these draw their own frame
                                                // and never come back — no
                                                // pty round-trip
                                                if entries[i2].args.first().map(String::as_str) == Some("poweroff") {
                                                    power_off(&mut d, &font, &mut canvas, blanked);
                                                }
                                                fill_rect(&mut canvas, pitch, w, h, 0, 0, w as i32, h as i32, BG);
                                                draw_centered(&mut canvas, pitch, w, h, &font, (h as i32 - 8 * 5) / 2, "RESTARTING", 5, GREEN);
                                                d.back_buf().copy_from_slice(&canvas);
                                                d.dpms(true); // relatch the frame (crtc may be off)
                                                let _ = std::process::Command::new(launch::BIN_AGINX_REBOOT)
                                                    .arg("reboot")
                                                    .spawn();
                                                std::process::exit(0);
                                            }
                                            // Registry entries carry their
                                            // own scale; PC-designed TUIs
                                            // need ~56 cols to breathe, the
                                            // phone-native UIs keep the big
                                            // touch glyphs.
                                            scale = entries[i2].scale;
                                            term_cols = cols_for(scale);
                                            let argv: Vec<&str> = std::iter::once(prog)
                                                .chain(entries[i2].args.iter().map(String::as_str))
                                                .collect();
                                            match spawn_shell(term_cols as u16, rows_for(false, scale) as u16, &argv) {
                                                Ok(c) => {
                                                    mode = Mode::Running(c);
                                                    kb_visible = false;
                                                    term = Term::new(term_cols, rows_for(false, scale));
                                                    parser = vte::Parser::new();
                                                    kb_dirty = true;
                                                    // wipe launcher pixels below the header —
                                                    // row-damage rendering only repaints
                                                    // terminal rows, so launcher art (the
                                                    // AGINXOS title top sliver) would linger
                                                    fill_rect(&mut canvas, pitch, w, h, 0, lg.toolbar_h as i32, w as i32, (h - lg.toolbar_h) as i32, BG);
                                                }
                                                Err(e) => eprintln!("aginx-term: spawn: {e}"),
                                            }
                                            redraw = true;
                                        }
                                    }
                                } else if let Mode::Picker = &mut mode {
                                    if let Some(i2) = lg.button_at(x, y, pkgs.len()) {
                                        if let Some(name) = pkgs.get(i2).cloned() {
                                            // synchronous install: paint the
                                            // frame first, the event loop is
                                            // about to block on aginx-download
                                            {
                                                let r = Render { font: &font, w, h, pitch };
                                                r.installing(&mut canvas[..], &name);
                                                d.back_buf().copy_from_slice(&canvas);
                                                d.present();
                                            }
                                            let out = std::process::Command::new(launch::BIN_AGINX_PKG)
                                                .arg("opt-in")
                                                .arg(&name)
                                                .output();
                                            pk_status = match out {
                                                Ok(o) if o.status.success() => format!("INSTALLED {name}"),
                                                Ok(_) => format!("FAILED {name}"),
                                                Err(e) => format!("FAILED {name}: {e}"),
                                            };
                                            // opt-in seeds /var/apps — the
                                            // registry may have grown; the
                                            // installed name leaves the list
                                            entries = launch::entries();
                                            pkgs = read_available();
                                            redraw = true;
                                        }
                                    }
                                } else if let Mode::Photos(p) = &mut mode {
                                    // decode box: full width, below the BACK strip
                                    let (mw, mh) = (w as u32, (h - lg.toolbar_h) as u32);
                                    let n = p.names().len();
                                    if p.view {
                                        // paint-first, then block in libjpeg
                                        // (the INSTALLING pattern)
                                        {
                                            let r = Render { font: &font, w, h, pitch };
                                            r.loading(&mut canvas[..]);
                                            d.back_buf().copy_from_slice(&canvas);
                                            d.present();
                                        }
                                        p.step(if x >= w / 2 { 1 } else { -1 }, mw, mh);
                                        redraw = true;
                                    } else if let Some(i2) = lg.button_at(x, y, n.min(12)) {
                                        {
                                            let r = Render { font: &font, w, h, pitch };
                                            r.loading(&mut canvas[..]);
                                            d.back_buf().copy_from_slice(&canvas);
                                            d.present();
                                        }
                                        p.open(i2, mw, mh);
                                        redraw = true;
                                    }
                                }
                            }
                            if kb_visible && y >= kg.extra_y {
                                let py_was = kb.pinyin_on(); // before 拼 may flip
                                let ev = if y >= kg.panel_y {
                                    kb.key_at(&kg, x, y)
                                } else {
                                    kb.extra_key_at(&kg, x, y)
                                };
                                if let Some(ev) = ev {
                                    // M40: while 拼 is on, the IME sees the
                                    // event first — letters build the buffer,
                                    // space/enter commit. Pass flows through
                                    // to inject() unchanged.
                                    match if py_was && matches!(mode, Mode::Running(_)) {
                                        ime.feed(&ev)
                                    } else {
                                        pinyin::Outcome::Pass
                                    } {
                                        pinyin::Outcome::Commit(s) => {
                                            inject(&mut mode, &mut term, &mut parser, &InputEvent::Text(s));
                                        }
                                        pinyin::Outcome::Consumed => {}
                                        pinyin::Outcome::Pass => {
                                            inject(&mut mode, &mut term, &mut parser, &ev);
                                            if input::repeatable(&ev) {
                                                held = Some((ev, Instant::now() + Duration::from_millis(400)));
                                            }
                                        }
                                    }
                                    // 拼 flipped this tap: drop the buffer and
                                    // repaint the terminal rows the strip was
                                    // floating over (row-damage alone would
                                    // leave stale strip pixels)
                                    if py_was != kb.pinyin_on() {
                                        ime.clear();
                                        for row in 0..term.rows {
                                            term.mark_row(row);
                                        }
                                    }
                                    // (modifier highlight / repaint handled
                                    // by the touch-feedback lines below)
                                }
                                // touch feedback: this keycap lights until lift
                                kb.press_locate(&kg, x, y);
                                kb_dirty = true;
                                redraw = true;
                            }
                        }
                        // Finger lifted: everything fired at Down already.
                        // A tap in the terminal area (no drag) summons or
                        // dismisses the keyboard; rows resize + SIGWINCH.
                        Touch::Tap(_x, y) => {
                            held = None;
                            // lift without a drag — the normal end of a key
                            // tap: drop the touch-feedback highlight
                            if kb.clear_pressed_if_any() {
                                kb_dirty = true;
                                redraw = true;
                            }
                            if std::env::var("AGINX_TERM_DEBUG").is_ok() {
                                eprintln!("aginx-term: touch tap y={y} kbvis={kb_visible}");
                            }
                            let kb_bot = if kb_visible { kg.extra_y } else { h };
                            // a lift over the candidate strip is an IME tap
                            // (already handled at Down) — it must not also
                            // toggle the keyboard away
                            let in_strip = kb_visible
                                && kb.pinyin_on()
                                && matches!(mode, Mode::Running(_))
                                && y >= kg.extra_y.saturating_sub(IME_STRIP_H);
                            if let Mode::Running(c) = &mode {
                                if y >= lg.toolbar_h && y < kb_bot && !in_strip {
                                    kb_visible = !kb_visible;
                                    let nr = rows_for(kb_visible, scale);
                                    term.resize_rows(nr);
                                    let ws = libc::winsize {
                                        ws_row: nr as u16,
                                        ws_col: term_cols as u16,
                                        ws_xpixel: 0,
                                        ws_ypixel: 0,
                                    };
                                    unsafe {
                                        libc::ioctl(c.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws);
                                    }
                                    // layout changed — wipe everything below
                                    // the header and repaint from scratch
                                    fill_rect(&mut canvas, pitch, w, h, 0, lg.toolbar_h as i32, w as i32, (h - lg.toolbar_h) as i32, BG);
                                    kb_dirty = true;
                                    redraw = true;
                                }
                            }
                        }
                        // Scrollback drag only counts if the touch STARTED
                        // in the terminal area (dragging across keys types
                        // nothing and scrolls nothing).
                        Touch::Up => {
                            held = None;
                            // lift after a drag that started on a key:
                            // drop the touch-feedback highlight
                            if kb.clear_pressed_if_any() {
                                kb_dirty = true;
                                redraw = true;
                            }
                        }
                        Touch::Drag(dy) => {
                            held = None; // finger slid off the key
                            let kb_bot = if kb_visible { kg.extra_y } else { h };
                            if down_y < kb_bot {
                                if let Mode::Running(_) = mode {
                                    let lines = dy / (8 * scale) as isize;
                                    if lines != 0 {
                                        term.scroll_view(lines);
                                        redraw = true;
                                    }
                                }
                            }
                        }
                        Touch::None => {}
                    }
                    }
                }
                i += 1;
            }
            if pwr.is_some() {
                if fds[i].revents & libc::POLLIN != 0 {
                    for (code, down) in pwr.as_mut().unwrap().poll() {
                        last_input = Instant::now();
                        if code != KEY_POWER {
                            continue; // volume-down rides the same node
                        }
                        if down {
                            power_down = Some(Instant::now());
                        } else if let Some(t) = power_down.take() {
                            // short press toggles blank; a long press was
                            // already acted on by the hold check below
                            if t.elapsed() < POWER_HOLD {
                                if blanked {
                                    blanked = false;
                                    d.dpms(true);
                                    redraw = true;
                                } else {
                                    blanked = true;
                                    d.dpms(false);
                                }
                            }
                        }
                    }
                }
                i += 1;
            }
            if let Mode::Running(_) = mode {
                if i < nfds && fds[i].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                    // pty readable — next loop iteration drains it
                    redraw = true;
                }
            }
            // M47⑤f: frame-arrival wake — just drain; the voice poll below
            // stats and renders if anything actually changed.
            if ino_wd >= 0 && matches!(mode, Mode::Voice) {
                let ij = i + if matches!(mode, Mode::Running(_)) { 1 } else { 0 };
                if ij < nfds && fds[ij].revents & libc::POLLIN != 0 {
                    ino_drain(ino_fd);
                }
            }
        }

        // power key held >= POWER_HOLD: shutdown (fires while still down)
        if let Some(t) = power_down {
            if t.elapsed() >= POWER_HOLD {
                power_off(&mut d, &font, &mut canvas, blanked);
            }
        }
        // idle blank
        if !blanked && last_input.elapsed() >= IDLE_BLANK {
            blanked = true;
            d.dpms(false);
        }

        // hold-to-repeat for DEL / arrows
        if let Some((ev, next)) = &mut held {
            if Instant::now() >= *next {
                inject(&mut mode, &mut term, &mut parser, ev);
                *next = Instant::now() + Duration::from_millis(60);
                redraw = true;
            }
        }

        // voice-path hook: file content types into the session, consumed
        if inject_file {
            if let Ok(s) = std::fs::read_to_string("/run/aginx-term.inject") {
                let _ = std::fs::remove_file("/run/aginx-term.inject");
                if !s.is_empty() {
                    last_input = Instant::now();
                    inject(&mut mode, &mut term, &mut parser, &InputEvent::Text(s));
                    redraw = true;
                }
            }
        }

        // M42a voice face: poll while the dialog is on screen. A live
        // dialog counts as activity — the screen must not blank mid-flow
        // (a face write arrives exactly when the user starts talking).
        // M42g: the viewfinder frame polls too — a frame landing ~1/s is
        // activity; decode box is the body area under the toolbar.
        if matches!(mode, Mode::Voice) {
            let face = voice.poll();
            let (_, _, eye_w, eye_h) = lg.eye_box();
            let eye = voice.poll_eye(eye_w as u32, eye_h as u32);
            if face || eye {
                last_input = Instant::now();
                if blanked {
                    blanked = false;
                    d.dpms(true);
                }
                redraw = true;
            }
        }

        // blink toggle — repaint only the cursor's row
        if last_blink.elapsed() > Duration::from_millis(500) {
            blink_on = !blink_on;
            last_blink = Instant::now();
            if matches!(mode, Mode::Running(_)) && term.view_offset == 0 {
                term.mark_row(term.cursor_y);
                redraw = true;
            }
        }

        // while blanked the framebuffer is not scanned out — skip render
        // and present entirely (pty keeps draining above, output renders
        // at wake)
        if !blanked && (redraw || term.dirty) {
            term.dirty = false;
            let r = Render { font: &font, w, h, pitch };
            let buf = &mut canvas[..];
            // M47⑤f: true when the eye frame went straight into the back
            // buffer — the canvas copy below is then skipped
            let mut direct = false;
            match &mode {
                Mode::Launcher => {
                    // launcher() full-covers the canvas
                    r.launcher(buf, &entries, &lg);
                }
                Mode::Picker => {
                    // picker() full-covers the canvas
                    r.picker(buf, &pkgs, &pk_status, &lg);
                }
                Mode::Photos(p) => {
                    // both photo screens full-cover the canvas
                    if p.view {
                        r.photo_view(buf, p, &lg);
                    } else {
                        r.photos_list(buf, p, &lg);
                    }
                }
                Mode::Voice => {
                    // M47⑤f: a fresh raw viewfinder frame blits fused
                    // (565→888 + upscale) straight into the back buffer — no
                    // Bitmap, no canvas detour, no 10 MB copy. Everything
                    // else (dialog / 取景中… / the JPEG fallback frame)
                    // renders into the canvas as before.
                    direct = voice.doc.eye
                        && voice.blit_eye_raw(d.back_buf(), pitch, w, h);
                    if !direct {
                        // voice() full-covers the canvas
                        r.voice(buf, &voice, &lg);
                    }
                }
                Mode::Running(_) => {
                    r.terminal(buf, &term, area_top, area_bottom(kb_visible) - area_top, scale, blink_on, lg.m);
                    if kb_dirty {
                        r.toolbar(buf, lg.m, lg.toolbar_h);
                        if kb_visible {
                            r.keyboard(buf, &kg, &kb);
                        }
                    }
                    // every pass, not just kb_dirty: the strip floats over
                    // terminal rows that repaint on cursor blink
                    if kb_visible && kb.pinyin_on() {
                        r.ime_strip(buf, &ime, &kg);
                    }
                }
            }
            term.clear_row_dirty();
            kb_dirty = false;
            if !direct {
                d.back_buf().copy_from_slice(&canvas);
            }
            let t0 = Instant::now();
            d.present();
            let el = t0.elapsed();
            if el > Duration::from_millis(25) {
                eprintln!("aginx-term: slow present {}ms", el.as_millis());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // M47⑤l: the fused 565→888 + bilinear-upscale present path,
    // hand-computed. A 2×2 source of primaries upscaled 1:2 into a 4×4 dst
    // (pitch 5 — wider than dw, so stride handling is exercised).
    // Source-center phase ((i+0.5)/2 − 0.5) gives taps −0.25/0.25/0.75/1.25:
    // edge dst pixels land exactly on their source pixel, the two interior
    // phases blend 1:3. Interior Q6 corner weights at 0.25/0.25: 36/12/12/4.
    #[test]
    fn upscale565_primaries() {
        let px = |r: u16, g: u16, b: u16| ((r << 11) | (g << 5) | b).to_le_bytes();
        let mut src = Vec::new();
        src.extend_from_slice(&0x31574752u32.to_le_bytes()); // "RGW1" magic
        src.extend_from_slice(&2u32.to_le_bytes()); // w
        src.extend_from_slice(&2u32.to_le_bytes()); // h (12-byte header skipped)
        for p in [px(31, 0, 0), px(0, 63, 0), px(0, 0, 31), px(31, 63, 31)] {
            src.extend_from_slice(&p);
        }
        let mut pix = [0xDEADBEEFu32; 5 * 4];
        upscale565(&mut pix, 5, 4, 4, &src, 2, 2);
        let at = |x: usize, y: usize| pix[y * 5 + x];
        assert_eq!(at(0, 0), 0xFF0000, "red (edge tap = exact source pixel)");
        assert_eq!(at(0, 3), 0x0000FF, "blue (row edge tap)");
        assert_eq!(at(3, 3), 0xFFFFFF, "white (corner tap)");
        // (1,1): quarter-phase both axes — Q6 weights 36/12/12/4:
        // r=255·40+32>>6=159, g=b=255·16+32>>6=64
        assert_eq!(at(1, 1), 0x009F4040, "interior red+green+blue+white blend");
        // (3,1): col taps clamp onto src col 1; rows blend green→white 3:1
        assert_eq!(at(3, 1), 0x0040FF40, "vertical green→white 1:3 blend");
        // 1-D pin: 2×1 red|blue → 4×1 — exact / 1:3 / 3:1 / exact
        let mut src2 = Vec::new();
        src2.extend_from_slice(&0x31574752u32.to_le_bytes());
        src2.extend_from_slice(&2u32.to_le_bytes()); // w
        src2.extend_from_slice(&1u32.to_le_bytes()); // h
        src2.extend_from_slice(&px(31, 0, 0));
        src2.extend_from_slice(&px(0, 0, 31));
        let mut pix2 = [0u32; 4];
        upscale565(&mut pix2, 4, 4, 1, &src2, 2, 1);
        assert_eq!(
            pix2,
            [0xFF0000, 0x00BF0040, 0x004000BF, 0x0000FF],
            "1-D bilinear phases"
        );
        // mid green g6=32: (32<<2)|(32>>4) = 130
        let mut src3 = Vec::new();
        src3.extend_from_slice(&[0u8; 12]);
        src3.extend_from_slice(&px(0, 32, 0));
        let mut pix3 = [0u32; 1];
        upscale565(&mut pix3, 1, 1, 1, &src3, 1, 1);
        assert_eq!(pix3[0], 0x008200, "g6=32 replicates to 130");
        // the LUT path and the formula agree at the corners
        let lut = lut565();
        assert_eq!(lut[0xF800], 0xFF0000, "lut red");
        assert_eq!(lut[0x07E0], 0x00FF00, "lut green");
        assert_eq!(lut[0x001F], 0x0000FF, "lut blue");
    }
}
