// vte -> cell grid. One Parser drives a Sink that owns the visible grid plus
// a flat scrollback ring (SGR 3 to recall). Colors: green phosphor default,
// ANSI white -> bright, everything else dims to green (phosphor terminal,
// user-fixed palette: black bg, green/white text).

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Normal,
    Bright,
    Inverse,
}

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}
/// Trailing half of a wide (2-cell) glyph — the leading cell holds the real
/// char; the renderer skips this sentinel and draws across both cells.
pub const WIDE_TAIL: char = '\0';
impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            style: Style::Normal,
        }
    }
}

pub struct Term {
    pub cols: usize,
    pub rows: usize,
    grid: Vec<Cell>,
    back: Vec<Vec<Cell>>,
    back_top: usize, // next write slot in the ring
    back_len: usize, // lines stored
    back_cap: usize,
    pub cursor_x: usize,
    pub cursor_y: usize,
    saved: (usize, usize),
    pub cursor_visible: bool,
    /// DECCKM (?1): child wants SS3 arrows (ESC O A), not CSI (ESC [ A).
    /// input::encode reads this — arrow encoding is the terminal's call.
    pub app_cursor: bool,
    style: Style,
    scroll_top: usize,
    scroll_bot: usize, // exclusive
    pub view_offset: usize, // lines scrolled back (0 = live)
    pub dirty: bool,
    row_dirty: Vec<bool>,
    wrap_pending: bool,
}

impl Term {
    pub fn new(cols: usize, rows: usize) -> Term {
        Term {
            cols,
            rows,
            grid: vec![Cell::default(); cols * rows],
            back: Vec::new(),
            back_top: 0,
            back_len: 0,
            back_cap: 400,
            cursor_x: 0,
            cursor_y: 0,
            saved: (0, 0),
            cursor_visible: true,
            app_cursor: false,
            style: Style::Normal,
            scroll_top: 0,
            scroll_bot: rows,
            view_offset: 0,
            dirty: true,
            row_dirty: vec![true; rows],
            wrap_pending: false,
        }
    }

    pub fn row_dirty(&self) -> &[bool] {
        &self.row_dirty
    }

    pub fn clear_row_dirty(&mut self) {
        for r in &mut self.row_dirty {
            *r = false;
        }
    }

    pub fn mark_all(&mut self) {
        for r in &mut self.row_dirty {
            *r = true;
        }
        self.dirty = true;
    }

    pub fn mark_row(&mut self, y: usize) {
        if y < self.rows {
            self.row_dirty[y] = true;
            self.dirty = true;
        }
    }

    /// Resize the visible grid when the keyboard shows/hides. Growing pads
    /// with blanks at the bottom; shrinking shifts the top lines into
    /// scrollback (capped so the cursor row never leaves the screen).
    pub fn resize_rows(&mut self, new_rows: usize) {
        if new_rows == self.rows {
            return;
        }
        if new_rows < self.rows {
            let shift = (self.rows - new_rows).min(self.cursor_y);
            for i in 0..shift {
                let line: Vec<Cell> = self.line(i).to_vec();
                self.push_scrollback(&line);
            }
            for y in 0..self.rows - shift {
                for x in 0..self.cols {
                    self.grid[y * self.cols + x] = self.grid[(y + shift) * self.cols + x];
                }
            }
            self.grid.truncate(self.cols * new_rows);
            self.cursor_y -= shift;
        } else {
            self.grid.resize(self.cols * new_rows, Cell::default());
        }
        self.rows = new_rows;
        self.scroll_top = 0;
        self.scroll_bot = new_rows;
        self.view_offset = 0;
        self.row_dirty = vec![true; new_rows];
        self.dirty = true;
    }

    /// New output while scrolled back: jump to the live edge.
    pub fn jump_live(&mut self) {
        if self.view_offset > 0 {
            self.view_offset = 0;
            self.mark_all();
        }
    }

    fn line(&self, y: usize) -> &[Cell] {
        &self.grid[y * self.cols..(y + 1) * self.cols]
    }

