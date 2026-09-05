// Launcher (M16, docs/SYSTEM.md §12.3): app buttons come from the
// registry at /var/apps/<id>/app.toml — scanned at every launcher draw,
// so a new app appears by dropping a file (aginx-pkg or the app-registry
// seeder write them), no OS source change. Four system actions (sh /
// wifi setup / restart / power off) stay built in, plus a thin toolbar
// strip above the keyboard ([SH] always, [BACK] when an app runs).
// Program exit -> back to launcher. Touch regions are computed from the
// keyboard geometry so the layout scales with panel size.

use aginx_svc::{scan_apps, AppEntry, APPS_DIR};

pub const BIN_SH: &str = "/bin/sh";
pub const BIN_WIZARD: &str = "/usr/bin/aginx-net-wizard";
pub const BIN_AGINX_REBOOT: &str = "/usr/bin/aginx-reboot";
pub const BIN_AGINX_PKG: &str = "/usr/bin/aginx-pkg";

pub struct Entry {
    pub label: String,
    pub bin: String,
    /// argv[1..] for the binary (empty = bare exec). aginx-reboot's actions
    /// ("reboot" / "poweroff") are intercepted before any pty spawn.
    pub args: Vec<String>,
    pub avail: bool,
    /// Terminal glyph scale while this entry runs: phone-native UIs keep
    /// the big 5x touch glyphs, the PC-designed TUIs (codex/grok) need
    /// ~56 cols so they get 3.
    pub scale: usize,
    /// "+" tile: instead of spawning, opens the optional-package picker
    /// (M23 tiering — `aginx-pkg available` / `opt-in`). No pty involved.
    pub picker: bool,
    /// "PHOTOS" tile: opens the M39 photo viewer (Mode::Photos) instead
    /// of spawning. Same non-terminal pattern as the picker.
    pub photos: bool,
    /// "VOICE" tile: opens the M42a voice dialog face (Mode::Voice) —
    /// the product's primary input modality. Pure aginx-term state; content
    /// comes from polling /run/aginx-voice/face (written by the aginx-voice daemon).
    pub voice: bool,
}

/// Registry apps first (alphabetical by id), then the system actions.
pub fn entries() -> Vec<Entry> {
    let mut v: Vec<Entry> = scan_apps(APPS_DIR)
        .into_iter()
        .map(app_entry)
        .collect();
    v.extend(builtins());
    v
}

fn app_entry(a: AppEntry) -> Entry {
    Entry {
        label: a.name,
        bin: a.binary.clone(),
        args: a.args,
        avail: std::path::Path::new(&a.binary).is_file(),
        scale: a.scale,
        picker: false,
        photos: false,
        voice: false,
    }
}

fn builtins() -> Vec<Entry> {
    let mut v = vec![Entry {
        label: "+".into(),
        bin: BIN_AGINX_PKG.into(),
        args: vec![],
        // dimmed if the installer itself is missing
        avail: std::path::Path::new(BIN_AGINX_PKG).is_file(),
        scale: 5,
        picker: true,
        photos: false,
        voice: false,
    }];
    v.extend(
        [
            ("VOICE", "", &[][..], 5usize),
            ("PHOTOS", "", &[][..], 5),
            ("SH", BIN_SH, &[][..], 5),
            ("WIFI SETUP", BIN_WIZARD, &[][..], 5),
            ("RESTART", BIN_AGINX_REBOOT, &["reboot"][..], 5),
            ("POWER OFF", BIN_AGINX_REBOOT, &["poweroff"][..], 5),
        ]
        .into_iter()
        .map(|(label, bin, args, scale)| Entry {
            label: label.into(),
            bin: bin.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            // the voice face and the photo viewer are pure aginx-term state —
            // always available; sh and aginx-reboot ship in the base image; the
            // wizard is a rootfs binary that always exists post-M5
            avail: label == "VOICE"
                || label == "PHOTOS"
                || bin == BIN_SH
                || bin == BIN_AGINX_REBOOT
                || std::path::Path::new(bin).is_file(),
            scale,
            picker: false,
            photos: label == "PHOTOS",
            voice: label == "VOICE",
        })
        .collect::<Vec<_>>(),
    );
    v
}

