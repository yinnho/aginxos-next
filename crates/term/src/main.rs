// aginx-term — AginxOS on-device terminal (M11 aterm; N4③b 改姓).
//
// bootcard's DRM path + 5x8 font, a vte-parsed cell grid (black bg, green /
// white text — the fixed phosphor palette), an openpty child (sh; debug
// sessions via AGINX_TERM_START), an evdev on-screen keyboard (tap = key,
// drag = scrollback), and the 面法 faces: Idle 待命面 (breathing cursor /
// transcript / result page), Eye 取景, Install 软件清单 (C7). Started by
// rcS's aginx-term-handoff once boot finishes; bootcard is wordmark-only
// now and self-exits (#246), so the handoff's kill is belt-and-braces.
// 批③ (09-10): launcher/picker/photos faces demolished — the install list
// face (pair bar → 软件清单) is the egg's install entry; the M39 photo
// viewer retired with them (photos stay files in /home/photos).
//
// M15 power management: the power key (node from [input.term]) blanks the
// panel (connector DPMS off — the same path that darkened the screen when a
// DRM master dropped), a second short press or any touch wakes it, 60 s idle
// blanks too, holding the key ~1.2 s runs `aginx-reboot poweroff`.
//
// M17 input split: the keyboard hit tests return typed InputEvents
// (KeyEvent vs TextInputEvent, input.rs) and EVERY write to the pty goes
// through inject() — the same entry point voice input uses with
// recognized text.
//
// Host verification: `aginx-term --ppm out.ppm` renders the console faces
// into P6 PPMs without touching DRM (same pattern as bootcard --ppm).

mod browser; // v4⑥ 活体结果面 CDP 面板客户端（接线于 main loop）
mod cjk;
mod drm;
mod font;
mod input;
mod kb;
mod launch;
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

// 面法待机面 (09-07): near-black with a faint green cast; v4⑤ the idle
// cursor breathes at the top anchor (same line the transcript starts on),
// typed text anchors there too.
const IDLE_BG: u32 = 0x00020503; // near-black, faint green cast
const MGREEN: u32 = 0x0000FF41; // Matrix green — typewriter lines / cursor

// 开机剧情 v4⑤ prompt face: glyph scale 5 (30×40 px cells). Cursor and
// typed text share the top-left anchor (x=w/12, y=h*8/100 — clears the
// front camera punch-hole); idle = bare breathing cursor on that line.
const PROMPT_CS: usize = 5;

// 警告注册表 (2026-09-09): /run/aginx-warn/ — one warning = one file
// (filename = source tag, content = one-line CJK message). v0 sole
// writer is net-watch; future daemons (battery/storage/thermal…) drop a
// file in without a term change (the D12 registry shape). Non-empty →
// the idle face paints a red center zone; empty dir = normal face.
const WARN_DIR: &str = "/run/aginx-warn";
const WARN_RED: u32 = 0x00FF3B30;
const WARN_WM_SCALE: usize = 13; // bootcard wordmark scale — same geometry
const WARN_LINE_SCALE: usize = 5; // warning lines below the wordmark (= transcript cell)
const WARN_MAX: usize = 4; // lines cap — the face is a glance, not a log

/// 呼吸光标 (v4⑤): level 0..=16 → 35%..100% of MGREEN per channel.
/// L0=0x00005917, L8=0x0000AB2C, L16=MGREEN — the golden tests pin these.
fn breath_shade(level: u8) -> u32 {
    let pct = 35 + 65 * level as u32 / 16;
    let ch = |c: u32| (c * pct + 50) / 100;
    (ch((MGREEN >> 16) & 0xFF) << 16) | (ch((MGREEN >> 8) & 0xFF) << 8) | ch(MGREEN & 0xFF)
}

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
// pattern. When face.eye is set the viewfinder takes the whole panel
// (M47⑤b) — the close keys are physical, not on-screen.
const VOICE_EYE: &str = "/run/aginx-voice/eye.jpg";
// M47⑤c raw fast path: cam-shot --raw-out publishes RGB565 every frame;
// term blits it with no JPEG decode (the encode+decode round trip stays
// only for QR, which reads eye.jpg at 2 Hz). Preferred when present.
const VOICE_EYE_RAW: &str = "/run/aginx-voice/eye.raw";
// v4⑥: the live result page source — voice publishes the full phosphor
// HTML here (atomic tmp+rename) before flipping face.result; term attaches
// it to the engine as a data: URL (browser.rs) and the panel shows the
// LIVE page (scrollable). The v4④ result.img PNG fallback is retired
// (v4⑥S5); ①a replaces the do-nothing gap: a missing/empty file now
// rebuilds the page from the session ledger (below) instead of silently
// keeping the prompt face.
const VOICE_RESULT_HTML: &str = "/run/aginx-voice/result.html";

// ---------------- ①a 账本恢复（结果页重建） ----------------
//
// 结果旗立着但 result.html 没了（voice 死在 rename 前、/run 被清、term
// 自己重启撞上文件丢失）——从会话账 fold 出最后一个 done(ok) 文本重建
// 降级页。稳态不变：result.html 文件接力仍是正路，这里只买崩溃恢复，
// 显示来自真源（D8 账）而不是又一个旁路文件。

/// 化身根：AGINX_HOME 覆写（试跑隔离），否则直钉平台默认 /home/.aginx
/// ——与 server unit 钉的 AGINX_HOME=/home/.aginx 同一处（M25 HOME=/home）。
/// 不走 HOME 推导：term 由 init.d 拉起、环境里 HOME=/（设备实测），按
/// HOME 解析会落 /.aginx 的空处。
fn workspaces_root() -> std::path::PathBuf {
    if let Ok(h) = std::env::var("AGINX_HOME") {
        return std::path::PathBuf::from(h).join("workspaces");
    }
    std::path::PathBuf::from("/home/.aginx/workspaces")
}

/// 账尾最后一个非空 done(ok) 文本。err/空文本 done 不算结果——旗只在
/// 成功轮收口后立起。坏行跳过（账可能截在半行上）。
fn fold_last_done_ok(log: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(log).ok()?;
    let mut last: Option<String> = None;
    for line in content.lines() {
        if let Ok(agi::Frame::Done(d)) = serde_json::from_str(line.trim()) {
            if d.ok && !d.text.trim().is_empty() {
                last = Some(d.text);
            }
        }
    }
    last
}

/// 降级壳：三钉（黑底 / min-height 满屏高 px / 视口=面板宽——引擎收据，
/// voice render.rs render_html 同款）+ 磷光可读性地板。panel 尺寸是参数
/// （D14）：产品走 hwd [panel]，host 测试喂 fixture。**不做 markdown 化**
/// ——排版归 aginxbrowser（①b 起 term 递原文给引擎 /render），恢复页
/// 就是原文。
fn degraded_shell(md: &str, pw: u32, ph: u32) -> String {
    let mut esc = String::with_capacity(md.len());
    for c in md.chars() {
        match c {
            '&' => esc.push_str("&amp;"),
            '<' => esc.push_str("&lt;"),
            '>' => esc.push_str("&gt;"),
            other => esc.push(other),
        }
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width={pw}\">\
<style>body{{background:#000;min-height:{ph}px}}\
pre{{white-space:pre-wrap;color:#8cffb0;font-size:36px;\
line-height:1.75;padding:24px;margin:0}}</style></head>\
<body><pre>{}</pre></body></html>",
        esc
    )
}

/// 从账重建结果页 HTML。等值护栏：fold 出的文本必须与 face.line 一致
/// ——母体直答不走账（v0 无账），对不上时账尾是别的化身（或旧轮）的
/// 结果，投上去就是张冠李戴；对不上就放弃恢复（文本面兜底=一等降级）。
/// 多化身按账 mtime 从新到旧依次试——即便命中旧账，展示的字节也与
/// line 全同，最坏只是出处歧义，没有内容错。
fn recover_result_html(
    root: &std::path::Path,
    line: Option<&str>,
    pw: u32,
    ph: u32,
) -> Option<String> {
    let want = line?.trim();
    if want.is_empty() {
        return None;
    }
    // #283 问句常驻：face.line = 「问句\n回复」——账上的 done 文本只是
    // 换行后的回复段。两种形状都认（问句自身含换行的罕见三段形状会取到
    // 半截、等值失配→放弃恢复，安全向——文本面兜底照旧一等）。
    let want_reply = want.split_once('\n').map(|(_, r)| r.trim());
    let mut cands: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let log = e.path().join("sessions").join("main.jsonl");
            let mtime = std::fs::metadata(&log).ok()?.modified().ok()?;
            Some((mtime, log))
        })
        .collect();
    cands.sort();
    cands.reverse();
    for (_, log) in cands {
        if let Some(text) = fold_last_done_ok(&log) {
            let t = text.trim();
            if t == want || want_reply.is_some_and(|w| t == w) {
                return Some(degraded_shell(&text, pw, ph));
            }
        }
    }
    None
}

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

// ---------------- pty ----------------

struct Child {
    master: std::fs::File,
    pid: libc::pid_t,
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
    Running(Child),
    /// 待命面 (开机剧情 v4, 面法 09-07 终稿): the resting screen — pure
    /// Matrix-cast near-black + the blinking block cursor at the prompt
    /// origin. No wordmark, no targets, no theater: the console only says
    /// what was actually said. Boot lands here; wake returns here.
    Idle,
    /// 眼视图 (面法 09-07, promoted from the M42g voice-face sub-state):
    /// fullscreen viewfinder. Entered from ANY mode when the aginx-voice
    /// face opens the eye (eye false→true), left when it closes — the
    /// prior mode is boxed away and restored. Pure display; close keys
    /// are physical (音量+ toggles, 音量下 closes).
    Eye,
    /// 软件清单面 (C7, 蛋的安装入口): manifest×stamps×bindir 行集 + 翻页 +
    /// 点击催装。job 槽在 main()（auto 触发与手动点击同一条单飞），面的
    /// 状态行读 install_line（idle 面同源）。
    Install(InstallView),
}

// ---------------- voice face ----------------

/// M42a: the JSON aginx-voice writes to /run/aginx-voice/face. 面法 09-07
/// → 开机剧情 v4: term reads the eye flag, the transcript line and the
/// result flag (voice may write extra fields; serde ignores them).
/// `alive` lives on VoiceView, not here — it means "the file read+parsed
/// at least once" (a vanished file = voice daemon death = eye treated as
/// closed).
#[derive(serde::Deserialize, Default)]
struct FaceDoc {
    #[serde(default)]
    /// M42g: viewfinder on — Mode::Eye takes the whole panel.
    eye: bool,
    /// 开机剧情 v4: the live result page is on the panel (term attached
    /// result.html to the engine on this flag's rising edge). Any new turn
    /// (PTT down / ASR landing) clears it — voice writes result=false.
    #[serde(default)]
    result: bool,
    /// The text typed onto the prompt face — the ASR transcript (v4③) or
    /// a text-fallback reply. '\n' forces a row break; term animates the
    /// typewriter reveal and wraps at the panel width.
    #[serde(default)]
    line: Option<String>,
}