    fn push_scrollback(&mut self, line: &[Cell]) {
        let v = line.to_vec();
        if self.back_len < self.back_cap {
            self.back.push(v);
            self.back_len += 1;
            self.back_top = self.back_len % self.back_cap;
        } else {
            self.back[self.back_top] = v;
            self.back_top = (self.back_top + 1) % self.back_cap;
        }
    }

    /// Historical line `i` where 0 = oldest stored.
    fn back_line(&self, i: usize) -> &[Cell] {
        let idx = (self.back_top + self.back_cap + i - self.back_len) % self.back_cap;
        &self.back[idx]
    }

    pub fn scroll_view(&mut self, delta: isize) {
        let max = self.back_len as isize;
        let v = (self.view_offset as isize + delta).clamp(0, max);
        self.view_offset = v as usize;
        self.mark_all();
    }

    /// Rendered row: scrollback when view_offset > 0, else live grid.
    pub fn render_line(&self, y: usize) -> Vec<Cell> {
        if self.view_offset == 0 {
            return self.line(y).to_vec();
        }
        let shift = self.view_offset as isize - y as isize;
        if shift > 0 && (shift as usize) <= self.back_len {
            self.back_line(self.back_len - shift as usize).to_vec()
        } else {
            let gy = (y as isize - self.view_offset as isize).max(0) as usize;
            self.line(gy.min(self.rows - 1)).to_vec()
        }
    }

    fn blank_line(&mut self, y: usize) {
        for x in 0..self.cols {
            self.grid[y * self.cols + x] = Cell::default();
        }
        self.row_dirty[y] = true;
        self.dirty = true;
    }

    fn scroll_up_region(&mut self) {
        if self.scroll_top == 0 && self.scroll_bot == self.rows {
            let first: Vec<Cell> = self.line(0).to_vec();
            self.push_scrollback(&first);
        }
        for y in self.scroll_top..self.scroll_bot - 1 {
            for x in 0..self.cols {
                self.grid[y * self.cols + x] = self.grid[(y + 1) * self.cols + x];
            }
        }
        self.blank_line(self.scroll_bot - 1);
        self.mark_all();
    }

    fn scroll_down_region(&mut self) {
        for y in (self.scroll_top + 1..self.scroll_bot).rev() {
            for x in 0..self.cols {
                self.grid[y * self.cols + x] = self.grid[(y - 1) * self.cols + x];
            }
        }
        self.blank_line(self.scroll_top);
        self.mark_all();
    }

    fn newline(&mut self) {
        self.row_dirty[self.cursor_y] = true;
        if self.cursor_y + 1 == self.scroll_bot {
            self.scroll_up_region();
        } else if self.cursor_y + 1 < self.rows {
            self.cursor_y += 1;
            self.row_dirty[self.cursor_y] = true;
        }
        self.dirty = true;
    }

    fn put(&mut self, ch: char) {
        if self.wrap_pending {
            self.wrap_pending = false;
            self.cursor_x = 0;
            self.newline();
        }
        let st = self.style;
        // Wide chars (CJK etc.) occupy two cells: the leading cell holds the
        // char, the trailing a WIDE_TAIL sentinel the renderer skips. There
        // is no room at the last column — wrap before placing.
        let wide = crate::cjk::char_width(ch) == 2;
        if wide && self.cursor_x + 2 > self.cols {
            self.grid[self.cursor_y * self.cols + self.cursor_x] = Cell { ch: ' ', style: st };
            self.cursor_x = 0;
            self.newline();
        }
        // Overwriting half of an existing wide pair orphans the other half —
        // blank it first so no torn glyph survives.
        if self.cursor_x > 0 {
            let prev = self.grid[self.cursor_y * self.cols + self.cursor_x - 1].ch;
            if crate::cjk::char_width(prev) == 2 {
                self.grid[self.cursor_y * self.cols + self.cursor_x - 1] = Cell { ch: ' ', style: st };
            }
        }
        if crate::cjk::char_width(self.grid[self.cursor_y * self.cols + self.cursor_x].ch) == 2
            && self.cursor_x + 1 < self.cols
        {
            self.grid[self.cursor_y * self.cols + self.cursor_x + 1] = Cell { ch: ' ', style: st };
        }
        self.grid[self.cursor_y * self.cols + self.cursor_x] = Cell { ch, style: st };
        self.row_dirty[self.cursor_y] = true;
        if wide {
            if self.cursor_x + 1 < self.cols {
                self.grid[self.cursor_y * self.cols + self.cursor_x + 1] = Cell { ch: WIDE_TAIL, style: st };
            }
            if self.cursor_x + 2 >= self.cols {
                self.wrap_pending = true;
            } else {
                self.cursor_x += 2;
            }
        } else if self.cursor_x + 1 >= self.cols {
            self.wrap_pending = true;
        } else {
            self.cursor_x += 1;
        }
    }

