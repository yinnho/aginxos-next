// Face geometry + spawn-path constants. 批③ (09-10): the M16 launcher
// (registry tiles from /var/apps) is demolished — no Entry list, no
// builtins, no app registry read. What survives here: `Geom` (the row
// layout the C7 install face and the fullscreen eye box hang off —
// test-pinned to the committed [panel] profile) and the binary constants
// the debug/aging paths spawn. AGINX_TERM_START is the only way a pty
// session starts now.

pub const BIN_SH: &str = "/bin/sh";
pub const BIN_AGINX_REBOOT: &str = "/usr/bin/aginx-reboot";
pub const BIN_AGINX_PKG: &str = "/usr/bin/aginx-pkg";

/// Scale for AGINX_TERM_START debug spawns: the known phone-native
/// binaries keep the big 5x touch glyphs, everything else gets 3.
pub fn scale_for(bin: &str) -> usize {
    if bin == BIN_SH || bin == BIN_AGINX_REBOOT {
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
    /// `n` = rows on one page (the install face passes
    /// INSTALL_ROWS_PAGE — the launcher used to feed its entry count).
    pub fn new(w: usize, h: usize, kb_panel_y: usize, n: usize) -> Geom {
        let m = 90;
        let toolbar_h = 72;
        let avail_h = kb_panel_y - toolbar_h;
        let gap = 40;
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

    /// Toolbar regions while a toolbar face is up (running session or the
    /// install list face): BACK at the right. In a running session it
    /// kills the child; the install face uses it as its exit. Nothing
    /// else — the header stays clean.
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
