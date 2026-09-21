// kiosk bits: status bar along the bottom (menu left, keyboard right) and the on screen keyboard itself, for touchscreens with nothing plugged in

use eframe::egui::{self, Align2, Pos2, RichText};

use crate::config::{parse_osk_row, KeyboardMode, OskKey, Text};

pub enum Press {
    Text(String),
    /// ctrl was on when this got typed
    Ctrl(char),
    /// one of schema::KEY_NAMES, minus shift and ctrl which we handle here
    Special(String),
}

#[derive(Default)]
pub struct Output {
    /// menu button: where to put the menu and which corner of it goes there
    pub menu: Option<(Pos2, Align2)>,
    pub presses: Vec<Press>,
}

pub struct Kiosk {
    mode: KeyboardMode,
    keyboard_open: bool,
    shift: bool,
    ctrl: bool,
    rows: Vec<Vec<OskKey>>,
    shifted: Vec<Vec<OskKey>>,
}

impl Kiosk {
    pub fn new(cfg: &crate::config::Kiosk) -> Self {
        // build.rs already checked these parse
        let parse = |rows: &[String]| rows.iter().filter_map(|r| parse_osk_row(r).ok()).collect();
        Kiosk {
            mode: cfg.keyboard_mode,
            keyboard_open: false,
            shift: false,
            ctrl: false,
            rows: parse(&cfg.keyboard),
            shifted: parse(&cfg.keyboard_shift),
        }
    }

    /// bottom panels, has to run before the central panel so it gets the leftover space
    pub fn show(&mut self, ui: &mut egui::Ui, text: &Text) -> Output {
        let mut out = Output::default();

        let bar = egui::Panel::bottom("taco_status_bar").exact_size(52.0).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                let menu = ui.button(RichText::new(&text.kiosk_menu).size(18.0));
                if menu.clicked() {
                    out.menu = Some((menu.rect.left_top(), Align2::LEFT_BOTTOM));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let kb = egui::Button::new(RichText::new(&text.kiosk_keyboard).size(18.0))
                        .selected(self.keyboard_open);
                    if ui.add(kb).clicked() {
                        self.keyboard_open = !self.keyboard_open;
                    }
                });
            });
        });

        if !self.keyboard_open {
            return out;
        }
        match self.mode {
            // a panel of its own, the terminal gets whats left
            KeyboardMode::Shrink => {
                egui::Panel::bottom("taco_keyboard").show(ui, |ui| {
                    ui.add_space(4.0);
                    self.keys(ui, &mut out);
                    ui.add_space(4.0);
                });
            }
            // floating right above the status bar, the terminal keeps its size
            KeyboardMode::Overlay => {
                let bar = bar.response.rect;
                egui::Area::new(egui::Id::new("taco_keyboard"))
                    .order(egui::Order::Foreground)
                    .pivot(Align2::LEFT_BOTTOM)
                    .fixed_pos(bar.left_top())
                    .show(ui.ctx(), |ui| {
                        egui::Frame::window(ui.style()).corner_radius(0.0).show(ui, |ui| {
                            ui.set_width(bar.width() - 16.0);
                            self.keys(ui, &mut out);
                        });
                    });
            }
        }
        out
    }

    fn keys(&mut self, ui: &mut egui::Ui, out: &mut Output) {
        let gap = 4.0;
        let height = 46.0;
        let rows = if self.shift { self.shifted.clone() } else { self.rows.clone() };

        let widest = rows
            .iter()
            .map(|r| (r.iter().map(units).sum::<f32>(), r.len()))
            .fold((1.0f32, 1usize), |a, b| (a.0.max(b.0), a.1.max(b.1)));
        let unit = ((ui.available_width() - gap * widest.1 as f32) / widest.0).clamp(24.0, 90.0);

        ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
        for row in &rows {
            let row_width: f32 = row.iter().map(|k| units(k) * unit).sum::<f32>() + gap * row.len() as f32;
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - row_width) / 2.0).max(0.0));
                for key in row {
                    let on = match key {
                        OskKey::Special(n) if n == "shift" => self.shift,
                        OskKey::Special(n) if n == "ctrl" => self.ctrl,
                        _ => false,
                    };
                    let button = egui::Button::new(RichText::new(label(key)).monospace().size(18.0)).selected(on);
                    if ui.add_sized([units(key) * unit, height], button).clicked() {
                        self.press(key, out);
                    }
                }
            });
        }
    }

    fn press(&mut self, key: &OskKey, out: &mut Output) {
        match key {
            OskKey::Special(n) if n == "shift" => self.shift = !self.shift,
            OskKey::Special(n) if n == "ctrl" => self.ctrl = !self.ctrl,
            OskKey::Special(n) => {
                out.presses.push(Press::Special(n.clone()));
                self.shift = false;
                self.ctrl = false;
            }
            OskKey::Char(s) => {
                match s.chars().next() {
                    Some(c) if self.ctrl => out.presses.push(Press::Ctrl(c)),
                    _ => out.presses.push(Press::Text(s.clone())),
                }
                self.shift = false;
                self.ctrl = false;
            }
        }
    }
}

fn units(key: &OskKey) -> f32 {
    match key {
        OskKey::Special(n) => match n.as_str() {
            "space" => 5.0,
            "enter" | "shift" => 1.75,
            "bksp" | "tab" | "ctrl" => 1.5,
            _ => 1.0,
        },
        OskKey::Char(_) => 1.0,
    }
}

fn label(key: &OskKey) -> String {
    match key {
        OskKey::Char(s) => s.clone(),
        OskKey::Special(n) => match n.as_str() {
            "esc" => "Esc",
            "tab" => "Tab ⇥",
            "bksp" => "Bksp",
            "enter" => "Enter",
            "shift" => "⇧",
            "ctrl" => "Ctrl",
            "space" => " ",
            "up" => "↑",
            "down" => "↓",
            "left" => "←",
            "right" => "→",
            "home" => "Home",
            "end" => "End",
            "pgup" => "PgUp",
            "pgdn" => "PgDn",
            "del" => "Del",
            "ins" => "Ins",
            f => return f.to_uppercase(),
        }
        .to_string(),
    }
}