/// Scale for non-launcher spawns (AGINX_TERM_START debug path, the first-boot
/// wizard): known phone-native binaries get 5, everything else 3.
pub fn scale_for(bin: &str) -> usize {
    if bin == BIN_SH || bin == BIN_WIZARD || bin == BIN_AGINX_REBOOT {
        5
    } else {
        3
    }
}

pub struct Geom {
    pub bx: usize,
    pub bw: usize,
    pub bh: usize,
    pub gap: usize,
    pub by0: usize,
    pub toolbar_h: usize,
    pub kb_panel_y: usize,
    pub m: usize, // global side margin (matches kb::KB_M)
    pub w: usize,
    /// full panel height — the M47⑤ fullscreen eye box needs it
    pub h: usize,
}

impl Geom {
    pub fn new(w: usize, h: usize, kb_panel_y: usize, n: usize) -> Geom {
        let m = 90;
        let toolbar_h = 72;
        let avail_h = kb_panel_y - toolbar_h;
        let gap = 40;
        // n buttons (launcher entries), evenly filling the space
        let bh = ((avail_h - 120 - gap * (n - 1)) / n).min(180);
        Geom {
            bx: m,
            bw: w - 2 * m,
            bh,
            gap,
            by0: toolbar_h + 70,
            toolbar_h,
            kb_panel_y,
            m: 28,
            w,
            h,
        }
    }

    /// M47⑤b eye viewfinder box (x, y, w, h) = the WHOLE panel. User
    /// receipt 2026-09-05 「界面要做成全屏」: while the eye is open the
    /// frame fills 1080×2340 — no toolbar, no title, no bottom strip; the
    /// close keys are physical (音量+ toggles, 音量下 closes). This is the
    /// ONE layout authority — voice() blits into it and poll_eye decodes
    /// against it. The aspect must stay what aginx-voice spawns cam-shot
    /// with (`--aspect 1080:2340`, hardcoded in the glue layer — no shared
    /// crate); VIEWFINDER_ASPECT + the test below pin the two sides.
    pub fn eye_box(&self) -> (usize, usize, usize, usize) {
        (0, 0, self.w, self.h)
    }

    pub fn button_at(&self, x: usize, y: usize, n: usize) -> Option<usize> {
        if x < self.bx || x >= self.bx + self.bw {
            return None;
        }
        for i in 0..n {
            let y0 = self.by0 + i * (self.bh + self.gap);
            if y >= y0 && y < y0 + self.bh {
                return Some(i);
            }
        }
        None
    }

    /// Toolbar regions while an app runs: BACK at the right (kill app,
    /// return to launcher). Nothing else — the header stays clean.
    pub fn toolbar_hit(&self, x: usize, y: usize, running: bool) -> Option<Toolbar> {
        if y >= self.toolbar_h {
            return None;
        }
        if running && x >= self.w - self.m - 170 {
            Some(Toolbar::Back)
        } else {
            None
        }
    }
}

/// cam-shot is spawned with --aspect 1080:2340 (aginx-voice, fullscreen
/// viewfinder); term's eye box must match — assert via Geom in tests, not
/// by copying numbers into render.
pub const VIEWFINDER_ASPECT: f64 = 1080.0 / 2340.0;

#[derive(PartialEq)]
pub enum Toolbar {
    Back,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real device numbers (redfin 1080x2340): the fullscreen eye box must
    /// land exactly on the aspect cam-shot is spawned with. If the panel
    /// geometry ever changes, this test goes red and voice's hardcoded
    /// `--aspect 1080:2340` must move with it.
    #[test]
    fn eye_box_matches_viewfinder_aspect() {
        let g = Geom::new(1080, 2340, 1748, 5);
        let (x, y, w, h) = g.eye_box();
        assert_eq!((x, y), (0, 0));
        assert_eq!((w, h), (1080, 2340));
        let aspect = w as f64 / h as f64;
        assert!(
            (aspect - VIEWFINDER_ASPECT).abs() < 1e-9,
            "eye box aspect {aspect} != VIEWFINDER_ASPECT {VIEWFINDER_ASPECT}"
        );
    }
}