#[derive(Default)]
struct VoiceView {
    doc: FaceDoc,
    mtime: Option<std::time::SystemTime>,
    alive: bool,
    /// M42g viewfinder frame cache. `poll_eye` gates on eye.jpg mtime; the
    /// decode itself blocks the event loop for a frame (DCT-scaled to the
    /// box, ~tens of ms; M47⑤ cam-shot
    /// runs resident at ~10fps, so the loop picks up every other frame).
    eye_mtime: Option<std::time::SystemTime>,
    /// M47⑤c: raw-frame mtime gate (the preferred source; eye_mtime/JPEG is
    /// the fallback when cam-shot runs without --raw-out).
    raw_mtime: Option<std::time::SystemTime>,
    /// M47⑤t: when THIS eye session opened. eye.raw/eye.jpg still hold the
    /// PREVIOUS session's last frame at eye-open, and the mtime gates below
    /// reset to None on eye-off — without this stamp the first poll mistakes
    /// any old mtime for a fresh frame and blits the stale frame (the brief
    /// "wrong scene" flash at eye-open, 2026-09-06). Only frames published
    /// after the open are ours.
    eye_open: Option<std::time::SystemTime>,
    eye_img: Option<aginx_img::Bitmap>,
    /// M47⑤f: raw frames no longer build a Bitmap at poll time — the render
    /// pass blits 565→888 fused with the upscale straight into the back
    /// buffer. `raw_dirty` marks a fresh frame; `raw_buf` is the reused
    /// 2.2 MB file buffer (clear + read_to_end keeps the capacity).
    raw_dirty: bool,
    raw_buf: Vec<u8>,
    /// 开机剧情 typewriter: chars of doc.line already revealed
    /// (across '\n'), plus the reveal baseline for the ~90 ms/char clock.
    /// The daemon swaps the text; an extension (a longer transcript/reply
    /// replacing a prefix) keeps the revealed prefix and restarts the
    /// clock from that baseline, anything else types from scratch.
    line_prog: usize,
    line_base: usize,
    line_seen: Option<String>,
    /// None until the first line lands; Some(t) = the clock started at t
    type_at: Option<Instant>,
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
    /// (one repaint so the prior face comes back clean); eye=true → stat
    /// the frame file and flag a change. Returns true when a repaint is
    /// due; the pixel work itself happens at render time (M47⑤f: the raw
    /// path blits fused straight into the back buffer — a 45 fps publish
    /// must not pay Bitmap-build + canvas detour per present).
    /// C6 `term_owns`: 蛋上 term 自持取景（voice 不在，doc.eye 恒 false）
    /// ——term 持有会话时同样走帧路；eye_open 戳记的是「本次会话」（两种
    /// 持有者共用，⑤t 旧帧护栏同律）。
    fn poll_eye(&mut self, max_w: u32, max_h: u32, term_owns: bool) -> bool {
        if !self.doc.eye && !term_owns {
            self.eye_mtime = None;
            self.raw_mtime = None;
            self.raw_dirty = false;
            if self.eye_open.take().is_some() {
                return true; // one repaint to leave the eye view cleanly
            }
            return self.eye_img.take().is_some();
        }
        if self.eye_open.is_none() {
            // Stamp the session open (see the field doc). Affinity is NOT
            // set here — it follows the eye FLAG from main(), in every
            // mode: the stream can outlive the voice view (the user backs
            // out while cam-shot keeps going).
            self.eye_open = Some(std::time::SystemTime::now());
        }
        let opened = self.eye_open;
        // M47⑤c: prefer the raw RGB565 frame (published every frame); the
        // JPEG is the fallback when cam-shot runs without --raw-out.
        // ⑤t: frames must ALSO postdate the eye-open stamp — the files
        // carry the previous session's tail at open.
        let mtime = std::fs::metadata(VOICE_EYE_RAW)
            .and_then(|m| m.modified())
            .ok();
        if let Some(t) = mtime {
            if Some(t) != self.raw_mtime && t > opened.unwrap_or(t) {
                self.raw_mtime = Some(t);
                self.raw_dirty = true;
                return true;
            }
            return false;
        }
        let mtime = std::fs::metadata(VOICE_EYE)
            .and_then(|m| m.modified())
            .ok();
        if mtime.is_none() || mtime == self.eye_mtime || mtime <= opened {
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
    /// runs the normal canvas render (取景中 / JPEG fallback).
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

    /// 开机剧情 v4: the transcript typewriter is mid-reveal — drives the
    /// loop's 90 ms poll cadence so the reveal animates smoothly instead
    /// of jumping 4-5 chars per idle tick.
    fn typing(&self) -> bool {
        match self.doc.line.as_ref() {
            Some(l) if !l.is_empty() => self.line_prog < l.chars().count(),
            _ => false,
        }
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
/// as a preview-width mosaic on the panel — the user saw 「像素超级低」).
/// All resampling happens AFTER the LUT expand: lerping 565
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
    /// 待命面 (v4⑤): pure near-black + a breathing block cursor. `line`
    /// (the transcript / text fallback) types left-aligned from the top
    /// anchor (below the front camera), wrapped at the panel width (~16
    /// CJK hanzi per row at scale 5); a solid cursor rides the reveal
    /// edge while typing, then breathes at the end of the last row. With
    /// nothing typed the cursor breathes at the top anchor. `breath` =
    /// cursor level 0..=16 (16 = full MGREEN); None = no cursor (demo
    /// beats that only want text).
    fn prompt(&self, pix: &mut [u32], line: &str, prog: usize, breath: Option<u8>) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, IDLE_BG);
        let cs = PROMPT_CS;
        let x0 = self.w / 12; // bootcard's left column
        let y0 = self.h * 8 / 100; // below the front camera punch-hole
        let avail = self.w - x0 - cs * 6; // right margin: one cell
        let chars: Vec<char> = line.chars().collect();
        let cw = |ch: char| if cjk::char_width(ch) == 2 { 12 * cs } else { 6 * cs };
        // wrap into rows at the panel width ('\n' forces a break)
        let mut rows: Vec<(usize, usize)> = Vec::new(); // (start char, len)
        let (mut start, mut run_w) = (0usize, 0usize);
        for (i, &ch) in chars.iter().enumerate() {
            if ch == '\n' {
                rows.push((start, i - start));
                start = i + 1;
                run_w = 0;
            } else {
                let w = cw(ch);
                if run_w + w > avail && i > start {
                    rows.push((start, i - start));
                    start = i;
                    run_w = w;
                } else {
                    run_w += w;
                }
            }
        }
        if start < chars.len() || rows.is_empty() {
            rows.push((start, chars.len() - start));
        }
        let total = chars.len();
        let shown_end = prog.min(total);
        let row_h = 8 * cs + 24;
        let mut cursor: Option<(i32, i32)> = None;
        for (ri, &(rs, rl)) in rows.iter().enumerate() {
            let y = (y0 + ri * row_h) as i32;
            if y + (8 * cs) as i32 > self.h as i32 - 24 {
                break; // panel bottom — rows beyond are dropped
            }
            let take = shown_end.saturating_sub(rs).min(rl);
            if take > 0 {
                let shown: String = chars[rs..rs + take].iter().collect();
                draw_text(pix, self.pitch, self.w, self.h, self.font, x0 as i32, y, &shown, cs, MGREEN);
            }
            if take < rl || (take == rl && shown_end < total) {
                // the reveal edge lands on/after this row — solid cursor
                let edge: usize = chars[rs..rs + take].iter().map(|&c| cw(c)).sum();
                cursor = Some((x0 as i32 + edge as i32, y));
                break;
            }
        }
        if shown_end == total {
            // typed out — the cursor breathes at the end of the last row;
            // empty line = the bare idle face, cursor breathes at the top
            // anchor (where the next transcript will start typing)
            let (cx, cy) = if total == 0 {
                (x0 as i32, y0 as i32)
            } else {
                let li = rows.len() - 1;
                let (lr, ll) = rows[li];
                let edge: usize = chars[lr..lr + ll].iter().map(|&c| cw(c)).sum();
                (x0 as i32 + edge as i32, (y0 + li * row_h) as i32)
            };
            if let Some(level) = breath {
                fill_rect(pix, self.pitch, self.w, self.h, cx, cy, (5 * cs) as i32, (8 * cs) as i32, breath_shade(level));
            }
        } else if let Some((cx, cy)) = cursor {
            fill_rect(pix, self.pitch, self.w, self.h, cx, cy, (5 * cs) as i32, (8 * cs) as i32, MGREEN);
        }
    }

    /// 警告注册表红警区 (2026-09-09): red AginxOS wordmark at bootcard's
    /// exact anchor (centered, y = h*45/100, scale 13) + one red line per
    /// warning file below it. Drawn AFTER prompt() — the top transcript
    /// and the breathing cursor are not touched.
    fn warn(&self, pix: &mut [u32], warns: &[String]) {
        let y = (self.h * 45 / 100) as i32;
        draw_centered(pix, self.pitch, self.w, self.h, self.font, y, "AginxOS", WARN_WM_SCALE, WARN_RED);
        let mut ly = self.h * 45 / 100 + 8 * WARN_WM_SCALE + 60;
        for msg in warns {
            draw_centered(pix, self.pitch, self.w, self.h, self.font, ly as i32, msg, WARN_LINE_SCALE, WARN_RED);
            ly += 8 * WARN_LINE_SCALE + 24;
        }
    }

    /// C6 底部双目标条（未配对蛋面的入口）：y∈[h-200,h-60] 高 140，左=
    /// 扫码配网（GREEN，本面）、右=软件清单（GREEN，C7 Mode::Install）。
    /// 命中几何 `pair_bar_hit` 用同一套数字（测试钉住）。
    fn pair_bar(&self, pix: &mut [u32]) {
        let (w, h) = (self.w, self.h);
        let y0 = (h - 200) as i32;
        let bh = 140i32;
        let bs = 4usize;
        let mut cell = |x0: i32, x1: i32, label: &str, c: u32| {
            let cw = x1 - x0;
            fill_rect(pix, self.pitch, w, h, x0, y0, cw, bh, KEYCAP);
            fill_rect(pix, self.pitch, w, h, x0, y0, cw, 2, DIM);
            fill_rect(pix, self.pitch, w, h, x0, y0 + bh - 2, cw, 2, DIM);
            fill_rect(pix, self.pitch, w, h, x0, y0, 2, bh, DIM);
            fill_rect(pix, self.pitch, w, h, x0 + cw - 2, y0, 2, bh, DIM);
            // 盒内居中（CJK 宽字符 12*cs、窄字符 6*cs——prompt 同律）
            let tw: usize =
                label.chars().map(|ch| if cjk::char_width(ch) == 2 { 12 * bs } else { 6 * bs }).sum();
            let tx = x0 + (cw - tw as i32) / 2;
            let ty = y0 + (bh - 8 * bs as i32) / 2;
            draw_text(pix, self.pitch, w, h, self.font, tx, ty, label, bs, c);
        };
        cell(60, (w / 2 - 30) as i32, "扫码配网", GREEN);
        cell((w / 2 + 30) as i32, (w - 60) as i32, "软件清单", GREEN);
    }

    /// C7 软件清单面：picker 同款行几何，行 = 名 + v版本 + 右对齐状态标
    /// （✓=就绪 DIM / 装·更新·选装=GREEN 可点）。翻页条只在多页时画
    /// （几何与 `install_page_hit` 同一套数字）。状态行优先级：job 进度
    /// 行 > 无网提示（有未完成行时）> 全部就绪 / 点击提示。
    fn install_list(
        &self,
        pix: &mut [u32],
        v: &InstallView,
        status: Option<&str>,
        net_ok: bool,
        g: &launch::Geom,
    ) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
        self.toolbar(pix, g.m, g.toolbar_h);
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.toolbar_h as i32 + 14, "软件清单", 5, GREEN);
        let pending = v.rows.iter().any(|r| r.state != RowState::Ready);
        if v.rows.is_empty() {
            draw_centered(pix, self.pitch, self.w, self.h, self.font, (self.h as i32 - 24) / 2, "(无清单)", 3, UNAVAIL);
        }
        let start = v.page * INSTALL_ROWS_PAGE;
        for (i, row) in v.rows.iter().skip(start).take(INSTALL_ROWS_PAGE).enumerate() {
            let y0 = (g.by0 + i * (g.bh + g.gap)) as i32;
            fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0, g.bw as i32, 3, DIM);
            fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0 + g.bh as i32 - 3, g.bw as i32, 3, DIM);
            fill_rect(pix, self.pitch, self.w, self.h, g.bx as i32, y0, 3, g.bh as i32, DIM);
            fill_rect(pix, self.pitch, self.w, self.h, (g.bx + g.bw - 3) as i32, y0, 3, g.bh as i32, DIM);
            let ty = y0 + (g.bh as i32 - 8 * 5) / 2;
            // 名（截尾防溢出）+ v版本（DIM）+ 右对齐状态标
            let name: String = row.name.chars().take(16).collect();
            draw_text(pix, self.pitch, self.w, self.h, self.font, g.bx as i32 + 30, ty, &name, 5, GREEN);
            if let Some(ver) = &row.version {
                let vs = format!("v{ver}");
                let vw = text_w(&vs, 3) as i32;
                let nw = text_w(&name, 5) as i32;
                // 挤到状态标就丢版本（名是真源，版本是装饰）
                let vx = g.bx as i32 + 40 + nw;
                if vx + vw < (g.bx + g.bw - 160) as i32 {
                    draw_text(pix, self.pitch, self.w, self.h, self.font, vx, ty + 16, &vs, 3, DIM);
                }
            }
            let (tag, tc) = match row.state {
                RowState::Ready => ("✓", DIM),
                RowState::Stale => ("更新", GREEN),
                RowState::Missing => ("装", GREEN),
                RowState::OptReady => ("选装", GREEN),
            };
            let tw = text_w(tag, 4) as i32;
            draw_text(pix, self.pitch, self.w, self.h, self.font, (g.bx + g.bw - 40) as i32 - tw, ty + 8, tag, 4, tc);
        }
        // 翻页条（多页才有）：<格 | 页码 | >格——几何 install_page_hit 同数
        let pages = v.pages();
        if pages > 1 {
            let y0 = g.kb_panel_y.saturating_sub(150) as i32;
            let mut cell = |x0: i32, x1: i32, label: &str, c: u32| {
                fill_rect(pix, self.pitch, self.w, self.h, x0, y0, x1 - x0, 80, KEYCAP);
                fill_rect(pix, self.pitch, self.w, self.h, x0, y0, x1 - x0, 2, DIM);
                fill_rect(pix, self.pitch, self.w, self.h, x0, y0 + 78, x1 - x0, 2, DIM);
                let tw = text_w(label, 4) as i32;
                draw_text(pix, self.pitch, self.w, self.h, self.font, x0 + ((x1 - x0) - tw) / 2, y0 + (80 - 8 * 4) / 2, label, 4, c);
            };
            let pc = if v.page == 0 { DIM } else { GREEN };
            let nl = if v.page + 1 >= pages { DIM } else { GREEN };
            cell(60, 300, if v.page == 0 { "·" } else { "<" }, pc);
            cell((self.w - 300) as i32, (self.w - 60) as i32, if v.page + 1 >= pages { "·" } else { ">" }, nl);
            let mid = format!("{} / {}", v.page + 1, pages);
            let mw = text_w(&mid, 3) as i32;
            draw_text(pix, self.pitch, self.w, self.h, self.font, (self.w as i32 - mw) / 2, y0 + (80 - 8 * 3) / 2, &mid, 3, pc);
        }
        // 状态行（picker 脚注位）
        let (line, lc) = match status {
            Some(s) => (s.to_string(), GREEN),
            None if !net_ok && pending => ("无网络 · 连网后自动安装".to_string(), UNAVAIL),
            None if !pending => ("全部就绪".to_string(), GREEN),
            _ => ("点击行催装".to_string(), UNAVAIL),
        };
        draw_centered(pix, self.pitch, self.w, self.h, self.font, g.kb_panel_y as i32 - 40, &line, 3, lc);
    }

    /// 眼视图 (面法 09-07, was the M42g eye branch of the voice face):
    /// fullscreen viewfinder — eye box = whole panel, JPEG frame by
    /// nearest-neighbor aspect-fill. The raw RGB565 fast path blits fused
    /// straight into the back buffer at render dispatch and never reaches
    /// here; this is the JPEG fallback plus the first-frame placeholder.
    fn eye(&self, pix: &mut [u32], v: &VoiceView, g: &launch::Geom) {
        fill_rect(pix, self.pitch, self.w, self.h, 0, 0, self.w as i32, self.h as i32, BG);
        if let Some(b) = &v.eye_img {
            let (_, _, bw, bh) = g.eye_box();
            if bh > 0 && b.w > 0 && b.h > 0 {
                // aspect-FILL by nearest-neighbor upscale (decode_scaled
                // only downscales; preview→panel upscaling lives here).
                // The frame's --aspect already matches the box.
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
    fn terminal(&self, pix: &mut [u32], t: &Term, area_top: usize, scale: usize, blink_on: bool, x_off: usize) {
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

/// Poll the warning registry (/run/aginx-warn/): entries sorted by
/// filename (= source tag), first line of each file, capped at WARN_MAX.
/// Missing dir / unreadable entries read as "no warnings" — a red face
/// must never come from a reader glitch.
fn read_warnings() -> Vec<String> {
    read_warnings_dir(WARN_DIR)
}

fn read_warnings_dir(dir: &str) -> Vec<String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.lines().next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty())
        .take(WARN_MAX)
        .collect()
}

// ---------------- C5: 自持等网行（voice #282 的蛋面镜像） ----------------

/// 裸蛋上 voice 不存在，没人画「正在联网…」。term 自己持有同一套 #282
/// 语义（常量照搬 voice）：开机窗内已配对未通网 → 等待行；internet ok →
/// 问候行（=状态一句话，selfnet_greet——与 voice status_text 同形状，09-10
/// 定稿剧场话术退役）；300s 窗尽 → 停止轮询（等待行不撤——红警面接着讲
/// 无网的故事）；voice 复活 → 整行让位（它的 BootNet 会写自己的等待行，
/// 两侧不叠）。
const SELFNET_UPTIME_GATE_SECS: f64 = 180.0;
const SELFNET_WATCH: Duration = Duration::from_secs(300);
const SELFNET_POLL: Duration = Duration::from_secs(5);
const SELFNET_WAITING: &str = "正在联网…";
const WIFI_CONF_PATH: &str = "/etc/wifi.conf";
const BOOT_STATE_PATH: &str = "/run/boot.state";

/// 布防四门（纯函数，真值表可测）：开机窗（uptime ≤180s——问候是开机的
/// 事）、已配对（wifi.conf 在——未配对机的路是配对面 C6，不是等网）、
/// 网未通（已通就直接问候，不占台）、voice 不在（face 文件从未出现——
/// voice 在则它自己等网，term 让位）。
fn selfnet_should_arm(uptime: f64, wifi_conf: bool, internet_ok: bool, voice_alive: bool) -> bool {
    uptime <= SELFNET_UPTIME_GATE_SECS && wifi_conf && !internet_ok && !voice_alive
}

fn boot_state_has_internet() -> bool {
    // 行形如 `internet ok www.baidu.com`（C3 后 aginx-pair apply 也会定点
    // 刷这四行）；run/fail/缺行都算未通。
    std::fs::read_to_string(BOOT_STATE_PATH)
        .map(|s| s.lines().any(|l| l.trim_start().starts_with("internet ok")))
        .unwrap_or(false)
}

fn uptime_secs() -> f64 {
    // 读不到（非 Linux host 测试）按超窗处理——不布防。
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|t| t.parse().ok()))
        .unwrap_or(f64::MAX)
}

