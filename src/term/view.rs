// drawing the grid with egui and turning pointer stuff into terminal mouse events

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Vec2};

use super::input::{mouse_seq, MouseAction};
use super::{Palette, TermGrid};

/// how far a finger has to move (points) before a touch counts as a drag. touchscreens jitter, a kindle's by
/// 10+ points on a plain tap, and a tap the app sees as a drag is a click it never gets
const TOUCH_SLOP: f32 = 20.0;

pub struct TermView {
    font: FontId,
    /// size of one cell in points, measured on the first frame
    pub cell: Vec2,
    last_mouse_button: Option<u8>, // track held mouse button for drag/release
    scroll_accumulator: f32,        // accumulate pixel scroll delta for discrete line events
    last_motion: Option<(u16, u16)>, // cell of the last hover/drag report, only report changes
    touch_origin: Option<Pos2>,       // where a finger came down, until it moves further than TOUCH_SLOP
}

impl TermView {
    pub fn new(font_size: f32) -> Self {
        TermView {
            font: FontId::monospace(font_size),
            cell: Vec2::ZERO,
            last_mouse_button: None,
            scroll_accumulator: 0.0,
            last_motion: None,
            touch_origin: None,
        }
    }

    pub fn measure(&mut self, ctx: &egui::Context) {
        if self.cell != Vec2::ZERO {
            return;
        }
        let font = self.font.clone();
        self.cell = ctx.fonts_mut(|f| {
            let galley = f.layout_no_wrap("M".into(), font.clone(), Color32::WHITE);
            egui::vec2(galley.rect.width(), f.row_height(&font))
        });
    }

    /// how many cells fit in rect
    pub fn fit(&self, rect: Rect) -> (u16, u16) {
        if self.cell.x <= 0.0 || self.cell.y <= 0.0 {
            return (0, 0);
        }
        let cols = (rect.width() / self.cell.x).floor() as u16;
        let rows = (rect.height() / self.cell.y).floor() as u16;
        (cols.max(20), rows.max(5))
    }

