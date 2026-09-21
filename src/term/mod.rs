// e gui emo minus terminal, a undercooked taco wrap for em tui apps with mouse, some symbol and scroll support
//
// like basically most of the code in here is either recycled from my eguiemo-minus project i have yet to release, stolen from stackoverflow and in some cases repaired by generative ai (where ive added comments)
// i didnt feel like writing an entire terminal emulator from scratch yall guys please chill

pub mod input;
pub mod view;

use eframe::egui::Color32;
use vte::{Params, Perform};

/// the two colors the config picks, everything else is the ansi palette
#[derive(Clone, Copy)]
pub struct Palette {
    pub fg: Color32,
    pub bg: Color32,
}

// terminally ill colors 

#[derive(Clone, Copy, PartialEq)]
enum TermColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}


// this fn wasnt made by me i think i let ai handle this or i stole it from somewhere idk just so yk
fn ansi_color(idx: u8) -> Color32 {
    match idx {
        0 => Color32::from_rgb(0, 0, 0),
        1 => Color32::from_rgb(170, 0, 0),
        2 => Color32::from_rgb(0, 170, 0),
        3 => Color32::from_rgb(170, 85, 0),
        4 => Color32::from_rgb(0, 0, 170),
        5 => Color32::from_rgb(170, 0, 170),
        6 => Color32::from_rgb(0, 170, 170),
        7 => Color32::from_rgb(170, 170, 170),
        8 => Color32::from_rgb(85, 85, 85),
        9 => Color32::from_rgb(255, 85, 85),
        10 => Color32::from_rgb(85, 255, 85),
        11 => Color32::from_rgb(255, 255, 85),
        12 => Color32::from_rgb(85, 85, 255),
        13 => Color32::from_rgb(255, 85, 255),
        14 => Color32::from_rgb(85, 255, 255),
        15 => Color32::from_rgb(255, 255, 255),
        // 6x6x6 color cube
        16..=231 => {
            let idx = (idx - 16) as u16;
            let ri = idx / 36;
            let gi = (idx % 36) / 6;
            let bi = idx % 6;
            let v = |i: u16| -> u8 {
                if i == 0 {
                    0
                } else {
                    55 + i as u8 * 40
                }
            };
            Color32::from_rgb(v(ri), v(gi), v(bi))
        }
        // grayscale ramp
        232..=255 => {
            let g = 8 + (idx - 232) * 10;
            Color32::from_rgb(g, g, g)
        }
    }
}

fn resolve_color(c: TermColor, is_fg: bool, pal: &Palette) -> Color32 {
    match c {
        TermColor::Default => {
            if is_fg {
                pal.fg
            } else {
                pal.bg
            }
        }
        TermColor::Indexed(i) => ansi_color(i),
        TermColor::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}

// terminal incell stuff

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    fg: TermColor,
    bg: TermColor,
    bold: bool,
    reverse: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: TermColor::Default,
            bg: TermColor::Default,
            bold: false,
            reverse: false,
        }
    }
}

impl Cell {
    pub fn resolved_fg(&self, pal: &Palette) -> Color32 {
        if self.reverse {
            resolve_color(self.bg, false, pal)
        } else {
            let c = resolve_color(self.fg, true, pal);
            if self.bold {
                // brighten bold text a bit
                let [r, g, b, a] = c.to_array();
                Color32::from_rgba_premultiplied(
                    r.saturating_add(40),
                    g.saturating_add(40),
                    b.saturating_add(40),
                    a,
                )
            } else {
                c
            }
        }
    }

    pub fn resolved_bg(&self, pal: &Palette) -> Color32 {
        if self.reverse {
            resolve_color(self.fg, true, pal)
        } else {
            resolve_color(self.bg, false, pal)
        }
    }
}

// terminal grid 
// contains quiet a few ai solved bugfixes they seem ... fineish... to me and work
pub struct TermGrid {
    pub cells: Vec<Vec<Cell>>,
    pub rows: usize,
    pub cols: usize,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    scroll_top: usize,
    scroll_bottom: usize,

    // current drawing attributes
    attr_fg: TermColor,
    attr_bg: TermColor,
    attr_bold: bool,
    attr_reverse: bool,

    // saved cursor
    saved_cursor: Option<(usize, usize)>,

    // alternate screen buffer
    alt_saved: Option<(Vec<Vec<Cell>>, usize, usize)>,