struct SelfNet {
    /// 我们持有的行：Some(等待/问候) = 这张脸是 term 的；None = 让位/无台
    /// （voice 接管）。窗尽只停轮询，行不撤——镜像 voice 的文件残留语义。
    line: Option<String>,
    watching: bool,
    armed_at: Instant,
    last_poll: Instant,
}

impl SelfNet {
    fn idle() -> SelfNet {
        SelfNet { line: None, watching: false, armed_at: Instant::now(), last_poll: Instant::now() }
    }

    fn arm() -> SelfNet {
        SelfNet { line: Some(SELFNET_WAITING.to_string()), watching: true, ..SelfNet::idle() }
    }

    /// 主循环每拍。voice 复活即让位（任何态）；Watching 每 5s 读一次
    /// boot.state，internet ok → 问候；300s 窗尽 → 停轮询不撤行。
    /// 返回 true = 行变了，需要重画。
    fn tick(&mut self, voice_alive: bool) -> bool {
        if self.line.is_none() {
            return false;
        }
        if voice_alive {
            self.line = None;
            self.watching = false;
            eprintln!("aginx-term: selfnet yields to voice");
            return true;
        }
        if !self.watching {
            return false;
        }
        if self.armed_at.elapsed() >= SELFNET_WATCH {
            self.watching = false;
            eprintln!("aginx-term: selfnet window over — no greet");
            return false;
        }
        if self.last_poll.elapsed() < SELFNET_POLL {
            return false;
        }
        self.last_poll = Instant::now();
        if boot_state_has_internet() {
            self.line = Some(selfnet_greet());
            self.watching = false;
            eprintln!("aginx-term: selfnet up — greeted");
            true
        } else {
            false
        }
    }
}

/// 问候 = 状态一句话（voice status_text 同形状；问候时 internet 刚 ok，
/// 网恒已连）。蛋上 voice 不在，term 自己报告真状态——开机第一句话就是
/// 机器的真实状态。
fn selfnet_greet() -> String {
    let time = std::process::Command::new("date")
        .arg("+%H %M")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            // 按整数解析自然去前导零——字符串修剪法在 00 点会连吞两位
            // （"00点45分"→"点45分"，09-10 立案次日修，与 voice 同法）。
            let mut it = s.split_whitespace();
            let h: u32 = it.next()?.parse().ok()?;
            let m: u32 = it.next()?.parse().ok()?;
            Some(format!("{h}点{m}分"))
        })
        .unwrap_or_default();
    let bat = std::fs::read_to_string(format!(
        "{}/capacity",
        hwd::load_or_exit().paths.power_supply
    ))
    .ok()
    .and_then(|s| s.trim().parse::<u8>().ok())
    .unwrap_or(0);
    format!("{time}，电池{bat}%，网已连。")
}

// ---------------- C6: 自持扫码配网（voice 眼的蛋面镜像） ----------------

/// 常量照搬 voice（同名同值，见 voice main.rs）：取景总窗 30s（人对准
/// 之前机器不催，也不能永远开着镜头）、重生预算 3、卡帧 5s（首帧未落
/// 放宽一倍——子进程冷启动）、QR 解码限频 2Hz。
const TEYE_VIEW_SECS: u64 = 30;
const TEYE_RETRIES: u8 = 3;
const TEYE_STUCK_SECS: u64 = 5;
const TEYE_QR_EVERY: Duration = Duration::from_millis(400);
/// aginx-pair apply 侧预算 240s（join 90 + ntpd 20 + 两单元各 10，余量给
/// 首启冷路）；term 侧再加挂死保险：超窗 kill + 失败行。
const PAIR_JOB_BUDGET: Duration = Duration::from_secs(300);

/// C6 底部双目标条（未配对蛋面的入口）：左=扫码配网（本条）、右=软件
/// 清单（C7 开 Mode::Install——蛋的安装入口）。几何与 `Render::pair_bar` /
/// `pair_bar_hit` 同一套数字（测试钉住）。条画在键盘带——idle 面 kb 恒
/// 隐藏，那里本是死区，不与任何既有触摸目标重叠。
enum PairBar {
    Scan,
    Install,
}

/// 条的可见门（纯函数）：voice 不在（整机态配网面归 voice）且未配对
/// （无 wifi.conf——配上即隐退，入口不恋战）。
fn pair_bar_visible(voice_alive: bool, wifi_conf: bool) -> bool {
    !voice_alive && !wifi_conf
}

/// 条的命中几何：y∈[h-200, h-60]（下含上不含），左半=扫码配网、右半=
/// 软件清单。
fn pair_bar_hit(x: usize, y: usize, w: usize, h: usize) -> Option<PairBar> {
    if y >= h.saturating_sub(200) && y < h.saturating_sub(60) {
        if x < w / 2 {
            Some(PairBar::Scan)
        } else {
            Some(PairBar::Install)
        }
    } else {
        None
    }
}

/// 命中分诊（纯函数）：配对码（AGINXPAIR1 超集）或 WIFI: 连网码 →
/// Some（原样喂 aginx-pair apply——同一入口吃两种）；其他（文本码）→
/// None（C6 不消费，取景照常收）。
fn pick_pair_payload(payloads: &[String]) -> Option<String> {
    payloads.iter().find_map(|p| {
        let t = p.trim();
        (aginx_qr::parse_pair_payload(t).is_some() || aginx_qr::parse_wifi_payload(t).is_some())
            .then(|| t.to_string())
    })
}

/// term 自持取景会话（voice EyeView + 异步解码槽）。蛋上 voice 不存在，
/// 相机没有持有者——term 自己开 cam-shot、自己解 QR。解码绝不
/// `.output()`（100-300ms 纯计算也冻结主循环——冻屏红线）：spawn 进
/// `dec` 槽每拍 try_wait，退出后才读行。输出落
/// /run/aginx-voice/eye.{jpg,raw}——与 voice 同路径：voice 复活边沿由
/// 主循环护栏即时让路（相机一持有者）。
struct TermEye {
    child: std::process::Child,
    /// 取景总窗（TEYE_VIEW_SECS）基准
    since: Instant,
    /// 最近一次看到的 eye.jpg mtime（None=还没见过帧）。QR 只认 jpg——
    /// raw 是显示快路，JPEG 500ms 慢车道正好对上 2Hz 解码限频。
    mtime: Option<std::time::SystemTime>,
    /// 上次 mtime 变化（或 spawn）时刻——卡帧自愈基准
    mtime_seen: Instant,
    last_qr: Instant,
    retries: u8,
    /// 异步 aginx-qr 解码槽（Some=在算）
    dec: Option<std::process::Child>,
}

/// 取景一轮的退出决定（voice EyeExit 同形）。
enum TermEyeExit {
    Hit(Vec<String>),
    GiveUp(&'static str),
}

/// voice eye_spawn 同款 argv（粘合层平台旗标 + [quirks] eye_stream_args +
/// --aspect + 双产物落点 + cam.log 每开截断），唯一差别：先
/// `mkdir -p /run/aginx-voice`——整机上该目录归 voice 建，蛋上没人建。
fn term_cam_spawn() -> Result<std::process::Child, String> {
    let _ = std::fs::create_dir_all("/run/aginx-voice");
    let p = hwd::load_or_exit();
    let aspect = format!("{}:{}", p.panel.width, p.panel.height);
    let mut cmd = std::process::Command::new("/usr/bin/aginx-cam-shot");
    cmd.args(["--stream", "--rear", "--forever", "--aec", "--jpeg"])
        .arg("--jpeg-every-ms")
        .arg("500")
        .args(&p.quirks.eye_stream_args)
        .arg("--aspect")
        .arg(&aspect)
        .arg("--jpeg-out")
        .arg(VOICE_EYE)
        .arg("--raw-out")
        .arg(VOICE_EYE_RAW);
    // ⑤u 同律：一个日志文件，开眼截断；开不了退回 null（观察不能弄死眼）
    let log = std::fs::File::create("/run/aginx-voice/cam.log").ok();
    cmd.stdout(log.as_ref().and_then(|f| f.try_clone().ok()).map_or_else(
        std::process::Stdio::null,
        std::process::Stdio::from,
    ));
    cmd.stderr(log.map_or_else(std::process::Stdio::null, std::process::Stdio::from));
    cmd.spawn().map_err(|e| format!("cam-shot spawn: {e}"))
}

fn term_eye_spawn() -> Result<TermEye, String> {
    Ok(TermEye {
        child: term_cam_spawn()?,
        since: Instant::now(),
        mtime: None,
        mtime_seen: Instant::now(),
        last_qr: Instant::now(),
        retries: 0,
        dec: None,
    })
}

/// voice eye_stop 同款：TERM（STREAMOFF teardown + aec.state 落盘）→2s→
/// KILL（std 的 kill 只有 SIGKILL，先 libc::kill 发 TERM）。
fn term_eye_halt(child: &mut std::process::Child) {
    let pid = child.id() as i32;
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// 整会话收口：解码槽先杀（纯计算，KILL 即弃），相机 TERM→KILL，槽清空。
fn term_eye_stop(te: &mut Option<TermEye>) {
    if let Some(mut ev) = te.take() {
        if let Some(mut d) = ev.dec.take() {
            let _ = d.kill();
            let _ = d.wait();
        }
        term_eye_halt(&mut ev.child);
    }
}

/// 杀旧重生（voice eye_respawn 同序：retries+1、mtime 清零、节拍重置）。
/// 返回 Err = spawn 失败——下一轮 try_wait 再走重生/放弃路径。
fn term_eye_respawn(te: &mut TermEye) -> Result<(), String> {
    term_eye_halt(&mut te.child);
    te.retries += 1;
    te.mtime = None;
    te.mtime_seen = Instant::now();
    te.last_qr = Instant::now();
    te.child = term_cam_spawn()?;
    Ok(())
}

/// 配网 job（C6）：`aginx-pair apply` 异步单飞。payload 走 stdin（argv 恒
/// 两词——/proc/*/cmdline 永不出现 psk/三键，泄密是硬红线），stdout 汇总
/// 行收割时读首行（无字段值，apply 侧合同）。
struct PairJob {
    child: std::process::Child,
    since: Instant,
}

fn spawn_pair_apply(payload: &str) -> Option<PairJob> {
    let mut child = match std::process::Command::new("/usr/bin/aginx-pair")
        .arg("apply")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("aginx-term: aginx-pair spawn: {e}");
            return None;
        }
    };
    if let Some(mut si) = child.stdin.take() {
        // 块结束 drop 写端——apply 读到 EOF 收行；payload 短，write 不阻塞
        let _ = si.write_all(format!("{payload}\n").as_bytes());
    }
    Some(PairJob { child, since: Instant::now() })
}

/// idle 面状态行的所有权（纯函数）：voice 在 → 它的面（term 状态行让位，
/// 同 selfnet 让位律）；否则 配网行（String）> 装软件行 > selfnet 行
/// > None。装软件行压过 selfnet：配网刚成的靴上 sync 是当下的事。
fn idle_status(
    voice_alive: bool,
    pair_line: &Option<String>,
    install_line: &Option<String>,
    selfnet_line: Option<&str>,
) -> Option<String> {
    if voice_alive {
        return None;
    }
    pair_line
        .clone()
        .or_else(|| install_line.clone())
        .or_else(|| selfnet_line.map(|s| s.to_string()))
}

// ---------------- C7: 软件清单面（蛋的安装入口） ----------------

/// 判等用路径（pkg Paths 同款真值；不走 env 覆写——显示面读真源）。
const MANIFEST_PATH: &str = "/etc/agpkg.manifest";
const STAMPS_DIR: &str = "/var/lib/aginx/stamps";
const PKG_BINDIR: &str = "/var/bin";
/// 行 UI 每页行数（picker 同上限——Geom 行算术无符号，滚动分页顶替）。
const INSTALL_ROWS_PAGE: usize = 12;
/// 面开着时的行刷新节拍（manifest/stamps 都是小文件，读真源不缓存）。
const INSTALL_REFRESH: Duration = Duration::from_secs(5);
/// aginx-pkg job 挂死保险：首拉 8 包 ~700MB 国内镜像 ~10min，30min 才 kill。
const INSTALL_JOB_BUDGET: Duration = Duration::from_secs(1800);

