// Launcher (M16, docs/SYSTEM.md §12.3): app buttons come from the
// registry at /var/apps/<id>/app.toml — scanned at every launcher draw,
// so a new app appears by dropping a file (aginx-pkg or the app-registry
// seeder write them), no OS source change. Seven built-in tiles stay
// (+ picker / photos / install / sh / wifi setup / restart / power off),
// plus a thin toolbar strip above the content ([BACK] when a toolbar face
// is up). Program exit -> back to launcher. Touch regions are computed
// from the keyboard geometry so the layout scales with panel size.

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
    /// "INSTALL" tile (C7): opens the software-list face (Mode::Install)
    /// — the egg's tap-to-install entry, also reachable from the pairing
    /// bar's right cell.
    pub install: bool,
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
        install: false,
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
        install: false,
    }];
    // 面法 09-07: no VOICE tile — the voice dialog face is retired; the
    // eye enters from ANY mode on the face flag, and the resting screen is
    // Mode::Idle. This list is the debug launcher (reachable only via pty
    // exit / debug paths), not the product face.
    v.extend(
        [
            ("PHOTOS", "", &[][..], 5),
            ("INSTALL", "", &[][..], 5),
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
            // the photo viewer and the install face are pure aginx-term
            // state — always available; sh and aginx-reboot ship in the
            // base image; the wizard is a rootfs binary that always
            // exists post-M5
            avail: label == "PHOTOS"
                || label == "INSTALL"
                || bin == BIN_SH
                || bin == BIN_AGINX_REBOOT
                || std::path::Path::new(bin).is_file(),
            scale,
            picker: false,
            photos: label == "PHOTOS",
            install: label == "INSTALL",
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
    /// frame fills the panel — no toolbar, no title, no bottom strip; the
    /// close keys are physical (音量+ toggles, 音量下 closes). This is the
    /// ONE layout authority — voice() blits into it and poll_eye decodes
    /// against it. The aspect must stay what aginx-voice spawns cam-shot
    /// with (--aspect built from [panel] in the glue layer — no shared
    /// crate); the test below pins both readers to the same profile.
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

    /// Toolbar regions while a toolbar face is up (running app or a list
    /// face — picker/photos/install/launcher): BACK at the right. In a
    /// running app it kills the child; the list faces use it as their
    /// exit. Nothing else — the header stays clean.
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

/// cam-shot is spawned with --aspect built from [panel] (aginx-voice,
/// fullscreen viewfinder); term's eye box must match — asserted against
/// the same profile in tests, not by copying numbers into render.

#[derive(PartialEq)]
pub enum Toolbar {
    Back,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Committed first-target profile (D14-exempt: host reads the real
    /// file — schema and data stay in lockstep with what gets baked).
    fn profile_panel() -> (usize, usize) {
        let p = hwd::from_path(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../devices/redfin/device.toml" // D14-exempt: reads the real profile
        )))
        .unwrap();
        (p.panel.width as usize, p.panel.height as usize)
    }

    /// The fullscreen eye box must land exactly on the panel the profile
    /// declares — the same [panel] voice builds cam-shot's --aspect from
    /// (glue layer, no shared crate). One source of truth, two readers;
    /// this test pins term's side to it. If the profile's panel ever
    /// changes, both sides move together or this goes red.
    #[test]
    fn eye_box_is_the_whole_profile_panel() {
        let (w, h) = profile_panel();
        let kg = crate::kb::Kb::geom(w, h);
        let g = Geom::new(w, h, kg.panel_y, 5);
        let (x, y, ew, eh) = g.eye_box();
        assert_eq!((x, y), (0, 0));
        assert_eq!((ew, eh), (w, h));
    }
}