    pub fn paint(&self, painter: &egui::Painter, rect: Rect, grid: &TermGrid, pal: &Palette) {
        let (cw, ch) = (self.cell.x, self.cell.y);
        if cw <= 0.0 {
            return;
        }

        // attemt to make text be alligned more sexily
        for row in 0..grid.rows {
            let y = rect.min.y + row as f32 * ch;

            // draw sexy background and batch ones where bg is le same
            let mut bg_start = 0usize;
            let mut current_bg = grid.cells[row][0].resolved_bg(pal);

            for col in 0..=grid.cols {
                let cell_bg = if col < grid.cols {
                    grid.cells[row][col].resolved_bg(pal)
                } else {
                    Color32::TRANSPARENT // le flush de toilete of last span
                };

                if cell_bg != current_bg || col == grid.cols {
                    // draw the background spank
                    if current_bg != pal.bg {
                        let x0 = rect.min.x + bg_start as f32 * cw;
                        let x1 = rect.min.x + col as f32 * cw;
                        painter.rect_filled(
                            Rect::from_min_max(egui::pos2(x0, y), egui::pos2(x1, y + ch)),
                            0.0,
                            current_bg,
                        );
                    }
                    bg_start = col;
                    current_bg = cell_bg;
                }
            }

            // attempt at preventing subbixel shift of text
            for col in 0..grid.cols {
                let cell = &grid.cells[row][col];
                if cell.ch == ' ' || cell.ch == '\0' {
                    continue;
                }
                let x = rect.min.x + col as f32 * cw;
                let mut buf = [0u8; 4];
                painter.text(
                    egui::pos2(x, y),
                    egui::Align2::LEFT_TOP,
                    cell.ch.encode_utf8(&mut buf),
                    self.font.clone(),
                    cell.resolved_fg(pal),
                );
            }
        }

        // le cursor
        if grid.cursor_visible && grid.cursor_row < grid.rows && grid.cursor_col < grid.cols {
            let cx = rect.min.x + grid.cursor_col as f32 * cw;
            let cy = rect.min.y + grid.cursor_row as f32 * ch;
            painter.rect_filled(
                Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, ch)),
                0.0,
                Color32::from_rgba_premultiplied(180, 180, 180, 100),
            );
        }
    }

    /// translate egui to terminal mouse events, returns what to send to the app.
    /// skip_secondary = that right click belongs to our context menu,
    /// covered = the pointer is over something of ours (menu, banner, keyboard)
    pub fn mouse(
        &mut self,
        input: &egui::InputState,
        rect: Rect,
        grid: &TermGrid,
        skip_secondary: bool,
        covered: bool,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        if !grid.mouse_enabled() {
            self.last_mouse_button = None;
            return out;
        }
        let (cw, ch) = (self.cell.x, self.cell.y);
        // a finger is gone the same frame it lets go, theres no latest_pos then. interact_pos still knows where
        let Some(pos) = input.pointer.latest_pos().or(input.pointer.interact_pos()) else { return out };
        if cw <= 0.0 || ch <= 0.0 {
            return out;
        }
        let sgr = grid.mouse_sgr;
        let inside = rect.contains(pos) && !covered;
        let cell = |p: Pos2| {
            let col = ((p.x - rect.min.x) / cw).floor().clamp(0.0, grid.cols.saturating_sub(1) as f32) as u16;
            let row = ((p.y - rect.min.y) / ch).floor().clamp(0.0, grid.rows.saturating_sub(1) as f32) as u16;
            (col, row)
        };
        let (col, row) = cell(pos);

        // ai fix ahead, had issues where buttons in lists would loose their selection/highlighting box caused by scrolling too fast + smooth scrolling (mainly on macos, linux was fine)
        // either i suck at googling or the internet is just lobotomized by now but i was unable to find a fix so yea ... llm was used sry

        // scroll events — accumulate pixel delta, fire at most a few events per frame
        if inside {
            let scroll_y: f32 = input
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                        egui::MouseWheelUnit::Point => delta.y,
                        egui::MouseWheelUnit::Line => delta.y * ch,
                        egui::MouseWheelUnit::Page => delta.y * rect.height(),
                    }),
                    _ => None,
                })
                .sum();
            if scroll_y != 0.0 {
                self.scroll_accumulator += scroll_y;
                // use 3x cell height as threshold so scrolling feels natural
                // and cap at 3 events per frame to prevent list flying to the top
                let step = (ch * 3.0).max(30.0);
                let max_events = 3u8;
                let mut fired = 0u8;
                while self.scroll_accumulator.abs() >= step && fired < max_events {
                    let button: u8 = if self.scroll_accumulator > 0.0 { 64 } else { 65 };
                    if self.scroll_accumulator > 0.0 {
                        self.scroll_accumulator -= step;
                    } else {
                        self.scroll_accumulator += step;
                    }
                    fired += 1;
                    out.extend(mouse_seq(button, col, row, MouseAction::Press, sgr));
                }
                // drain leftover so momentum so no infinite scrolling mlol
                if fired == max_events {
                    self.scroll_accumulator = 0.0;
                }
            }
        }

        // je suis pressing le bouton (i dont speak french btw only swiss-german)
        if inside && self.last_mouse_button.is_none() {
            let pressed = if input.pointer.button_pressed(egui::PointerButton::Primary) {
                Some(0)
            } else if input.pointer.button_pressed(egui::PointerButton::Middle) {
                Some(1)
            } else if input.pointer.button_pressed(egui::PointerButton::Secondary) && !skip_secondary {
                Some(2)
            } else {
                None
            };
            if let Some(button) = pressed {
                self.last_mouse_button = Some(button);
                // the press is where the button is now, no motion report for the same cell after it
                self.last_motion = Some((col, row));
                self.touch_origin = input.any_touches().then_some(pos);
                out.extend(mouse_seq(button, col, row, MouseAction::Press, sgr));
            }
        }

        if let Some(button) = self.last_mouse_button {
            // a finger stays where it came down until it clearly moves, so a shaky tap is still a click
            let (col, row) = match self.touch_origin {
                Some(origin) if origin.distance(pos) < TOUCH_SLOP => cell(origin),
                _ => {
                    self.touch_origin = None;
                    (col, row)
                }
            };
            // oui oui ratatui je suis machen le button nolonger pressing
            if input.pointer.any_released() {
                self.last_mouse_button = None;
                out.extend(mouse_seq(button, col, row, MouseAction::Release, sgr));
            } else if self.last_motion != Some((col, row)) {
                // drag queen
                self.last_motion = Some((col, row));
                out.extend(mouse_seq(button, col, row, MouseAction::Drag, sgr));
            }
        } else if grid.mouse_any && inside && self.last_motion != Some((col, row)) {
            // hover: no button held, the app asked for every move (?1003).
            // "button" 3 + the motion flag = 35, what xterm sends
            self.last_motion = Some((col, row));
            out.extend(mouse_seq(3, col, row, MouseAction::Drag, sgr));
        }
        out
    }

    /// forget a held button, eg when a popup steals the pointer mid drag
    pub fn release_all(&mut self) {
        self.last_mouse_button = None;
        self.scroll_accumulator = 0.0;
        self.last_motion = None;
        self.touch_origin = None;
    }
}
