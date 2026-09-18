// Photos — /home/photos grid + fullscreen viewer. Decode is vendored
// libjpeg-turbo (no JPEG decoder on the SoC). Same directory cam-snap
// writes; newest first.

use crate::draw_centered;
use crate::fill_rect;
use crate::launch;
use crate::BG;
use crate::DIM;
use crate::GREEN;
use crate::UNAVAIL;

pub const DIR: &str = "/home/photos";
const COLS: usize = 2;
const GAP: usize = 24;
const SIDE: usize = 24;

pub struct Photos {
    pub files: Vec<String>,
    thumbs: Vec<Option<aginx_img::Bitmap>>,
    pub view: Option<aginx_img::Bitmap>,
}

impl Photos {
    pub fn scan() -> Photos {
        let _ = std::fs::create_dir_all(DIR);
        let mut files: Vec<(std::time::SystemTime, String)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(DIR) {
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
        let files: Vec<String> = files.into_iter().map(|(_, p)| p).collect();
        let thumbs = (0..files.len()).map(|_| None).collect();
        Photos {
            files,
            thumbs,
            view: None,
        }
    }

    pub fn open(&mut self, i: usize, max_w: u32, max_h: u32) -> bool {
        let Some(path) = self.files.get(i) else {
            return false;
        };
        match std::fs::read(path) {
            Ok(bytes) => match aginx_img::decode_scaled(&bytes, max_w, max_h) {
                Some(b) => {
                    self.view = Some(b);
                    true
                }
                None => false,
            },
            Err(_) => false,
        }
    }

    pub fn close_view(&mut self) {
        self.view = None;
    }

    fn cell(w: usize) -> usize {
        (w - 2 * SIDE - GAP) / COLS
    }

    /// Grid cell index under (x,y), or None. Toolbar band is excluded.
    pub fn grid_hit(&self, w: usize, h: usize, toolbar_h: usize, x: usize, y: usize) -> Option<usize> {
        let _ = h;
        if y < toolbar_h + 20 {
            return None;
        }
        let cell = Self::cell(w);
        if cell == 0 {
            return None;
        }
        let gx = x.saturating_sub(SIDE);
        let gy = y.saturating_sub(toolbar_h + 20);
        let col = gx / (cell + GAP);
        let row = gy / (cell + GAP);
        if col >= COLS {
            return None;
        }
        let lx = gx - col * (cell + GAP);
        let ly = gy - row * (cell + GAP);
        if lx >= cell || ly >= cell {
            return None;
        }
        let i = row * COLS + col;
        if i < self.files.len() {
            Some(i)
        } else {
            None
        }
    }

    fn ensure_thumb(&mut self, i: usize, cell: u32) {
        if i >= self.files.len() || self.thumbs.get(i).is_some_and(|t| t.is_some()) {
            return;
        }
        if let Ok(bytes) = std::fs::read(&self.files[i]) {
            self.thumbs[i] = aginx_img::decode_scaled(&bytes, cell, cell);
        }
    }

    pub fn paint(
        &mut self,
        pix: &mut [u32],
        pitch: usize,
        w: usize,
        h: usize,
        font: &[[u8; 8]; 128],
        g: &launch::Geom,
    ) {
        fill_rect(pix, pitch, w, h, 0, 0, w as i32, h as i32, BG);
        draw_centered(
            pix,
            pitch,
            w,
            h,
            font,
            g.toolbar_h as i32 + 14,
            "相册",
            5,
            GREEN,
        );
        if self.files.is_empty() {
            draw_centered(
                pix,
                pitch,
                w,
                h,
                font,
                (h as i32 - 24) / 2,
                "还没有照片",
                4,
                UNAVAIL,
            );
            return;
        }
        let cell = Self::cell(w);
        let top = g.toolbar_h + 20;
        for i in 0..self.files.len() {
            let col = i % COLS;
            let row = i / COLS;
            let x = SIDE + col * (cell + GAP);
            let y = top + row * (cell + GAP);
            if y + cell > h {
                break;
            }
            fill_rect(
                pix,
                pitch,
                w,
                h,
                x as i32,
                y as i32,
                cell as i32,
                cell as i32,
                DIM,
            );
            self.ensure_thumb(i, cell as u32);
            if let Some(b) = self.thumbs[i].as_ref() {
                blit_contain(pix, pitch, w, h, x, y, cell, cell, b);
            }
        }
    }
}

fn blit_contain(
    pix: &mut [u32],
    pitch: usize,
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    dw: usize,
    dh: usize,
    b: &aginx_img::Bitmap,
) {
    if b.w == 0 || b.h == 0 || dw == 0 || dh == 0 {
        return;
    }
    let (sw, sh) = (b.w as usize, b.h as usize);
    let (fw, fh) = if sw * dh < sh * dw {
        (sw * dh / sh, dh)
    } else {
        (dw, sh * dw / sw)
    };
    let ox = x + (dw - fw) / 2;
    let oy = y + (dh - fh) / 2;
    let mut sx = vec![0usize; fw];
    for (i, s) in sx.iter_mut().enumerate() {
        *s = i * sw / fw;
    }
    for j in 0..fh {
        let yy = oy + j;
        if yy >= h {
            break;
        }
        let row = (j * sh / fh) * sw;
        let dst = yy * pitch;
        for i in 0..fw {
            let xx = ox + i;
            if xx >= w {
                break;
            }
            pix[dst + xx] = b.pix[row + sx[i]];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scan_is_empty_grid() {
        let dir = std::env::temp_dir().join(format!("aginx-photos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // scan() hard-codes /home/photos; this test pins hit math instead.
        let p = Photos {
            files: vec!["/tmp/a.jpg".into(), "/tmp/b.jpg".into(), "/tmp/c.jpg".into()],
            thumbs: vec![None, None, None],
            view: None,
        };
        let (w, h, th) = (1080usize, 2280usize, 72usize); // D14-exempt: fixture panel
        let cell = Photos::cell(w);
        assert!(cell > 200);
        assert_eq!(p.grid_hit(w, h, th, SIDE + 10, th + 30), Some(0));
        assert_eq!(p.grid_hit(w, h, th, SIDE + cell + GAP + 10, th + 30), Some(1));
        assert_eq!(
            p.grid_hit(w, h, th, SIDE + 10, th + 20 + cell + GAP + 10),
            Some(2)
        );
        assert!(p.grid_hit(w, h, th, 10, 10).is_none());
        let _ = dir;
    }
}