    // mouse tracking modes not sure if i need all but whatever, been partially corrected by ai at somepoint i think
    mouse_normal: bool, // ?1000 = clicks
    mouse_button: bool, // ?1002 = press and release tracking
    mouse_any: bool,    // ?1003 = hover and stuff
    pub mouse_sgr: bool, // ?1006 = when sending mouse events to the child process (the TUI app) we be using the SGR encoding format instead of the legacy X10 format cuz we cool guys
}

// wohoo more basic tty handling that I definitely did not steal and totally know what it does
impl TermGrid {
    pub fn new(rows: usize, cols: usize) -> Self {
        TermGrid {
            cells: vec![vec![Cell::default(); cols]; rows],
            rows,
            cols,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            scroll_top: 0,
            scroll_bottom: rows,
            attr_fg: TermColor::Default,
            attr_bg: TermColor::Default,
            attr_bold: false,
            attr_reverse: false,
            saved_cursor: None,
            alt_saved: None,
            mouse_normal: false,
            mouse_button: false,
            mouse_any: false,
            mouse_sgr: false,
        }
    }

    pub fn mouse_enabled(&self) -> bool {
        self.mouse_normal || self.mouse_button || self.mouse_any
    }

    /// blank slate for a fresh session, same size
    pub fn reset(&mut self) {
        *self = TermGrid::new(self.rows, self.cols);
    }

    pub fn resize(&mut self, new_rows: usize, new_cols: usize) {
        if new_rows == self.rows && new_cols == self.cols {
            return;
        }
        for row in &mut self.cells {
            row.resize(new_cols, Cell::default());
        }
        while self.cells.len() < new_rows {
            self.cells.push(vec![Cell::default(); new_cols]);
        }
        self.cells.truncate(new_rows);
        self.rows = new_rows;
        self.cols = new_cols;
        self.scroll_top = 0;
        self.scroll_bottom = new_rows;
        self.cursor_row = self.cursor_row.min(new_rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(new_cols.saturating_sub(1));
    }

    fn reset_attrs(&mut self) {
        self.attr_fg = TermColor::Default;
        self.attr_bg = TermColor::Default;
        self.attr_bold = false;
        self.attr_reverse = false;
    }

    fn put_char(&mut self, c: char) {
        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.line_feed();
        }
        if self.cursor_row < self.rows && self.cursor_col < self.cols {
            self.cells[self.cursor_row][self.cursor_col] = Cell {
                ch: c,
                fg: self.attr_fg,
                bg: self.attr_bg,
                bold: self.attr_bold,
                reverse: self.attr_reverse,
            };
        }
        self.cursor_col += 1;
    }

    fn line_feed(&mut self) {
        if self.cursor_row + 1 >= self.scroll_bottom {
            self.scroll_up();
        } else {
            self.cursor_row += 1;
        }
    }

    fn scroll_up(&mut self) {
        if self.scroll_top < self.scroll_bottom && self.scroll_bottom <= self.rows {
            self.cells.remove(self.scroll_top);
            self.cells
                .insert(self.scroll_bottom - 1, vec![Cell::default(); self.cols]);
        }
    }

    fn scroll_down(&mut self) {
        if self.scroll_top < self.scroll_bottom && self.scroll_bottom <= self.rows {
            self.cells.remove(self.scroll_bottom - 1);
            self.cells
                .insert(self.scroll_top, vec![Cell::default(); self.cols]);
        }
    }

