// Photos — the M39 photo viewer state machine. SM7250 has no hardware
// JPEG decoder (camss cam_jpeg is encode-only, Venus has no JPEG — see
// HARDWARE.md M39), so decode is vendored libjpeg-turbo with NEON
// (../img), DCT-scaled straight to screen size. Photos live in
// /home/photos (state tar carries /home — they survive reflash);
// `ag cam-shot --jpeg-out /home/photos/shot.jpg` is the writer today.
//
// The viewer itself is two screens inside aginx-term's Mode::Photos, Picker
// style — no pty, no child process: a text list, then a full-frame blit.

/// Where photos live. Created on first scan so `ag cam-shot --jpeg-out`
/// has a dir to drop into even before the viewer ever opened.
pub const PHOTOS_DIR: &str = "/home/photos";

pub struct Photos {
    /// absolute paths, newest first (mtime sort — names from cam-shot are
    /// frame counters, not sortable strings)
    pub files: Vec<String>,
    pub sel: usize,
    pub img: Option<aginx_img::Bitmap>,
    /// false = list screen, true = full-screen image
    pub view: bool,
    /// last open error, shown in the list footer until the next attempt
    pub err: String,
}

impl Photos {
    pub fn scan() -> Photos {
        let _ = std::fs::create_dir_all(PHOTOS_DIR);
        let mut files: Vec<(std::time::SystemTime, String)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(PHOTOS_DIR) {
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_lowercase();
                if !name.ends_with(".jpg") && !name.ends_with(".jpeg") {
                    continue;
                }
                let mtime = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                files.push((mtime, p.to_string_lossy().into_owned()));
            }
        }
        files.sort_by(|a, b| b.0.cmp(&a.0));
        Photos {
            files: files.into_iter().map(|(_, p)| p).collect(),
            sel: 0,
            img: None,
            view: false,
            err: String::new(),
        }
    }

    /// Basenames without extension, for the list rows.
    pub fn names(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|p| {
                let s = p.rsplit('/').next().unwrap_or(p);
                s.trim_end_matches(".jpeg").trim_end_matches(".jpg").to_string()
            })
            .collect()
    }

    /// Decode `files[i]` into `max_w`×`max_h` and switch to the view
    /// screen. Returns false (staying on the list, `err` set) on any
    /// failure — a corrupt half-written capture must not kill the viewer.
    pub fn open(&mut self, i: usize, max_w: u32, max_h: u32) -> bool {
        if i >= self.files.len() {
            return false;
        }
        self.sel = i;
        self.img = None;
        self.err.clear();
        match std::fs::read(&self.files[i]) {
            Ok(bytes) => match aginx_img::decode_scaled(&bytes, max_w, max_h) {
                Some(b) => {
                    self.img = Some(b);
                    self.view = true;
                    true
                }
                None => {
                    self.err = "DECODE FAILED".into();
                    false
                }
            },
            Err(_) => {
                self.err = "READ FAILED".into();
                false
            }
        }
    }

    /// Next/previous with wraparound (no-op for <2 photos). The caller
    /// paints the LOADING frame before calling — decode blocks the loop.
    pub fn step(&mut self, delta: isize, max_w: u32, max_h: u32) {
        let n = self.files.len() as isize;
        if n == 0 {
            return;
        }
        let i = ((self.sel as isize + delta).rem_euclid(n)) as usize;
        self.open(i, max_w, max_h);
    }
}