#[derive(PartialEq, Clone, Copy, Debug)]
enum RowTier {
    Core,
    Opt,
}

/// 行四态：就绪（stamp 等且真身在）/ 待更（真身在 stamp 不等）/ 缺装
/// （core 没真身）/ 选装（opt 没真身——opt-in 装）。
#[derive(PartialEq, Clone, Copy, Debug)]
enum RowState {
    Ready,
    Stale,
    Missing,
    OptReady,
}

#[derive(PartialEq, Debug)]
struct InstallRow {
    name: String,
    version: Option<String>,
    tier: RowTier,
    state: RowState,
}

/// 清单行（纯函数，判等法照 pkg cmd_sync：stamp==sha 且真身在 = up to
/// date）。**不做** sync 的二进制自哈希复活（对 239MB 的 asr 哈希会把
/// 渲染循环冻住）——显示面只看 stamp+存在性，真判等归 `aginx-pkg sync`；
/// manifest 直读文本不验签（sig 门归 aginx-pkg）。坏行跳过不炸面。
fn install_rows(
    manifest_text: &str,
    stamps_dir: &std::path::Path,
    bindir: &std::path::Path,
) -> Vec<InstallRow> {
    let mut rows = Vec::new();
    for raw in manifest_text.lines() {
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 {
            continue;
        }
        let tier = if f.get(3) == Some(&"opt") { RowTier::Opt } else { RowTier::Core };
        let version = f.get(4).filter(|v| !v.is_empty()).map(|v| v.to_string());
        let bin = bindir.join(f[0]);
        let stamp = std::fs::read_to_string(stamps_dir.join(f[0]))
            .ok()
            .map(|s| s.trim().to_string());
        let state = if bin.exists() && stamp.as_deref() == Some(f[2]) {
            RowState::Ready
        } else if tier == RowTier::Opt {
            RowState::OptReady
        } else if bin.exists() {
            RowState::Stale
        } else {
            RowState::Missing
        };
        rows.push(InstallRow { name: f[0].to_string(), version, tier, state });
    }
    rows
}

/// 设备真源读一行集（auto 触发门与面刷新共用）。
fn device_install_rows() -> Vec<InstallRow> {
    let manifest = std::fs::read_to_string(MANIFEST_PATH).unwrap_or_default();
    install_rows(&manifest, std::path::Path::new(STAMPS_DIR), std::path::Path::new(PKG_BINDIR))
}

/// 装软件 job（C7）：`aginx-pkg sync` / `opt-in <name>` 异步单飞。auto
/// 触发与清单面点击共用这一个槽——同一引擎无旁路（provision resync 的
/// 配网当靴补跑）。
struct InstallJob {
    child: std::process::Child,
    since: Instant,
}

fn spawn_install_job(args: &[&str]) -> Option<InstallJob> {
    match std::process::Command::new(launch::BIN_AGINX_PKG)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => Some(InstallJob { child, since: Instant::now() }),
        Err(e) => {
            eprintln!("aginx-term: aginx-pkg spawn: {e}");
            None
        }
    }
}

/// 清单面状态（Mode::Install 的负载）：行集 + 页码。状态行不在此——
/// job 进度是 main() 的 install_line（idle 面与清单面同源）。
struct InstallView {
    rows: Vec<InstallRow>,
    page: usize,
    last_refresh: Instant,
}

impl InstallView {
    fn new() -> InstallView {
        InstallView { rows: device_install_rows(), page: 0, last_refresh: Instant::now() }
    }

    fn refresh(&mut self) -> bool {
        let rows = device_install_rows();
        if rows == self.rows {
            return false;
        }
        self.rows = rows;
        // 页码夹回有效域（行数缩了不悬空）
        let pages = self.pages();
        if self.page >= pages {
            self.page = pages.saturating_sub(1);
        }
        true
    }

    fn pages(&self) -> usize {
        self.rows.len().div_ceil(INSTALL_ROWS_PAGE)
    }
}

/// 翻页条命中：Some(-1)=上一页、Some(1)=下一页（条画在 kb_panel_y-150
/// 高 80，左右各 240px 格；几何与 `Render::install_list` 同一套数字，
/// 测试钉住）。
fn install_page_hit(x: usize, y: usize, w: usize, kb_panel_y: usize) -> Option<i32> {
    let y0 = kb_panel_y.saturating_sub(150);
    if y >= y0 && y < y0 + 80 {
        if x >= 60 && x < 300 {
            Some(-1)
        } else if x >= w.saturating_sub(300) && x < w.saturating_sub(60) {
            Some(1)
        } else {
            None
        }
    } else {
        None
    }
}

/// The prompt face's render (开机剧情 v4): the transcript typewriter face.
/// `breath` = cursor level 0..=16. The live result page never comes through
/// here — its frames blit straight into the back buffer (result_frame).
/// C5 `selfnet` / C6 `pair_line`: term 自己的状态行（等待网/问候/配网进度）
/// 画在 transcript 的位置，整行直显（状态不是台词，不打字机），光标在
/// 行尾呼吸；voice 在时归让（idle_status 判所有权）。
fn render_prompt(
    r: &Render,
    pix: &mut [u32],
    voice: &VoiceView,
    breath: u8,
    warns: &[String],
    status: Option<&str>,
) {
    let (line, prog) = match status {
        Some(s) => (s, usize::MAX),
        None => (voice.doc.line.as_deref().unwrap_or(""), voice.line_prog),
    };
    r.prompt(pix, line, prog, Some(breath));
    // 警告注册表非空 → 中屏红警区叠加；顶部 transcript+呼吸光标不动
    if !warns.is_empty() {
        r.warn(pix, warns);
    }
    // C6 未配对蛋面：底部双目标条（扫码配网 / 软件清单）
    if pair_bar_visible(voice.alive, std::path::Path::new(WIFI_CONF_PATH).exists()) {
        r.pair_bar(pix);
    }
}