    fn erase_display(&mut self, mode: u16) {
        match mode {
            0 => {
                // cursor to end
                for c in self.cursor_col..self.cols {
                    self.cells[self.cursor_row][c] = Cell::default();
                }
                for r in (self.cursor_row + 1)..self.rows {
                    for c in 0..self.cols {
                        self.cells[r][c] = Cell::default();
                    }
                }
            }
            1 => {
                for r in 0..self.cursor_row {
                    for c in 0..self.cols {
                        self.cells[r][c] = Cell::default();
                    }
                }
                for c in 0..=self.cursor_col.min(self.cols.saturating_sub(1)) {
                    self.cells[self.cursor_row][c] = Cell::default();
                }
            }
            2 | 3 => {
                for r in 0..self.rows {
                    for c in 0..self.cols {
                        self.cells[r][c] = Cell::default();
                    }
                }
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: u16) {
        let row = self.cursor_row;
        if row >= self.rows {
            return;
        }
        match mode {
            0 => {
                for c in self.cursor_col..self.cols {
                    self.cells[row][c] = Cell::default();
                }
            }
            1 => {
                for c in 0..=self.cursor_col.min(self.cols.saturating_sub(1)) {
                    self.cells[row][c] = Cell::default();
                }
            }
            2 => {
                for c in 0..self.cols {
                    self.cells[row][c] = Cell::default();
                }
            }
            _ => {}
        }
    }

    fn erase_chars(&mut self, n: usize) {
        let row = self.cursor_row;
        if row >= self.rows {
            return;
        }
        for i in 0..n {
            let c = self.cursor_col + i;
            if c < self.cols {
                self.cells[row][c] = Cell::default();
            }
        }
    }

    fn delete_chars(&mut self, n: usize) {
        let row = self.cursor_row;
        if row >= self.rows {
            return;
        }
        for _ in 0..n {
            if self.cursor_col < self.cols {
                self.cells[row].remove(self.cursor_col);
                self.cells[row].push(Cell::default());
            }
        }
    }

    fn insert_chars(&mut self, n: usize) {
        let row = self.cursor_row;
        if row >= self.rows {
            return;
        }
        for _ in 0..n {
            if self.cursor_col < self.cols {
                self.cells[row].insert(self.cursor_col, Cell::default());
                self.cells[row].truncate(self.cols);
            }
        }
    }

    fn insert_lines(&mut self, n: usize) {
        for _ in 0..n {
            if self.cursor_row < self.scroll_bottom {
                if self.scroll_bottom <= self.rows {
                    self.cells.remove(self.scroll_bottom - 1);
                }
                self.cells
                    .insert(self.cursor_row, vec![Cell::default(); self.cols]);
            }
        }
    }

    fn delete_lines(&mut self, n: usize) {
        for _ in 0..n {
            if self.cursor_row < self.scroll_bottom && self.cursor_row < self.rows {
                self.cells.remove(self.cursor_row);
                let insert_at = (self.scroll_bottom - 1).min(self.cells.len());
                self.cells
                    .insert(insert_at, vec![Cell::default(); self.cols]);
            }
        }
    }

    fn enter_alt_screen(&mut self) {
        self.alt_saved = Some((self.cells.clone(), self.cursor_row, self.cursor_col));
        self.erase_display(2);
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    fn leave_alt_screen(&mut self) {
        if let Some((cells, row, col)) = self.alt_saved.take() {
            self.cells = cells;
            self.cursor_row = row;
            self.cursor_col = col;
        }
    }

    // sgr = set graphics rendition (colors and attributes) not the mouse tracking stuff like above!
    fn sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            self.reset_attrs();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => self.reset_attrs(),
                1 => self.attr_bold = true,
                7 => self.attr_reverse = true,
                22 => self.attr_bold = false,
                27 => self.attr_reverse = false,
                30..=37 => self.attr_fg = TermColor::Indexed(params[i] as u8 - 30),
                38 => {
                    // this is extended foreground color
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        self.attr_fg = TermColor::Indexed(params[i + 2] as u8);
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        self.attr_fg = TermColor::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        );
                        i += 4;
                    }
                }
                39 => self.attr_fg = TermColor::Default,
                40..=47 => self.attr_bg = TermColor::Indexed(params[i] as u8 - 40),
                48 => {
                    // and dis da extended bg color
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        self.attr_bg = TermColor::Indexed(params[i + 2] as u8);
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        self.attr_bg = TermColor::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        );
                        i += 4;
                    }
                }
                49 => self.attr_bg = TermColor::Default,
                90..=97 => self.attr_fg = TermColor::Indexed(params[i] as u8 - 90 + 8),
                100..=107 => self.attr_bg = TermColor::Indexed(params[i] as u8 - 100 + 8),
                _ => {}
            }
            i += 1;
        }
    }

    fn handle_csi(&mut self, params: &[u16], intermediates: &[u8], action: char) {
        // helper: get param with default
        let p = |i: usize, def: u16| -> u16 {
            params.get(i).copied().filter(|&v| v > 0).unwrap_or(def)
        };

        let private = intermediates.contains(&b'?');

        match action {
            'A' => {
                let n = p(0, 1) as usize;
                self.cursor_row = self.cursor_row.saturating_sub(n);
            }
            'B' => {
                let n = p(0, 1) as usize;
                self.cursor_row = (self.cursor_row + n).min(self.rows.saturating_sub(1));
            }
            'C' => {
                let n = p(0, 1) as usize;
                self.cursor_col = (self.cursor_col + n).min(self.cols.saturating_sub(1));
            }
            'D' => {
                let n = p(0, 1) as usize;
                self.cursor_col = self.cursor_col.saturating_sub(n);
            }
            'H' | 'f' => {
                // cursor position (1-based)
                let row = p(0, 1) as usize;
                let col = p(1, 1) as usize;
                self.cursor_row = row.saturating_sub(1).min(self.rows.saturating_sub(1));
                self.cursor_col = col.saturating_sub(1).min(self.cols.saturating_sub(1));
            }
            'J' => self.erase_display(p(0, 0)),
            'K' => self.erase_line(p(0, 0)),
            'L' => self.insert_lines(p(0, 1) as usize),
            'M' => self.delete_lines(p(0, 1) as usize),
            'P' => self.delete_chars(p(0, 1) as usize),
            'X' => self.erase_chars(p(0, 1) as usize),
            '@' => self.insert_chars(p(0, 1) as usize),
            'G' | '`' => {
                let col = p(0, 1) as usize;
                self.cursor_col = col.saturating_sub(1).min(self.cols.saturating_sub(1));
            }
            'd' => {
                let row = p(0, 1) as usize;
                self.cursor_row = row.saturating_sub(1).min(self.rows.saturating_sub(1));
            }
            'S' => {
                for _ in 0..p(0, 1) {
                    self.scroll_up();
                }
            }
            'T' => {
                for _ in 0..p(0, 1) {
                    self.scroll_down();
                }
            }
            'm' => {
                if params.is_empty() {
                    self.sgr(&[0]);
                } else {
                    self.sgr(params);
                }
            }
            'r' => {
                let top = p(0, 1) as usize;
                let bottom = p(1, self.rows as u16) as usize;
                self.scroll_top = top.saturating_sub(1);
                self.scroll_bottom = bottom.min(self.rows);
            }
            's' => {
                self.saved_cursor = Some((self.cursor_row, self.cursor_col));
            }
            'u' => {
                if let Some((r, c)) = self.saved_cursor {
                    self.cursor_row = r.min(self.rows.saturating_sub(1));
                    self.cursor_col = c.min(self.cols.saturating_sub(1));
                }
            }
            'h' if private => {
                for &param in params {
                    match param {
                        25 => self.cursor_visible = true,
                        1000 => self.mouse_normal = true,
                        1002 => self.mouse_button = true,
                        1003 => self.mouse_any = true,
                        1006 => self.mouse_sgr = true,
                        1049 => self.enter_alt_screen(),
                        _ => {}
                    }
                }
            }
            'l' if private => {
                for &param in params {
                    match param {
                        25 => self.cursor_visible = false,
                        1000 => self.mouse_normal = false,
                        1002 => self.mouse_button = false,
                        1003 => self.mouse_any = false,
                        1006 => self.mouse_sgr = false,
                        1049 => self.leave_alt_screen(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

// vte perform implementation, basically the heart of the term emu
impl Perform for TermGrid {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => {
                // backspace
                self.cursor_col = self.cursor_col.saturating_sub(1);
            }
            0x09 => {
                // tab (ltrly just moves cursor 8 spaces forward)
                self.cursor_col = ((self.cursor_col / 8) + 1) * 8;
                if self.cursor_col >= self.cols {
                    self.cursor_col = self.cols.saturating_sub(1);
                }
            }
            0x0A..=0x0C => {
                // line feed
                self.line_feed();
            }
            0x0D => {
                // enter (cr)
                self.cursor_col = 0;
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }
        let flat: Vec<u16> = params.iter().map(|sub| sub[0]).collect();
        self.handle_csi(&flat, intermediates, action);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if !intermediates.is_empty() {
            return;
        }
        match byte {
            b'7' => {
                self.saved_cursor = Some((self.cursor_row, self.cursor_col));
            }
            b'8' => {
                if let Some((r, c)) = self.saved_cursor {
                    self.cursor_row = r.min(self.rows.saturating_sub(1));
                    self.cursor_col = c.min(self.cols.saturating_sub(1));
                }
            }
            b'D' => self.line_feed(),
            b'M' => {
                // reverse indexsexd
                if self.cursor_row == self.scroll_top {
                    self.scroll_down();
                } else {
                    self.cursor_row = self.cursor_row.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
}