    fn csi(&mut self, params: &vte::Params, intermediates: &[u8], action: char) {
        let old_y = self.cursor_y;
        let p = |i: usize, d: usize| -> usize {
            params
                .iter()
                .nth(i)
                .and_then(|s| s.first().copied())
                .filter(|&v| v != 0)
                .map(|v| v as usize)
                .unwrap_or(d)
        };
        match action {
            'A' => self.cursor_y = self.cursor_y.saturating_sub(p(0, 1)),
            'B' | 'e' => {
                self.cursor_y = (self.cursor_y + p(0, 1)).min(self.rows - 1);
            }
            'C' | 'a' => {
                self.cursor_x = (self.cursor_x + p(0, 1)).min(self.cols - 1);
            }
            'D' => self.cursor_x = self.cursor_x.saturating_sub(p(0, 1)),
            'E' => {
                self.cursor_y = (self.cursor_y + p(0, 1)).min(self.rows - 1);
                self.cursor_x = 0;
            }
            'F' => {
                self.cursor_y = self.cursor_y.saturating_sub(p(0, 1));
                self.cursor_x = 0;
            }
            'G' | '`' => self.cursor_x = (p(0, 1) - 1).min(self.cols - 1),
            'H' | 'f' => {
                self.cursor_y = (p(0, 1) - 1).min(self.rows - 1);
                self.cursor_x = (p(1, 1) - 1).min(self.cols - 1);
            }
            'd' => self.cursor_y = (p(0, 1) - 1).min(self.rows - 1),
            'J' => match p(0, 0) {
                0 => {
                    for x in self.cursor_x..self.cols {
                        self.grid[self.cursor_y * self.cols + x] = Cell::default();
                    }
                    for y in self.cursor_y + 1..self.rows {
                        self.blank_line(y);
                    }
                }
                1 => {
                    for y in 0..self.cursor_y {
                        self.blank_line(y);
                    }
                    for x in 0..=self.cursor_x {
                        self.grid[self.cursor_y * self.cols + x] = Cell::default();
                    }
                }
                2 | 3 => {
                    for y in 0..self.rows {
                        self.blank_line(y);
                    }
                }
                _ => {}
            },
            'K' => match p(0, 0) {
                0 => {
                    for x in self.cursor_x..self.cols {
                        self.grid[self.cursor_y * self.cols + x] = Cell::default();
                    }
                }
                1 => {
                    for x in 0..=self.cursor_x {
                        self.grid[self.cursor_y * self.cols + x] = Cell::default();
                    }
                }
                2 => self.blank_line(self.cursor_y),
                _ => {}
            },
            'L' => {
                let n = p(0, 1).min(self.scroll_bot - self.cursor_y);
                for _ in 0..n {
                    for y in (self.cursor_y + 1..self.scroll_bot).rev() {
                        for x in 0..self.cols {
                            self.grid[y * self.cols + x] = self.grid[(y - 1) * self.cols + x];
                        }
                    }
                    self.blank_line(self.cursor_y);
                }
            }
            'M' => {
                let n = p(0, 1).min(self.scroll_bot - self.cursor_y);
                for _ in 0..n {
                    for y in self.cursor_y..self.scroll_bot - 1 {
                        for x in 0..self.cols {
                            self.grid[y * self.cols + x] = self.grid[(y + 1) * self.cols + x];
                        }
                    }
                    self.blank_line(self.scroll_bot - 1);
                }
            }
            'P' => {
                let n = p(0, 1).min(self.cols - self.cursor_x);
                for x in self.cursor_x..self.cols {
                    self.grid[self.cursor_y * self.cols + x] = if x + n < self.cols {
                        self.grid[self.cursor_y * self.cols + x + n]
                    } else {
                        Cell::default()
                    };
                }
            }
            'S' => {
                for _ in 0..p(0, 1) {
                    self.scroll_up_region();
                }
            }
            'T' => {
                for _ in 0..p(0, 1) {
                    self.scroll_down_region();
                }
            }
            'X' => {
                let n = p(0, 1).min(self.cols - self.cursor_x);
                for x in self.cursor_x..self.cursor_x + n {
                    self.grid[self.cursor_y * self.cols + x] = Cell::default();
                }
            }
            'r' => {
                self.scroll_top = p(0, 1) - 1;
                self.scroll_bot = p(1, self.rows).min(self.rows);
                self.cursor_x = 0;
                self.cursor_y = 0;
            }
            's' => self.saved = (self.cursor_x, self.cursor_y),
            'u' => {
                self.cursor_x = self.saved.0;
                self.cursor_y = self.saved.1;
            }
            'm' => {
                // SGR: palette is fixed (green phosphor) — only intensity
                // and inverse survive.
                let mut any = false;
                for sub in params.iter() {
                    for &v in sub {
                        any = true;
                        match v {
                            0 => self.style = Style::Normal,
                            1 => self.style = Style::Bright,
                            7 => self.style = Style::Inverse,
                            27 => {
                                if self.style == Style::Inverse {
                                    self.style = Style::Normal;
                                }
                            }
                            39 | 49 => self.style = Style::Normal,
                            _ => {}
                        }
                    }
                }
                if !any {
                    self.style = Style::Normal;
                }
            }
            'h' | 'l' => {
                // Private modes (vte collects the '?' into intermediates):
                // ?1 application cursor keys, ?25 cursor visibility.
                // Non-private SM/RM carries nothing we track.
                if intermediates == [b'?'] {
                    for sub in params.iter() {
                        for &v in sub {
                            match v {
                                1 => self.app_cursor = action == 'h',
                                25 => self.cursor_visible = action == 'h',
                                _ => {}
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        match action {
            'J' | 'K' | 'L' | 'M' | 'P' | 'S' | 'T' | 'X' | 'r' => self.mark_all(),
            _ => {
                self.mark_row(old_y);
                self.mark_row(self.cursor_y);
            }
        }
        self.dirty = true;
    }
}

impl vte::Perform for Term {
    fn print(&mut self, ch: char) {
        self.put(ch);
        self.dirty = true;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => {
                self.newline();
                self.wrap_pending = false;
            }
            b'\r' => {
                self.cursor_x = 0;
                self.wrap_pending = false;
            }
            0x08 => {
                if self.wrap_pending {
                    self.wrap_pending = false;
                } else {
                    self.cursor_x = self.cursor_x.saturating_sub(1);
                }
            }
            b'\t' => {
                let next = (self.cursor_x / 8 + 1) * 8;
                self.cursor_x = next.min(self.cols - 1);
                self.wrap_pending = false;
            }
            _ => {}
        }
        let y = self.cursor_y;
        self.mark_row(y); // cursor may have moved within the row
    }

    fn hook(&mut self, _p: &vte::Params, _i: &[u8], _ignore: bool, _a: char) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _p: &[&[u8]], _bell: bool) {}

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        self.csi(params, intermediates, action);
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => self.saved = (self.cursor_x, self.cursor_y),
            b'8' => {
                self.cursor_x = self.saved.0;
                self.cursor_y = self.saved.1;
            }
            b'D' => self.newline(),
            b'E' => {
                self.cursor_x = 0;
                self.newline();
            }
            b'M' => {
                if self.cursor_y == self.scroll_top {
                    self.scroll_down_region();
                } else {
                    self.cursor_y = self.cursor_y.saturating_sub(1);
                }
            }
            b'c' => {
                for y in 0..self.rows {
                    self.blank_line(y);
                }
                self.cursor_x = 0;
                self.cursor_y = 0;
                self.style = Style::Normal;
                self.scroll_top = 0;
                self.scroll_bot = self.rows;
                self.app_cursor = false;
            }
            _ => {}
        }
        let y = self.cursor_y;
        self.mark_row(y);
        self.dirty = true;
    }
}