/// v4⑥: fullscreen 1:1 row-copy of a live-panel screencast frame straight
/// into the DRM back buffer — the eye raw path's `direct` sibling. No
/// canvas, no 10 MB copy. Caller guarantees the frame is panel-sized.
fn blit_result_direct(back: &mut [u32], pitch: usize, bm: &aginx_img::Bitmap) {
    let bw = bm.w as usize;
    for j in 0..bm.h as usize {
        back[j * pitch..j * pitch + bw].copy_from_slice(&bm.pix[j * bw..j * bw + bw]);
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

/// Host --ppm panel geometry (D14)：env AGINX_DEVICE_TOML > /etc/aginx/
/// device.toml > 本仓烤机的真档案（host 转储缺省直指真数据，不落数字）。
/// env 只在单线程 CLI 入口设置——测试线程下有 OnceLock 竞态（voice
/// audio.rs 只在测试里 from_path，同一纪律）。
fn host_panel() -> (usize, usize) {
    if std::env::var_os(hwd::DEVICE_TOML_ENV).is_none()
        && !std::path::Path::new(hwd::DEVICE_TOML_PATH).exists()
    {
        std::env::set_var(
            hwd::DEVICE_TOML_ENV,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../devices/redfin/device.toml" // D14-exempt: committed real profile
            ),
        );
    }
    let p = &hwd::load_or_exit().panel;
    (p.width as usize, p.height as usize)
}

fn host_ppm(out: &str) {
    let font = font::font_init();
    let (w, h) = host_panel();
    let pitch = w;
    let mut pix = vec![0u32; pitch * h];
    let kg = Kb::geom(w, h);
    // 批③: the launcher fed entries.len() here; the install face owns the
    // row geometry now — one page of rows.
    let lg = launch::Geom::new(w, h, kg.extra_y, INSTALL_ROWS_PAGE);
    let r = Render { font: &font, w, h, pitch };
    // 开机剧情 v4⑤: the prompt-face beats — top-anchor breathing cursor (full
    // + dim ends of the breath range), mid-typing CJK transcript (wrap
    // exercise, top anchor), result frame — dumps exist to eyeball every
    // console beat on the host.
    {
        let mut pixi = vec![0u32; pitch * h];
        r.prompt(&mut pixi, "", 0, Some(16));
        let path = format!("{}-prompt-idle", out);
        if let Err(e) = ppm_dump(&path, &pixi, w, h, pitch) {
            eprintln!("ppm: {e}");
        }
        println!("wrote {path}");
        let mut pixd = vec![0u32; pitch * h];
        r.prompt(&mut pixd, "", 0, Some(0));
        let path = format!("{}-prompt-idle-dim", out);
        if let Err(e) = ppm_dump(&path, &pixd, w, h, pitch) {
            eprintln!("ppm: {e}");
        }
        println!("wrote {path}");
    }
    {
        let mut pixi = vec![0u32; pitch * h];
        r.prompt(
            &mut pixi,
            "帮我把客厅的摄像头画面调出来，再看看今天下午的日程安排",
            14,
            None,
        );
        let path = format!("{}-prompt-typing", out);
        if let Err(e) = ppm_dump(&path, &pixi, w, h, pitch) {
            eprintln!("ppm: {e}");
        }
        println!("wrote {path}");
    }
    {
        // 警告注册表 (2026-09-09): idle + red center zone — the standby
        // face a dead network leaves on screen (top cursor untouched).
        let mut pixw = vec![0u32; pitch * h];
        r.prompt(&mut pixw, "", 0, Some(16));
        r.warn(&mut pixw, &["无网络 · 自动重连中".to_string()]);
        let path = format!("{}-prompt-warn", out);
        if let Err(e) = ppm_dump(&path, &pixw, w, h, pitch) {
            eprintln!("ppm: {e}");
        }
        println!("wrote {path}");
    }
    // first frame (批③): the install face — the launcher's successor. Rows
    // from the pure install_rows() on a two-tier fixture (missing core /
    // opt-ready) so the row render path stays host-verifiable.
    let iv = InstallView {
        rows: install_rows(
            "aginx-server https://example.invalid/a aaaa core 0.9.0\naginx-ocr https://example.invalid/b bbbb opt\n",
            std::path::Path::new("/nonexistent-aginx-stamps"),
            std::path::Path::new("/nonexistent-aginx-bin"),
        ),
        page: 0,
        last_refresh: Instant::now(),
    };
    r.install_list(&mut pix, &iv, None, false, &lg);

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
    r.terminal(&mut pix2, &t, area_top0, sc0, true, kb::KB_M);
    r.keyboard(&mut pix2, &kg, &kb0());
    let term_path = format!("{}-term", out);
    if let Err(e) = ppm_dump(out, &pix, w, h, pitch) {
        eprintln!("ppm: {e}");
    }
    if let Err(e) = ppm_dump(&term_path, &pix2, w, h, pitch) {
        eprintln!("ppm: {e}");
    }

    // third frame (M40): pinyin IME — AGINX_TERM_IME_DEMO=<syllable> latches
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
        r.terminal(&mut pix4, &t, area_top0, sc0, true, kb::KB_M);
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

/// M47⑤t: while the eye streams, park this process on [affinity] ui_cores
/// (the little cluster). cam-shot pins its process to the big pair
/// ([affinity] cam_cores) and the pixel chain saturates both; term's fused
/// 565→888 + bilinear upscale measured ~37% of one big core per frame (⑤i
/// probe) and aginx-qr's decode bursts (2 Hz, 100-300 ms) landed unpinned
/// on the pair — together they erased the 70 ms fast-frame mode in service
/// (in-service min 83 ms vs isolated 70 ms, 2026-09-06). The upscale fits
/// in one little core; at eye close the full mask returns (terminal
/// gets the big cores back — union of ui+big+cam, sorted+deduped).
///
/// Called from main() on eye-FLAG transitions in ANY mode — the first cut
/// hooked it inside poll_eye (the retired voice face only), which leaked
/// the park: VolUp is handled by the voice daemon regardless of the view
/// on screen, so the stream can outlive the eye view, and a back-out
/// mid-stream left this process parked forever.
#[cfg(target_os = "linux")]
fn set_eye_affinity(on: bool) {
    let a = &hwd::load_or_exit().affinity;
    let cores: Vec<u32> = if on {
        a.ui_cores.clone()
    } else {
        let mut all = a.ui_cores.clone();
        all.extend_from_slice(&a.big_cores);
        all.extend_from_slice(&a.cam_cores);
        all.sort_unstable();
        all.dedup();
        all
    };
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &c in &cores {
            libc::CPU_SET(c as usize, &mut set);
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}
#[cfg(not(target_os = "linux"))]
fn set_eye_affinity(_on: bool) {}

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
    // D14 软校验：DRM 枚举是显示真值；[panel] 供非 DRM 消费者（voice HTML
    // 三钉 / cam --aspect / 触摸缩放）。不符（烤错档案）大声警告，不致命
    // ——显示继续按 DRM 走，错的是数据侧该修数据。
    {
        let p = hwd::load_or_exit();
        if p.panel.width != d.width || p.panel.height != d.height {
            eprintln!(
                "aginx-term: panel mismatch — drm {}x{}, profile {}x{} ({})",
                d.width, d.height, p.panel.width, p.panel.height, p.device.name
            );
        }
    }
    let pitch = d.pitch_px();

    let mut kb = Kb::new();
    let mut ime = pinyin::Ime::new(); // M40: 拼 buffer + candidate page
    let kg = Kb::geom(w, h);
    // 批③: the install face owns the row geometry now (the launcher fed
    // entries.len() here) — one 12-row page, install_list 分页翻行.
    let lg = launch::Geom::new(w, h, kg.extra_y, INSTALL_ROWS_PAGE);

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
    // 开机剧情 v4: boot lands on the prompt face (纯黑+光标). The eye race
    // still gets one face poll first — the eye open wins if voice already
    // flagged it.
    let mut voice = VoiceView::default();
    let mut mode = {
        voice.poll();
        if voice.alive && voice.doc.eye {
            Mode::Eye
        } else {
            Mode::Idle
        }
    };
    // 面法: the eye flag drives Mode::Eye transitions in the loop — this
    // mirrors the loop's edge detector (voice.poll() already ran above).
    let mut eye_on_prev = voice.alive && voice.doc.eye;
    // C5 自持等网行：四门全过才布防（见 selfnet_should_arm）。蛋上 voice
    // 永不出现，这张脸由 term 持有；整机上 voice 起慢了也只是短暂接管、
    // 复活即让位。首帧（下方首画块）就带着等待行——不等技术循环。
    let mut selfnet = if selfnet_should_arm(
        uptime_secs(),
        std::path::Path::new(WIFI_CONF_PATH).exists(),
        boot_state_has_internet(),
        voice.alive,
    ) {
        eprintln!("aginx-term: selfnet watching");
        SelfNet::arm()
    } else {
        SelfNet::idle()
    };
    // C6 自持扫码配网三件套：未配对（无 wifi.conf）且 voice 不在 → Idle 面
    // 底部双目标条（扫码配网 / 软件清单）。取景会话 term_eye、配网 job
    // （单飞）、状态行 pair_line（idle 面显示层所有权见 idle_status）。
    let mut term_eye: Option<TermEye> = None;
    let mut pair_line: Option<String> = None;
    let mut pair_job: Option<PairJob> = None;
    // C7 装软件 job 槽（单飞）：auto 触发与清单面点击共用同一条引擎。
    // install_line 是 idle 面与清单面共用的状态行；install_auto 只点火一次
    // ——把 provision resync 在配网当靴补跑（蛋首启 provision 早退在
    // wifi fail，配网成功后由这里接管安装承诺）。
    let mut install_job: Option<InstallJob> = None;
    let mut install_line: Option<String> = None;
    let mut install_auto = false;
    // 面法: mode boxed away while Mode::Eye has the screen — restored on
    // eye close; None (boot straight into the eye) → Idle.
    let mut mode_before_eye: Option<Box<Mode>> = None;
    // M47⑤t: last affinity decision from the eye flag (see the main-loop
    // watcher) — keeps sched_setaffinity off the no-change path.
    let mut eye_parked = false;
    // Debug/headless path: AGINX_TERM_START=<bin> spawns the program
    // immediately (e.g. AGINX_TERM_START=/bin/sh) — the only way a pty
    // session starts since the launcher came down (批③).
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
    }
    // 未连网的开机不再自动拉 wizard：纯光标面 + PTT 语音流程就是装机流程
    // （对准配对码，M42c 链）。批② C2（09-10）摘瓦片；批③（09-10）wizard
    // 出蛋——AGINX_TERM_START 只起绝对路径，不再有 wizard 可达。
    // Input nodes are panel data ([input.term], D14) — touch + the pon
    // keys (power + volume-down) ride whatever the profile declares.
    let ipt = hwd::load_or_exit().input.term.clone();
    let mut touch = TouchReader::open(&ipt.touch_device, w as i32, h as i32);
    let mut pwr = KeyReader::open(&ipt.power_device);
    // M15 blank state
    let mut blanked = false;
    let mut last_input = Instant::now();
    let mut power_down: Option<Instant> = None;

    // 警告注册表 (2026-09-09): polled on the idle tick; the first frame
    // carries whatever is already on disk (net down at term start → red
    // from the very first paint).
    let mut warns: Vec<String> = read_warnings();
    // Persistent canvas: renderers repaint only damaged rows into it, and
    // each present() memcpy's it into the back buffer (~10 MB, ~1 ms) so
    // double-buffer semantics survive partial redraws.
    let mut canvas = vec![0u32; pitch * h];
    // First frame BEFORE the mode set (panel snapshots at SETCRTC).
    {
        let r = Render { font: &font, w, h, pitch };
        let buf = &mut canvas[..];
        match &mode {
            Mode::Idle => render_prompt(
                &r,
                buf,
                &voice,
                16,
                &warns,
                idle_status(voice.alive, &pair_line, &install_line, selfnet.line.as_deref()).as_deref(),
            ),
            Mode::Eye => r.eye(buf, &voice, &lg),
            Mode::Install(v) => {
                r.install_list(buf, v, install_line.as_deref(), boot_state_has_internet(), &lg)
            }
            Mode::Running(_) => {
                fill_rect(buf, pitch, w, h, 0, 0, w as i32, h as i32, BG);
                r.toolbar(buf, lg.m, lg.toolbar_h);
                r.terminal(buf, &term, area_top, scale, true, lg.m);
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
    // v4⑤ breath tick: the idle cursor's phase (0..=32, triangle — level
    // = tick<=16 ? tick : 32-tick), starts full to match the first frame
    let mut last_breath = Instant::now();
    let mut breath_tick: u8 = 16;
    // 警告注册表 poll (2026-09-09): 2 s cadence on the idle face; repaint
    // only when the set changes.
    let mut last_warn_poll = Instant::now();
    let mut kb_dirty = true;
    // Hold-to-repeat (DEL / arrows), Termux-style: the event + next fire
    // deadline. Repeats go through inject() like every other input.
    let mut held: Option<(InputEvent, Instant)> = None;
    let mut down_y = 0usize; // where the current touch started
    // M47⑤f frame-arrival watch (armed lazily — the directory may not
    // exist yet when term starts at boot).
    let (ino_fd, mut ino_wd) = ino_init();
    // v4⑥: the live result page — a CDP panel client (browser.rs). Lives
    // exactly while face.result is set; screencast frames land in
    // result_frame and present under the Idle && result && !blanked gate.
    let mut live: Option<browser::Browser> = None;
    let mut result_frame: Option<aginx_img::Bitmap> = None;
    let mut result_prev = false;

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
                // 批③: the launcher is gone — a finished session returns to
                // the 待命面 (the resting face).
                mode = Mode::Idle;
                kb_visible = false;
                scale = 5;
                term_cols = cols_for(scale);
                term = Term::new(term_cols, rows_for(false, scale));
                parser = vte::Parser::new();
                redraw = true;
            }
        }

        // input (touch / power key / pty / CDP)
        let mut fds = [libc::pollfd { fd: -1, events: libc::POLLIN, revents: 0 }; 6];
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
        // eye view is on screen — every eye.raw / eye.jpg / face publish
        // then wakes the loop the instant it lands. 开机剧情 v4: same while
        // the result face shows (face / result.html publishes wake the loop).
        if ino_wd >= 0
            && (matches!(mode, Mode::Eye)
                || (matches!(mode, Mode::Idle) && voice.doc.result))
        {
            fds[nfds].fd = ino_fd;
            nfds += 1;
        }
        // v4⑥: the live page's CDP socket — POLLIN wakes the pump the
        // instant a screencast frame (or Setup reply) lands; POLLOUT arms
        // only while a large command (the ~130 KB navigate) is mid-drain.
        if let Some(b) = live.as_ref() {
            if let Some(fd) = b.fd() {
                fds[nfds].fd = fd;
                fds[nfds].events = libc::POLLIN
                    | if b.wants_write() {
                        libc::POLLOUT
                    } else {
                        0
                    };
                nfds += 1;
            }
        }
        let timeout: libc::c_int = if redraw {
            0
        } else if held.is_some() || power_down.is_some() {
            30
        } else if live.as_ref().and_then(|b| b.fd()).is_some() {
            // v4⑥: the panel pump cadence — frames and Setup replies wake
            // via POLLIN; this timer only carries the 0.3 s heartbeat and
            // the Setup op pacing.
            60
        } else if matches!(mode, Mode::Eye) {
            // M47⑤b: the eye polls files on this cadence — 400 ms capped
            // the viewfinder display at 2.5 fps even with cam-shot
            // publishing ~8 fps (user receipt 2026-09-05 「看起来很卡」).
            // M47⑤f: with the frame-arrival watch armed, IN_MOVED_TO wakes
            // the loop the instant a frame or face write lands — the timer
            // is only a 200 ms safety net. Unarmed (directory absent), the
            // stat cadence carries it: 12 ms while the eye is live, 30 ms
            // idle.
            if ino_wd >= 0 {
                200
            } else if voice.doc.eye { 12 } else { 30 }
        } else if matches!(mode, Mode::Idle) && voice.typing() {
            // 开机剧情 v4: the transcript typewriter animates at ~90 ms/char —
            // poll the face file on that cadence while the reveal is live
            90
        } else if matches!(mode, Mode::Idle) && !voice.doc.result {
            // v4⑤: the breathing cursor cadence (16 levels × 125 ms ≈ 4 s
            // period); a result on the panel is static — keep the idle 400 ms
            125
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
                        // 面法 09-07: waking from blank lands on the 待机面
                        // (eye open → 眼视图). A Running session restores
                        // in place.
                        if matches!(mode, Mode::Idle | Mode::Eye) {
                            mode = if (voice.alive && voice.doc.eye) || term_eye.is_some() {
                                Mode::Eye
                            } else {
                                Mode::Idle
                            };
                        }
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
                                if lg.toolbar_hit(x, y, matches!(mode, Mode::Running(_) | Mode::Install(_)))
                                    == Some(launch::Toolbar::Back)
                                {
                                    if let Mode::Running(c) = &mode {
                                        unsafe { libc::kill(c.pid, libc::SIGHUP) };
                                    } else if matches!(mode, Mode::Install(_)) {
                                        // C7 清单面：BACK 回待机面
                                        mode = Mode::Idle;
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
                                } else if let Mode::Install(v) = &mut mode {
                                    // C7 清单面：翻页条优先（条在 kb_panel_y-150，
                                    // 与行区不重叠）；行点按按态分诊催装（单飞槽，
                                    // job 在跑就只重画）。Ready 不点。
                                    if let Some(dp) = install_page_hit(x, y, w, lg.kb_panel_y) {
                                        let pages = v.pages();
                                        let np =
                                            ((v.page as isize + dp as isize).clamp(0, pages as isize - 1)) as usize;
                                        if np != v.page {
                                            v.page = np;
                                            redraw = true;
                                        }
                                    } else if let Some(i2) = lg.button_at(x, y, INSTALL_ROWS_PAGE) {
                                        if let Some(row) = v.rows.get(v.page * INSTALL_ROWS_PAGE + i2) {
                                            match row.state {
                                                RowState::Missing | RowState::Stale => {
                                                    if install_job.is_none()
                                                        && spawn_install_job(&["sync"]).is_some()
                                                    {
                                                        install_line = Some("同步中…".into());
                                                    }
                                                    redraw = true;
                                                }
                                                RowState::OptReady => {
                                                    let name = row.name.clone();
                                                    if install_job.is_none()
                                                        && spawn_install_job(&["opt-in", &name]).is_some()
                                                    {
                                                        install_line = Some(format!("装 {name} 中…"));
                                                    }
                                                    redraw = true;
                                                }
                                                RowState::Ready => {}
                                            }
                                        }
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
                            // C6 未配对蛋面入口：双目标条画在键盘带（idle
                            // 面 kb 恒隐藏，那里本是死区）。左格=扫码配网；
                            // 右格=软件清单（C7 开 Mode::Install）。
                            if matches!(mode, Mode::Idle)
                                && pair_bar_visible(
                                    voice.alive,
                                    std::path::Path::new(WIFI_CONF_PATH).exists(),
                                )
                            {
                                match pair_bar_hit(x, y, w, h) {
                                    Some(PairBar::Scan) => {
                                        // 配网 job 在跑 → 单飞让路（「配网中…」
                                        // 行已在陈述状态）
                                        if pair_job.is_none() {
                                            // paint-first：第一帧 ~2s 在路上，
                                            // 「取景中…」先上屏再开相机
                                            {
                                                let r = Render { font: &font, w, h, pitch };
                                                r.eye(&mut canvas[..], &voice, &lg);
                                                d.back_buf().copy_from_slice(&canvas);
                                                d.present();
                                            }
                                            match term_eye_spawn() {
                                                Ok(te) => {
                                                    term_eye = Some(te);
                                                    mode = Mode::Eye;
                                                    redraw = true;
                                                }
                                                Err(e) => {
                                                    eprintln!("aginx-term: eye spawn {e}");
                                                    pair_line = Some("相机没起来，再试一次。".into());
                                                    redraw = true;
                                                }
                                            }
                                        }
                                    }
                                    Some(PairBar::Install) => {
                                        mode = Mode::Install(InstallView::new());
                                        redraw = true;
                                    }
                                    None => {}
                                }
                            } else if matches!(mode, Mode::Eye) && term_eye.is_some() {
                                // C6 自持取景的点按退出（voice 的眼由音量键
                                // 管，term 的眼触屏全权）
                                term_eye_stop(&mut term_eye);
                                mode = Mode::Idle;
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
                            // v4⑥: the result face is a fullscreen scroll
                            // area — the live page scrolls directly, no
                            // keyboard threshold involved.
                            if matches!(mode, Mode::Idle) && voice.doc.result {
                                if let Some(b) = live.as_mut() {
                                    b.scroll_by(dy as isize);
                                }
                            } else {
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
                                    // 面法 09-07: waking from blank lands on
                                    // the 待机面 (eye open → 眼视图); debug
                                    // modes restore in place
                                    if matches!(mode, Mode::Idle | Mode::Eye) {
                                        mode = if (voice.alive && voice.doc.eye) || term_eye.is_some() {
                                            Mode::Eye
                                        } else {
                                            Mode::Idle
                                        };
                                    }
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
            // M47⑤f: frame-arrival wake — just drain; the eye/result poll
            // below stats and renders if anything actually changed.
            if ino_wd >= 0
                && (matches!(mode, Mode::Eye)
                    || (matches!(mode, Mode::Idle) && voice.doc.result))
            {
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

        // M42a voice face: polled in EVERY mode (⑤t) — the eye flag in this
        // doc drives this process's CPU affinity. A live frame stream counts
        // as activity — the screen must not blank mid-flow (a face write
        // arrives exactly when the eye opens) — but that keep-awake, like
        // the rendering, is Eye-mode-only.
        // M42g: the viewfinder frame polls too — a frame landing ~1/s is
        // activity; decode box is the whole panel.
        let face = voice.poll();
        // C6 相机互斥护栏：voice 复活边沿（face 文件首次可读）即让路——
        // 整机态相机归 voice 独占，同路径输出只是兜底不是协议。让路同时
        // 清 term 状态行（idle 面归还 voice，同 selfnet 让位律）。
        if voice.alive && term_eye.is_some() {
            eprintln!("aginx-term: voice revived — term eye yields");
            term_eye_stop(&mut term_eye);
            pair_line = None;
            if matches!(mode, Mode::Eye) {
                mode = Mode::Idle;
            }
            redraw = true;
        }
        // 面法 09-07: the eye FLAG drives Mode::Eye from ANY mode — open
        // steals the screen (prior mode boxed away), close hands it back
        // (dead prior → Idle). A vanished face file (voice daemon death
        // mid-eye) counts as closed.
        let eye_on = voice.alive && voice.doc.eye;
        if eye_on != eye_on_prev {
            eye_on_prev = eye_on;
            if eye_on {
                mode_before_eye = Some(Box::new(std::mem::replace(&mut mode, Mode::Eye)));
            } else {
                mode = mode_before_eye.take().map(|m| *m).unwrap_or(Mode::Idle);
            }
            redraw = true;
        }
        // v4⑥: face.result edges drive the live panel client. Rising: read
        // result.html (voice publishes it BEFORE the flag; atomic rename)
        // and dial the engine. ①a: file missing/empty (voice died before
        // the rename, /run wiped) → rebuild a degraded page from the
        // session ledger; ledger doesn't corroborate (mother-direct turns
        // keep no ledger) → keep the prompt face (text stays first-class).
        // Falls: teardown — closeTarget must go out even best-effort (滞留 target
        // 会让引擎 RSS 爬坡).
        let result_on = voice.alive && voice.doc.result;
        if result_on != result_prev {
            result_prev = result_on;
            if result_on {
                let mut html: Option<String> = None;
                if let Ok(h) = std::fs::read_to_string(VOICE_RESULT_HTML) {
                    if !h.trim().is_empty() {
                        html = Some(h);
                    }
                }
                if html.is_none() {
                    let pn = &hwd::load_or_exit().panel;
                    if let Some(h) = recover_result_html(
                        &workspaces_root(),
                        voice.doc.line.as_deref(),
                        pn.width,
                        pn.height,
                    ) {
                        eprintln!("aginx-term: result.html missing, rebuilt from ledger");
                        html = Some(h);
                    }
                }
                if let Some(h) = html {
                    live = Some(browser::Browser::start(&h));
                    result_frame = None;
                }
            } else {
                if let Some(b) = live.as_mut() {
                    b.teardown();
                }
                live = None;
                result_frame = None;
            }
            redraw = true;
        }
        // v4⑥: pump the panel client every pass — Setup advances one CDP op,
        // Live drains the socket (screencast frames: ack first, then decode)
        // and keeps the 0.3 s heartbeat going. Decode is gated on the frame
        // actually being presentable (Idle && result && !blanked): with the
        // eye open or the panel blanked we still ack — the stream must not
        // stall — but skip the ~70 ms jpeg decode.
        if let Some(b) = live.as_mut() {
            let can_present = matches!(mode, Mode::Idle) && voice.doc.result && !blanked;
            if let Some(bm) = b.pump(Instant::now(), can_present) {
                if bm.w as usize == w && bm.h as usize == h {
                    result_frame = Some(bm);
                } else {
                    // 帧解出但尺寸不合门——不进 result_frame，仍触发重绘
                    eprintln!(
                        "aginx-term: frame size {}x{} != panel {}x{}",
                        bm.w, bm.h, w, h
                    );
                }
                last_input = Instant::now();
                redraw = true;
            }
        }
        // 开机剧情 typewriter (#246): the daemon swaps doc.line, term owns
        // the reveal (~90 ms/char, the bootcard END cadence). An extension
        // keeps the revealed prefix (Act 3b's line 2 types after line 1)
        // and restarts the clock from that baseline; any other change
        // types from scratch.
        let line_now = voice.doc.line.clone();
        if line_now != voice.line_seen {
            let extends = match (&voice.line_seen, &line_now) {
                (Some(a), Some(b)) if b.starts_with(a.as_str()) => true,
                _ => false,
            };
            if !extends {
                voice.line_prog = 0;
                voice.line_base = 0;
            } else {
                voice.line_base = voice.line_prog;
            }
            voice.line_seen = line_now;
            voice.type_at = Some(Instant::now());
            redraw = true;
        }
        if let Some(l) = voice.doc.line.as_ref() {
            let total = l.chars().count();
            // v4⑥ 修：want 是不封顶的时钟，打完后永远 > line_prog —— 不加
            // typing() 门会每 pass 重绘（静态结果页 8.8 presents/s 白烧）。
            if voice.line_prog < total {
                let elapsed = voice
                    .type_at
                    .map(|t| t.elapsed().as_millis() as usize / 90)
                    .unwrap_or(0);
                let want = voice.line_base + elapsed;
                if want > voice.line_prog {
                    voice.line_prog = want.min(total);
                    redraw = true;
                }
            }
        }
        // 开机剧情 v4: a result on the panel is activity — the page must
        // not idle-blank while the user reads it (结果页不超时, 面法
        // 09-07). The bare prompt face blanks normally (60 s): the console
        // sleeps when you do.
        if voice.doc.result {
            last_input = Instant::now();
        }
        if matches!(mode, Mode::Eye) {
            let (_, _, eye_w, eye_h) = lg.eye_box();
            let eye = voice.poll_eye(eye_w as u32, eye_h as u32, term_eye.is_some());
            if face || eye {
                last_input = Instant::now();
                if blanked {
                    blanked = false;
                    d.dpms(true);
                }
                redraw = true;
            }
        }
        // ---- C6 自持取景生命周期（voice eye 同律：总窗/重生/卡帧/异步解码） ----
        let mut te_exit: Option<TermEyeExit> = None;
        if let Some(te) = term_eye.as_mut() {
            if te.since.elapsed() >= Duration::from_secs(TEYE_VIEW_SECS) {
                te_exit = Some(TermEyeExit::GiveUp("没拍到码，再点扫码配网。"));
            } else {
                match te.child.try_wait() {
                    Ok(Some(_)) => {
                        eprintln!("aginx-term: eye cam-shot exit");
                        if te.retries >= TEYE_RETRIES {
                            te_exit = Some(TermEyeExit::GiveUp("相机反复掉线，取景关闭。"));
                        } else if term_eye_respawn(te).is_err() {
                            te_exit = Some(TermEyeExit::GiveUp("相机没起来，取景关闭。"));
                        }
                    }
                    Err(e) => {
                        eprintln!("aginx-term: eye wait {e}");
                        te_exit = Some(TermEyeExit::GiveUp("相机掉线，取景关闭。"));
                    }
                    Ok(None) => {
                        // 解码槽收割：退出才读行（绝不 .output()——冻屏红线）
                        if let Some(mut dec) = te.dec.take() {
                            match dec.try_wait() {
                                Ok(Some(st)) => {
                                    if st.success() {
                                        let mut out = String::new();
                                        if let Some(mut r) = dec.stdout.take() {
                                            let _ = std::io::Read::read_to_string(&mut r, &mut out);
                                        }
                                        let payloads: Vec<String> = out
                                            .lines()
                                            .map(|l| l.trim().to_string())
                                            .filter(|l| !l.is_empty())
                                            .collect();
                                        if !payloads.is_empty() {
                                            te_exit = Some(TermEyeExit::Hit(payloads));
                                        }
                                    } // rc=1 没码——槽已清，下帧再试
                                }
                                Ok(None) => te.dec = Some(dec), // 还在算
                                Err(_) => {} // 槽丢弃，下帧重起
                            }
                        }
                        // 帧轮询：mtime 变 → 异步起解码；停滞 → 卡帧自愈重生
                        let mtime = std::fs::metadata(VOICE_EYE).and_then(|m| m.modified()).ok();
                        match mtime {
                            Some(t) if Some(t) != te.mtime => {
                                te.mtime = Some(t);
                                te.mtime_seen = Instant::now();
                                if te.last_qr.elapsed() >= TEYE_QR_EVERY && te.dec.is_none() {
                                    te.last_qr = Instant::now();
                                    match std::process::Command::new("/usr/bin/aginx-qr")
                                        .arg(VOICE_EYE)
                                        .stdout(std::process::Stdio::piped())
                                        .stderr(std::process::Stdio::null())
                                        .spawn()
                                    {
                                        Ok(c) => te.dec = Some(c),
                                        Err(e) => eprintln!("aginx-term: qr spawn: {e}"),
                                    }
                                }
                            }
                            _ => {
                                // mtime 没动（或首帧未落）超时 → 杀重生。首帧
                                // 预算放宽一倍（子进程冷启动，voice 同律）
                                let stuck =
                                    if te.mtime.is_none() { 2 * TEYE_STUCK_SECS } else { TEYE_STUCK_SECS };
                                if te.mtime_seen.elapsed() >= Duration::from_secs(stuck) {
                                    if te.retries >= TEYE_RETRIES {
                                        te_exit = Some(TermEyeExit::GiveUp("取景卡住了，取景关闭。"));
                                    } else {
                                        eprintln!("aginx-term: eye stuck frame, respawn");
                                        let _ = term_eye_respawn(te);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(exit) = te_exit {
            // paint-first：收相机（TERM→2s→KILL）可能阻塞，先把将要显示的
            // 那一帧放上面——命中即「配网中…」，放弃即失败行。
            let hit_payload = match &exit {
                TermEyeExit::Hit(payloads) => pick_pair_payload(payloads),
                TermEyeExit::GiveUp(msg) => {
                    pair_line = Some(msg.to_string());
                    None
                }
            };
            if hit_payload.is_some() {
                pair_line = Some("配网中…".into());
                {
                    let r = Render { font: &font, w, h, pitch };
                    render_prompt(&r, &mut canvas[..], &voice, 16, &warns, Some("配网中…"));
                    d.back_buf().copy_from_slice(&canvas);
                    d.present();
                }
            }
            term_eye_stop(&mut term_eye);
            if matches!(mode, Mode::Eye) {
                mode = Mode::Idle;
            }
            if let Some(payload) = hit_payload {
                match spawn_pair_apply(&payload) {
                    Some(job) => pair_job = Some(job),
                    None => pair_line = Some("配网没成，再试一次。".into()),
                }
            } else if let TermEyeExit::Hit(_) = &exit {
                // 命中了但不是配对码/连网码——文本码 C6 不消费
                pair_line = Some("不是配对码，取景已关。".into());
            }
            redraw = true;
        }
        // ---- C6 配网 job 收割：每拍 try_wait（绝不阻塞等）；预算尽才 kill。
        // 汇总行 = stdout 首行（apply 只在成功时出一行，无字段值）----
        if let Some(mut job) = pair_job.take() {
            let mut done = false;
            let mut ok = false;
            match job.child.try_wait() {
                Ok(Some(st)) => {
                    done = true;
                    ok = st.success();
                    if ok {
                        let mut out = String::new();
                        if let Some(mut r) = job.child.stdout.take() {
                            let _ = std::io::Read::read_to_string(&mut r, &mut out);
                        }
                        let line = out.lines().next().unwrap_or("").trim().to_string();
                        pair_line =
                            Some(if line.is_empty() { "配网完成。".into() } else { line });
                    } else {
                        eprintln!("aginx-term: pair apply exit {}", st.code().unwrap_or(-1));
                    }
                }
                Ok(None) => {
                    if job.since.elapsed() >= PAIR_JOB_BUDGET {
                        eprintln!("aginx-term: pair apply budget over — kill");
                        let _ = job.child.kill();
                        let _ = job.child.wait();
                        done = true;
                    } else {
                        pair_job = Some(job); // 还在跑
                    }
                }
                Err(_) => {
                    done = true;
                }
            }
            if done {
                if !ok {
                    pair_line = Some("配网没成，再试一次。".into());
                }
                redraw = true;
            }
        }
        // ---- C7 装软件 job：auto 触发 + 收割（同 pair_job 形状——每拍
        // try_wait，绝不阻塞等；预算尽才 kill）。auto 门：net ok 且有
        // 未完成的 core 行，只点火一次（单飞槽与手动点击同引擎，C2 进程
        // 锁兜并发）；触发即撤配网行——装软件行接台讲当下的故事。----
        if install_job.is_none() && !install_auto && boot_state_has_internet() {
            install_auto = true;
            let rows = device_install_rows();
            if rows.iter().any(|r| r.tier == RowTier::Core && r.state != RowState::Ready) {
                eprintln!("aginx-term: net ok, core pending — auto sync");
                match spawn_install_job(&["sync"]) {
                    Some(job) => {
                        install_job = Some(job);
                        install_line = Some("正在装软件…".into());
                        pair_line = None;
                    }
                    None => install_line = Some("安装没起来，去清单面再试。".into()),
                }
            }
        }
        if let Some(mut job) = install_job.take() {
            let mut done = false;
            let mut ok = false;
            match job.child.try_wait() {
                Ok(Some(st)) => {
                    done = true;
                    ok = st.success();
                    if ok {
                        let mut out = String::new();
                        if let Some(mut r) = job.child.stdout.take() {
                            let _ = std::io::Read::read_to_string(&mut r, &mut out);
                        }
                        // 末行：sync 全程打多行进度，收官行是结论（pair_job
                        // 收首行——那边 apply 只出一行）
                        let line = out
                            .lines()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .map(|l| l.trim().to_string());
                        install_line = Some(
                            line.filter(|l| !l.is_empty())
                                .unwrap_or_else(|| "软件装好了。".into()),
                        );
                    } else {
                        eprintln!("aginx-term: pkg job exit {}", st.code().unwrap_or(-1));
                    }
                }
                Ok(None) => {
                    if job.since.elapsed() >= INSTALL_JOB_BUDGET {
                        eprintln!("aginx-term: pkg job budget over — kill");
                        let _ = job.child.kill();
                        let _ = job.child.wait();
                        done = true;
                    } else {
                        install_job = Some(job); // 还在拉
                    }
                }
                Err(_) => {
                    done = true;
                }
            }
            if done {
                if !ok {
                    install_line = Some("安装没成，再试一次。".into());
                }
                // 面开着 → 行集重读（stamps 刚落）
                if let Mode::Install(v) = &mut mode {
                    v.refresh();
                }
                redraw = true;
            }
        }
        // M47⑤t: park on {0..5} while the eye streams, full mask when it
        // stops — keyed on the FLAG, any mode (the voice daemon opens and
        // closes the eye with VolUp no matter which view is showing).
        // C6: term 自持会话同为流态（同为取景的分核收益方）。
        let eye_streaming = voice.doc.eye || term_eye.is_some();
        if eye_streaming != eye_parked {
            eye_parked = eye_streaming;
            set_eye_affinity(eye_parked);
        }

        // blink toggle — repaint only the terminal cursor's row
        if last_blink.elapsed() > Duration::from_millis(500) {
            blink_on = !blink_on;
            last_blink = Instant::now();
            if matches!(mode, Mode::Running(_)) && term.view_offset == 0 {
                term.mark_row(term.cursor_y);
            }
        }
        // v4⑤ breath tick — the prompt cursor's only animation: advance the
        // triangle phase, the next dispatch repaints the face. A result on
        // the panel holds the frame still (结果页不超时).
        if matches!(mode, Mode::Idle)
            && !voice.doc.result
            && last_breath.elapsed() >= Duration::from_millis(125)
        {
            last_breath = Instant::now();
            breath_tick = (breath_tick + 1) % 32;
            redraw = true;
        }
        // 警告注册表 poll (2026-09-09): /run/aginx-warn/ 非空 → idle 面中屏
        // 红警。变化才重画；结果页持帧期间不抢（结果页不超时）。
        if matches!(mode, Mode::Idle)
            && !voice.doc.result
            && last_warn_poll.elapsed() >= Duration::from_secs(2)
        {
            last_warn_poll = Instant::now();
            let now = read_warnings();
            if now != warns {
                warns = now;
                redraw = true;
            }
        }
        // C5 自持等网行 tick：任何 mode 都跑（轮询自带 5s 门；voice 复活
        // 边沿在任何面都要让位），行变了才重画。
        if selfnet.tick(voice.alive) {
            redraw = true;
        }
        // C7 清单面行刷新：5s 节拍重读真源（manifest/stamps 都是小文件），
        // 行集变了才重画（页码在 refresh 内夹回有效域）。
        if let Mode::Install(v) = &mut mode {
            if v.last_refresh.elapsed() >= INSTALL_REFRESH {
                v.last_refresh = Instant::now();
                if v.refresh() {
                    redraw = true;
                }
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
                Mode::Idle => {
                    // 开机剧情 v4 dispatch — prompt/result full-covers canvas.
                    // v4⑥: a cached live-panel frame goes straight into the
                    // back buffer (the eye raw path's direct sibling); only
                    // the path between face flag and first frame shows the
                    // prompt (cursor face).
                    if let Some(bm) = result_frame.as_ref() {
                        blit_result_direct(d.back_buf(), pitch, bm);
                        direct = true;
                    } else {
                        let level = if breath_tick <= 16 { breath_tick } else { 32 - breath_tick };
                        render_prompt(
                            &r,
                            buf,
                            &voice,
                            level,
                            &warns,
                            idle_status(voice.alive, &pair_line, &install_line, selfnet.line.as_deref())
                                .as_deref(),
                        );
                    }
                }
                Mode::Eye => {
                    // M47⑤f: a fresh raw viewfinder frame blits fused
                    // (565→888 + upscale) straight into the back buffer — no
                    // Bitmap, no canvas detour, no 10 MB copy. Everything
                    // else (取景中… / the JPEG fallback frame) renders into
                    // the canvas as before.
                    direct = voice.blit_eye_raw(d.back_buf(), pitch, w, h);
                    if !direct {
                        // eye() full-covers the canvas
                        r.eye(buf, &voice, &lg);
                    }
                }
                Mode::Install(v) => {
                    // install_list() full-covers the canvas
                    r.install_list(buf, v, install_line.as_deref(), boot_state_has_internet(), &lg);
                }
                Mode::Running(_) => {
                    r.terminal(buf, &term, area_top, scale, blink_on, lg.m);
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

    /// 开机剧情 v4⑤ golden: the idle face is pure near-black + the block
    /// cursor breathing at the top anchor (x=w/12=90, y=h*8/100=187, 25×40
    /// at scale 5) — same line the next transcript will start typing on.
    /// Breath levels 0/8/16 pin the shade ramp; the panel bottom renders
    /// nothing when idle.
    #[test]
    fn prompt_face_idle_breathes_at_top_anchor() {
        let font = font::font_init();
        let (w, h) = (1080usize, 2340usize); // D14-exempt: fixture panel geometry
        let r = Render { font: &font, w, h, pitch: w };
        let at = |pix: &[u32], x: usize, y: usize| pix[y * w + x];
        let mut pix = vec![0u32; w * h];
        r.prompt(&mut pix, "", 0, Some(16));
        assert_eq!(at(&pix, 5, 120), 0x00020503, "near-black BG");
        assert_eq!(at(&pix, 118, 76), 0x00020503, "camera punch-hole zone empty");
        assert_eq!(at(&pix, 95, 2241), 0x00020503, "nothing at the panel bottom when idle");
        assert_eq!(at(&pix, 273, 1355), 0x00020503, "old wordmark spot retired");
        // the one cursor block at the top anchor
        assert_eq!(at(&pix, 95, 190), 0x0000FF41, "idle cursor at full breath");
        assert_eq!(at(&pix, 114, 226), 0x0000FF41, "cursor block spans the cell");
        // whole-panel palette: exactly the two console colors
        for &p in &pix {
            assert!(
                matches!(p, 0x00020503 | 0x0000FF41),
                "stray color {p:#010x} — idle face is BG + Matrix green only"
            );
        }
        // breath ramp pins (see breath_shade); None hides the cursor
        let mut pix0 = vec![0u32; w * h];
        r.prompt(&mut pix0, "", 0, Some(0));
        assert_eq!(at(&pix0, 95, 190), 0x00005917, "breath floor 35%");
        let mut pix8 = vec![0u32; w * h];
        r.prompt(&mut pix8, "", 0, Some(8));
        assert_eq!(at(&pix8, 95, 190), 0x0000AB2C, "breath mid");
        let mut pixn = vec![0u32; w * h];
        r.prompt(&mut pixn, "", 0, None);
        assert_eq!(at(&pixn, 95, 190), 0x00020503, "None = no cursor");
    }

    /// 开机剧情 v4⑤: the transcript types left-aligned from the TOP anchor
    /// (below the front camera), wraps at the panel width (16 CJK hanzi
    /// per row at scale 5: 960 px avail / 60 px per hanzi), a solid cursor
    /// rides the reveal edge mid-type, then the breath cursor waits at the
    /// end of the last row.
    #[test]
    fn prompt_face_types_transcript_wrap() {
        let font = font::font_init();
        let (w, h) = (1080usize, 2340usize); // D14-exempt: fixture panel geometry
        let r = Render { font: &font, w, h, pitch: w };
        let at = |pix: &[u32], x: usize, y: usize| pix[y * w + x];
        let text = "把客厅摄像头画面调出来看看今天下午的日程安排"; // 22 hanzi → rows of 16+6

        // mid-type (10 of 22): ink in row 1 only (y=187..227), solid cursor
        // at the edge (x=90+10*60=690), row 2 (y=251) untouched, camera clear
        let mut pix = vec![0u32; w * h];
        r.prompt(&mut pix, text, 10, None);
        let mut ink = 0;
        for y in 187..227 {
            for x in 90..690 {
                if at(&pix, x, y) == 0x0000FF41 {
                    ink += 1;
                }
            }
        }
        assert!(ink > 1000, "row-1 CJK ink too sparse: {ink}");
        assert_eq!(at(&pix, 695, 197), 0x0000FF41, "solid cursor at reveal edge");
        assert_eq!(at(&pix, 118, 76), 0x00020503, "camera zone clear");
        assert_eq!(at(&pix, 95, 260), 0x00020503, "row 2 untouched mid-type");
        for &p in &pix {
            assert!(matches!(p, 0x00020503 | 0x0000FF41), "palette {p:#010x}");
        }

        // fully typed: row 2 carries the tail (6 hanzi, y=251..291), the
        // breath cursor waits at its end (x=90+6*60=450)
        let mut pix = vec![0u32; w * h];
        r.prompt(&mut pix, text, 22, Some(16));
        let mut ink2 = 0;
        for y in 251..291 {
            for x in 90..450 {
                if at(&pix, x, y) == 0x0000FF41 {
                    ink2 += 1;
                }
            }
        }
        assert!(ink2 > 400, "row-2 wrap ink too sparse: {ink2}");
        assert_eq!(at(&pix, 455, 257), 0x0000FF41, "post-type cursor at row-2 end");
        assert_eq!(at(&pix, 455, 265), 0x0000FF41, "cursor block is tall");
        // breath floor at the same spot; None hides it
        let mut pix0 = vec![0u32; w * h];
        r.prompt(&mut pix0, text, 22, Some(0));
        assert_eq!(at(&pix0, 455, 257), 0x00005917, "breath floor at text end");
        let mut pixn = vec![0u32; w * h];
        r.prompt(&mut pixn, text, 22, None);
        assert_eq!(at(&pixn, 455, 257), 0x00020503, "None = no cursor");
    }

    /// 警告注册表 (2026-09-09) golden: non-empty registry → red AginxOS
    /// wordmark at bootcard's anchor (centered, y=h*45/100=1053, scale 13)
    /// + one red line per warning file below it; the top-anchor cursor
    /// keeps breathing green — the two zones coexist, palette grows by
    /// exactly one color.
    #[test]
    fn warn_zone_red_wordmark_and_lines() {
        let font = font::font_init();
        let (w, h) = (1080usize, 2340usize); // D14-exempt: fixture panel geometry
        let r = Render { font: &font, w, h, pitch: w };
        let at = |pix: &[u32], x: usize, y: usize| pix[y * w + x];
        let mut pix = vec![0u32; w * h];
        r.prompt(&mut pix, "", 0, Some(16));
        r.warn(&mut pix, &["无网络 · 自动重连中".to_string()]);
        // wordmark band (y=1053..1157, x=267..813): red ink present
        let mut red = 0;
        for y in 1053..1157 {
            for x in 267..813 {
                if at(&pix, x, y) == 0x00FF3B30 {
                    red += 1;
                }
            }
        }
        assert!(red > 500, "wordmark red ink too sparse: {red}");
        assert_eq!(at(&pix, 95, 190), 0x0000FF41, "top-anchor cursor untouched");
        // one red warning line below (y=1217..1257, scale 5 = transcript cell)
        let mut line = 0;
        for y in 1217..1257 {
            for x in 0..w {
                if at(&pix, x, y) == 0x00FF3B30 {
                    line += 1;
                }
            }
        }
        assert!(line > 200, "warning line red ink too sparse: {line}");
        for &p in &pix {
            assert!(
                matches!(p, 0x00020503 | 0x0000FF41 | 0x00FF3B30),
                "palette {p:#010x} — BG + green + warn red only"
            );
        }
        // multiple warnings stack a row each (second line at y=1281..1321)
        let mut pix2 = vec![0u32; w * h];
        r.prompt(&mut pix2, "", 0, None);
        r.warn(
            &mut pix2,
            &["无网络 · 自动重连中".to_string(), "电量低".to_string()],
        );
        let mut line2 = 0;
        for y in 1281..1321 {
            for x in 0..w {
                if at(&pix2, x, y) == 0x00FF3B30 {
                    line2 += 1;
                }
            }
        }
        assert!(line2 > 100, "second warning stacks below the first: {line2}");
    }

    /// 警告注册表 reader: sorted by filename (= source tag), first line
    /// only, empty files / subdirs / missing dir read as nothing.
    #[test]
    fn warn_registry_reader_sorts_and_trims() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("aginx-warn-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("net"), "无网络 · 自动重连中\n").unwrap();
        fs::write(dir.join("battery"), "电量低\n第二行不该上屏\n").unwrap();
        fs::write(dir.join("empty"), "\n").unwrap();
        fs::create_dir_all(dir.join("asubdir")).unwrap();
        let warns = read_warnings_dir(&dir.to_string_lossy());
        assert_eq!(
            warns,
            vec!["电量低".to_string(), "无网络 · 自动重连中".to_string()],
            "battery < net lexicographically; second line trimmed; empty dropped"
        );
        fs::remove_dir_all(&dir).unwrap();
        assert!(read_warnings_dir("/nonexistent-aginx-warn").is_empty(), "missing dir = no warnings");
    }

    // ---- ①a 账本恢复 ----

    fn write_ledger(root: &std::path::Path, avatar: &str, turns: &[(&str, &str)]) {
        let d = root.join(avatar).join("sessions");
        std::fs::create_dir_all(&d).unwrap();
        let mut s = String::new();
        for (i, (q, a)) in turns.iter().enumerate() {
            s.push_str(&format!(
                concat!(
                    r#"{{"t":"request","avatar":"{av}","session":"main","text":"{q}","turn":{n}}}"#,
                    "\n",
                    r#"{{"t":"done","ok":true,"text":"{a}","error":null,"turn":{n}}}"#,
                    "\n"
                ),
                av = avatar,
                q = q,
                a = a,
                n = i + 1,
            ));
        }
        std::fs::write(d.join("main.jsonl"), s).unwrap();
    }

    /// host 测试统一入口：fixture 面板（真机首目标尺寸，纯数据不碰
    /// /etc——browser.rs 的 browser() 同款 D14 切法）。
    fn recover(root: &std::path::Path, line: Option<&str>) -> Option<String> {
        recover_result_html(root, line, 1080, 2340) // D14-exempt: fixture panel
    }

    #[test]
    fn fold_takes_last_nonempty_ok_done() {
        let root = std::env::temp_dir().join("aginx-term-test-fold");
        let _ = std::fs::remove_dir_all(&root);
        // 旧账（无 turn 字段）+ err 轮 + 空 done 轮 + 尾部截断的半行
        std::fs::create_dir_all(root.join("a/sessions")).unwrap();
        std::fs::write(
            root.join("a/sessions/main.jsonl"),
            concat!(
                r#"{"t":"request","avatar":"a","session":"main","text":"q1"}"#,
                "\n",
                r#"{"t":"done","ok":true,"text":"答1","error":null}"#,
                "\n",
                r#"{"t":"request","avatar":"a","session":"main","text":"q2"}"#,
                "\n",
                r#"{"t":"done","ok":false,"text":"","error":{"code":"brain","message":"x"}}"#,
                "\n",
                r#"{"t":"done","ok":true,"text":"","error":null}"#,
                "\n",
                r#"{"t":"requ"#, // 崩溃截断的半行
            ),
        )
        .unwrap();
        assert_eq!(
            fold_last_done_ok(&root.join("a/sessions/main.jsonl")).as_deref(),
            Some("答1")
        );
        assert_eq!(fold_last_done_ok(&root.join("nope.jsonl")), None);
    }

    #[test]
    fn recover_requires_line_match_and_wraps_raw() {
        let root = std::env::temp_dir().join("aginx-term-test-recover");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        write_ledger(&root, "小喜", &[("报状态", "电量 87% <正常> & 信号 3 格")]);
        let h = recover(&root, Some("电量 87% <正常> & 信号 3 格")).unwrap();
        // 三钉 + 磷光地板
        assert!(h.contains("min-height:2340px")); // D14-exempt: fixture panel height
        assert!(h.contains("width=1080")); // D14-exempt: fixture panel width
        assert!(h.contains("background:#000"));
        // 原文直进 pre：转义生效、不做 markdown 化（无 h1/li/blockquote）
        assert!(h.contains("电量 87% &lt;正常&gt; &amp; 信号 3 格"));
        assert!(!h.contains("<h1>") && !h.contains("<li>") && !h.contains("<blockquote>"));
        // 等值护栏：对不上（母体直答、账尾是旧结果）→ 放弃恢复
        assert!(recover(&root, Some("别的回复")).is_none());
        // line 缺席/空白 → 无法验证归属，放弃
        assert!(recover(&root, None).is_none());
        assert!(recover(&root, Some("  ")).is_none());
        // 空根（没化身）→ None
        let empty = std::env::temp_dir().join("aginx-term-test-recover-empty");
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        assert!(recover(&empty, Some("什么")).is_none());
    }

    #[test]
    fn recover_walks_ledgers_newest_first() {
        // 站立结果出自旧化身，但新化身账更晚（后台轮写账）：从新到旧
        // 依次试等值护栏，旧账命中也照常恢复——展示字节与 line 全同。
        let root = std::env::temp_dir().join("aginx-term-test-recover-order");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        write_ledger(&root, "旧", &[("问", "旧答")]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_ledger(&root, "新", &[("问", "新答")]);
        assert!(recover(&root, Some("新答")).is_some());
        assert!(recover(&root, Some("旧答")).is_some());
        assert!(recover(&root, Some("谁的都不是")).is_none());
    }

    #[test]
    fn recover_accepts_question_prefix_line() {
        // #283 问句常驻：face.line = 「问句\n回复」——账上 done 文本只对
        // 回复段；旧形状（纯回复行）也照常认。
        let root = std::env::temp_dir().join("aginx-term-test-recover-q");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        write_ledger(&root, "小喜", &[("现在几点", "17 点 24 分")]);
        let h = recover(&root, Some("现在几点\n17 点 24 分")).unwrap();
        assert!(h.contains("17 点 24 分"));
        // 问句含换行的罕见形状（三段）→ 尾段对不上整行 → 放弃（安全向）
        assert!(recover(&root, Some("多行\n问句\n17 点 24 分")).is_none());
    }

    /// C5 自持等网行：四门真值表（16 行全枚举——门多一个都不许漏）。
    /// 语义镜像 voice #282 的 boot_net_arm + 第四门 !voice.alive。
    #[test]
    fn selfnet_should_arm_four_gate_truth_table() {
        let cases: &[(f64, bool, bool, bool, bool)] = &[
            // (uptime, wifi_conf, internet_ok, voice_alive, want)
            (30.0, true, false, false, true),  // 蛋的常态：开机窗+已配对+没网+没 voice
            (179.9, true, false, false, true), // 窗边界内
            (180.0, true, false, false, true), // 边界含端（≤180）
            (180.1, true, false, false, false), // 窗外——问候是开机的事
            (30.0, false, false, false, false), // 未配对——路在配对面（C6），不在等网
            (30.0, true, true, false, false),  // 已通网——直接问候不布防（不占台）
            (30.0, true, false, true, false),  // voice 在——它自己等网，term 让位
            (30.0, true, true, true, false),
            (30.0, false, true, false, false),
            (30.0, false, false, true, false),
            (30.0, false, true, true, false),
            (180.1, false, false, false, false),
            (180.1, true, true, false, false),
            (180.1, true, false, true, false),
            (180.1, false, true, true, false),
            (f64::MAX, true, false, false, false), // host 测试读不到 /proc/uptime 的形状
        ];
        for &(up, wc, net, va, want) in cases {
            assert_eq!(
                selfnet_should_arm(up, wc, net, va),
                want,
                "uptime={up} wifi_conf={wc} net={net} voice={va}"
            );
        }
        // 行内容两态：等待行静态满显（状态不是台词，无打字机）
        let sn = SelfNet::arm();
        assert_eq!(sn.line.as_deref(), Some(SELFNET_WAITING));
        // 刚布防 5s 轮询门未到 + voice 不在 → 行不变
        let mut sn = SelfNet::arm();
        assert!(!sn.tick(false));
        assert_eq!(sn.line.as_deref(), Some(SELFNET_WAITING));
        assert_eq!(SelfNet::idle().line, None);
    }

    // ---- C6 扫码配网 ----

    /// C6 双目标条命中几何（fixture 面板钉住）：y∈[h-200, h-60)（下含上
    /// 不含），左半 Scan / 右半 Install；界外 None。
    #[test]
    fn pair_bar_hit_geometry() {
        let (w, h) = (1080usize, 2340usize); // D14-exempt: fixture panel geometry
        let is_scan = |x: usize, y: usize| matches!(pair_bar_hit(x, y, w, h), Some(PairBar::Scan));
        let is_install = |x: usize, y: usize| matches!(pair_bar_hit(x, y, w, h), Some(PairBar::Install));
        // 边界：y=h-200 进、y=h-60 出、y=h-201 出
        assert!(is_scan(100, h - 200), "top edge inclusive");
        assert!(pair_bar_hit(100, h - 201, w, h).is_none(), "above the bar");
        assert!(pair_bar_hit(100, h - 60, w, h).is_none(), "bottom edge exclusive");
        assert!(pair_bar_hit(100, h - 1, w, h).is_none(), "below the bar");
        // 左右对半：x<540 Scan，x≥540 Install（Render::pair_bar 各留 60/30 缩进，
        // 命中几何按对半分——视觉上 30px 缝隙容差）
        assert!(is_scan(0, 2200) && is_scan(539, 2200));
        assert!(is_install(540, 2200) && is_install(w - 1, 2200));
        assert!(is_scan(100, 2140) && is_install(1000, 2279));
        // 小面板不炸：饱和减法把条带顶到 y∈[0,h-60)——真实面板远大
        // （D14-exempt: fixture 下界语义）不会走到，这里只钉住它不 panic 不越界
        assert!(matches!(pair_bar_hit(10, 10, 100, 100), Some(_)));
    }

    /// C6 条的可见门真值表：voice 不在且未配对才画（四行全枚举）。
    #[test]
    fn pair_bar_visible_truth_table() {
        assert!(pair_bar_visible(false, false), "bare egg: the entry face");
        assert!(!pair_bar_visible(false, true), "paired — entry retires");
        assert!(!pair_bar_visible(true, false), "voice alive — its face");
        assert!(!pair_bar_visible(true, true));
    }

    /// C6 命中分诊：AGINXPAIR1 超集 / WIFI: 连网码 → Some（原样，喂
    /// aginx-pair apply 同一入口）；文本码 → None；首个可消费者胜。
    #[test]
    fn pick_pair_payload_tries_each_and_keeps_first() {
        // 全身份码（pair apply 测试同款 fixture——无真值，纯形状）
        let full = "AGINXPAIR1|Legrand AP|p4ss w0rd!|sk-1234567890abcdef1234567890abcdef|cf49973e|relay-secret-9f8e7d6c";
        assert_eq!(pick_pair_payload(&[full.to_string()]).as_deref(), Some(full));
        assert_eq!(
            pick_pair_payload(&["WIFI:T:WPA;S:home;P:secret;;".to_string()]).as_deref(),
            Some("WIFI:T:WPA;S:home;P:secret;;")
        );
        // 文本码不消费
        assert_eq!(pick_pair_payload(&["hello world".to_string(), "https://aginx.net".into()]), None);
        assert_eq!(pick_pair_payload(&[]), None);
        // 半截身份码（空段）不是码——解析器拒它
        assert_eq!(pick_pair_payload(&["AGINXPAIR1|ssid||key|gw|sec".to_string()]), None);
        // 首个可消费者胜：文本在前不挡，码仍被挑出
        let mixed = vec!["hello world".to_string(), full.to_string()];
        assert_eq!(pick_pair_payload(&mixed).as_deref(), Some(full));
        // 首个码胜：两个码都在，拿前一个
        let two = vec![full.to_string(), "WIFI:T:WPA;S:b;P:c;;".to_string()];
        assert_eq!(pick_pair_payload(&two).as_deref(), Some(full));
    }

    /// C6/C7 idle 面状态行所有权：voice 在 → None（它的面）；否则 配网行 >
    /// 装软件行 > selfnet 静态行 > None（四层让位链）。
    #[test]
    fn idle_status_ownership_chain() {
        let pair = Some("配网中…".to_string());
        let inst = Some("正在装软件…".to_string());
        let net: Option<&'static str> = Some(SELFNET_WAITING);
        // voice 活着：term 的一切状态行让位
        assert_eq!(idle_status(true, &pair, &inst, net), None);
        assert_eq!(idle_status(true, &None, &None, net), None);
        // 配网行压过装软件行（配网是当下的事，装软件等网）
        assert_eq!(idle_status(false, &pair, &inst, net).as_deref(), Some("配网中…"));
        // 无配网行 → 装软件行压过 selfnet（刚连上网的靴上 sync 是当下的
        // 事——auto 触发撤 pair_line 后就是这层在讲）
        assert_eq!(idle_status(false, &None, &inst, net).as_deref(), Some("正在装软件…"));
        // 都没有 → selfnet 行
        assert_eq!(idle_status(false, &None, &None, net), Some(SELFNET_WAITING.to_string()));
        // 全空 → 无人（transcript 打字机照旧）
        assert_eq!(idle_status(false, &None, &None, None), None);
    }

    // ---- C7 软件清单面 ----

    /// install_rows 四态真值（判等法照 pkg cmd_sync：stamp==sha 且真身在
    /// = up to date）：core+stamp+bin→Ready；core+bin+stamp 不等→Stale；
    /// core 无 bin→Missing（有 stamp 也不算——真身不在就是缺）；opt 无
    /// bin→OptReady；opt+bin+stamp→Ready。版本=第 5 列可选；注释/空/坏行
    /// （<3 字段）跳过不炸面；目录缺失不炸。
    #[test]
    fn install_rows_four_states() {
        let dir = std::env::temp_dir().join(format!("aginx-install-rows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stamps = dir.join("stamps");
        let bindir = dir.join("bin");
        std::fs::create_dir_all(&stamps).unwrap();
        std::fs::create_dir_all(&bindir).unwrap();
        // Ready：stamp 等且真身在
        std::fs::write(stamps.join("aginx-server"), "aaaa\n").unwrap();
        std::fs::write(bindir.join("aginx-server"), b"bin").unwrap();
        // Stale：真身在、stamp 不等
        std::fs::write(bindir.join("aginx-runtime"), b"bin").unwrap();
        std::fs::write(stamps.join("aginx-runtime"), "bbbb").unwrap();
        // Missing：stamp 在（还等）、真身不在——core 缺装
        std::fs::write(stamps.join("aginx-gateway"), "cccc").unwrap();
        // OptReady：opt 行无真身
        std::fs::write(stamps.join("grok"), "dddd").unwrap();
        // opt+真身+stamp 等 → Ready
        std::fs::write(stamps.join("codex"), "eeee").unwrap();
        std::fs::write(bindir.join("codex"), b"bin").unwrap();
        let m = concat!(
            "# 蛋的必装清单（sha 是占位假值）\n",
            "aginx-server https://example.invalid/1 aaaa core 0.9.0\n",
            "aginx-runtime https://example.invalid/2 rrrr core\n",
            "aginx-gateway https://example.invalid/3 cccc core 0.2.1\n",
            "grok https://example.invalid/4 dddd opt\n",
            "codex https://example.invalid/5 eeee opt 1.0\n",
            "\n",
            "badline\n",
        );
        let rows = install_rows(m, &stamps, &bindir);
        let find = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        assert_eq!(rows.len(), 5, "comment/blank/bad lines skipped");
        assert_eq!(find("aginx-server").state, RowState::Ready);
        assert_eq!(find("aginx-server").tier, RowTier::Core);
        assert_eq!(find("aginx-server").version.as_deref(), Some("0.9.0"));
        assert_eq!(find("aginx-runtime").state, RowState::Stale);
        assert_eq!(find("aginx-runtime").version, None, "missing col 5 = None");
        assert_eq!(find("aginx-gateway").state, RowState::Missing);
        assert_eq!(find("grok").state, RowState::OptReady);
        assert_eq!(find("grok").tier, RowTier::Opt);
        assert_eq!(find("codex").state, RowState::Ready);
        assert_eq!(find("codex").version.as_deref(), Some("1.0"));
        std::fs::remove_dir_all(&dir).unwrap();
        // 目录全缺不炸：core 无 bin → Missing（蛋首启的真形状）
        let rows = install_rows(
            "aginx-server https://x/aaaa core\n",
            std::path::Path::new("/nonexistent-aginx-stamps"),
            std::path::Path::new("/nonexistent-aginx-bin"),
        );
        assert_eq!(rows[0].state, RowState::Missing);
    }

    /// C7 翻页条命中几何：y∈[kb_panel_y-150, kb_panel_y-70)（下含上不含），
    /// 左格 x∈[60,300)=-1、右格 x∈[w-300, w-60)=+1，缝隙/边外 None。
    #[test]
    fn install_page_hit_geometry() {
        let (w, kb) = (1080usize, 1700usize); // D14-exempt: fixture geometry
        assert_eq!(install_page_hit(60, kb - 150, w, kb), Some(-1), "left cell, top edge inclusive");
        assert_eq!(install_page_hit(299, kb - 71, w, kb), Some(-1));
        assert_eq!(install_page_hit(300, kb - 100, w, kb), None, "gap between cells");
        assert_eq!(install_page_hit(w - 300, kb - 100, w, kb), Some(1));
        assert_eq!(install_page_hit(w - 61, kb - 71, w, kb), Some(1));
        assert_eq!(install_page_hit(w - 60, kb - 100, w, kb), None, "right margin out");
        assert_eq!(install_page_hit(100, kb - 151, w, kb), None, "above the strip");
        assert_eq!(install_page_hit(100, kb - 70, w, kb), None, "below the strip (bottom edge exclusive)");
        // 小面板不炸：饱和减法把条顶到 y0=0——只钉住不 panic
        let _ = install_page_hit(100, 10, 400, 100);
    }
}
