// the setup / login screen for ssh mode: logo, server, username, password, the shell checkbox and on a kiosk the pin

use eframe::egui::{self, Color32, RichText};
use zeroize::Zeroizing;

use crate::config::{self, Config};
use crate::state::Saved;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Server,
    Username,
    Password,
    Pin,
}

pub struct Form {
    pub server: String,
    pub username: String,
    pub password: Zeroizing<String>,
    pub login_shell: bool,
    pub remember_server: bool,
    pub remember_username: bool,
    pub pin: Zeroizing<String>,
    pub error: Option<String>,
    /// last field that had focus, the on screen keyboard types into it
    pub focus: Field,
    /// give focus back to `focus` next frame (tapping the osk steals it)
    pub refocus: bool,
}

impl Form {
    pub fn new(cfg: &Config, saved: Option<&Saved>) -> Self {
        let ssh = cfg.ssh.as_ref();
        let toggle_default = ssh
            .and_then(|s| s.shell_toggle.as_ref())
            .is_none_or(|t| t.default);
        let server = saved.map(|s| s.server.clone()).unwrap_or_default();
        let username = saved.map(|s| s.username.clone()).unwrap_or_default();
        // first empty field, what was remembered can be skipped
        let focus = if server.is_empty() && ssh.is_some_and(|s| s.server.is_none()) {
            Field::Server
        } else if username.is_empty() {
            Field::Username
        } else {
            Field::Password
        };
        Form {
            server,
            username,
            password: Zeroizing::default(),
            login_shell: saved.map_or(toggle_default, |s| s.login_shell),
            remember_server: saved.is_none_or(|s| s.remember_server),
            remember_username: saved.is_none_or(|s| s.remember_username),
            pin: Zeroizing::default(),
            error: None,
            focus,
            refocus: true,
        }
    }

    pub fn field_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::Server => &mut self.server,
            Field::Username => &mut self.username,
            Field::Password => &mut self.password,
            Field::Pin => &mut self.pin,
        }
    }
}

pub struct Look<'a> {
    pub cfg: &'a Config,
    /// connecting right now, everything greyed out
    pub busy: Option<String>,
    /// kiosk with a saved setup: only the password can change, logout for the rest
    pub locked: bool,
    pub show_pin: bool,
    /// shown under everything, for the unsafe credential store
    pub warning: Option<&'a str>,
}

impl Look<'_> {
    pub fn show_server(&self) -> bool {
        self.cfg.ssh.as_ref().is_some_and(|s| s.server.is_none())
    }

    /// fields in tab order, only the ones you can type in
    pub fn fields(&self) -> Vec<Field> {
        let mut f = Vec::new();
        if !self.locked {
            if self.show_server() {
                f.push(Field::Server);
            }
            f.push(Field::Username);
        }
        f.push(Field::Password);
        if self.show_pin {
            f.push(Field::Pin);
        }
        f
    }
}

/// true = connect was pressed
pub fn show(ui: &mut egui::Ui, form: &mut Form, look: Look) -> bool {
    let cfg = look.cfg;
    let text = &cfg.text;
    let fg = config::color(&cfg.terminal.foreground);
    let logo_color = cfg.login.logo_color.as_deref().map_or(fg, config::color);
    let editable = look.busy.is_none();
    let mut connect = false;

    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(32.0);
            if !cfg.login.logo.trim().is_empty() {
                let logo = RichText::new(pad_logo(&cfg.login.logo)).monospace().color(logo_color);
                ui.add(egui::Label::new(logo).halign(egui::Align::LEFT).extend());
                ui.add_space(12.0);
            }
            if !cfg.login.title.is_empty() {
                ui.label(RichText::new(&cfg.login.title).size(20.0).color(fg));
                ui.add_space(12.0);
            }

            let mut field = |ui: &mut egui::Ui, form: &mut Form, which: Field, label: &str, enabled: bool| {
                ui.label(RichText::new(label).color(fg));
                let secret = matches!(which, Field::Password | Field::Pin);
                let len = form.field_mut(which).chars().count();
                let edit = egui::TextEdit::singleline(form.field_mut(which))
                    .password(secret)
                    .desired_width(280.0);
                let r = ui.add_enabled(enabled, edit);
                if form.refocus && form.focus == which && enabled {
                    r.request_focus();
                    form.refocus = false;
                    // text may have changed behind its back (osk), cursor goes to the end
                    let mut state = egui::text_edit::TextEditState::load(ui.ctx(), r.id).unwrap_or_default();
                    let end = egui::text::CCursorRange::one(egui::text::CCursor::new(len));
                    state.cursor.set_char_range(Some(end));
                    state.store(ui.ctx(), r.id);
                }
                if r.has_focus() {
                    form.focus = which;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    connect = true;
                }
                ui.add_space(6.0);
            };

            if look.show_server() {
                field(ui, form, Field::Server, &text.server, editable && !look.locked);
            }
            field(ui, form, Field::Username, &text.username, editable && !look.locked);
            field(ui, form, Field::Password, &text.password, editable);
            if look.show_pin {
                field(ui, form, Field::Pin, &text.kiosk_pin, editable);
            }

            // checkboxes in a left aligned column the width of the fields, centered as a
            // whole, otherwise every box sits somewhere else depending on its label
            let layout = egui::Layout::top_down(egui::Align::Min);
            ui.allocate_ui_with_layout(egui::vec2(280.0, 0.0), layout, |ui| {
                if let Some(toggle) = cfg.ssh.as_ref().and_then(|s| s.shell_toggle.as_ref()) {
                    ui.add_enabled(
                        editable && !look.locked,
                        egui::Checkbox::new(&mut form.login_shell, &toggle.label),
                    );
                }
                if !look.locked && look.show_server() && cfg.login.remember_server_checkbox {
                    ui.add_enabled(editable, egui::Checkbox::new(&mut form.remember_server, &text.remember_server));
                }
                if !look.locked && cfg.login.remember_username_checkbox {
                    ui.add_enabled(editable, egui::Checkbox::new(&mut form.remember_username, &text.remember_username));
                }
            });
            ui.add_space(6.0);

            if let Some(error) = &form.error {
                ui.label(RichText::new(error).color(Color32::from_rgb(255, 110, 110)));
                ui.add_space(6.0);
            }

            match &look.busy {
                Some(status) => {
                    ui.add(egui::Spinner::new().size(20.0));
                    ui.label(status);
                }
                None => {
                    if ui.button(RichText::new(&text.connect).size(16.0)).clicked() {
                        connect = true;
                    }
                }
            }
            if let Some(warning) = look.warning {
                ui.add_space(12.0);
                ui.label(RichText::new(warning).color(Color32::from_rgb(255, 200, 90)));
            }
            ui.add_space(32.0);
        });
    });
    connect && editable
}

/// ascii art hack fix to make it not look horrible
fn pad_logo(logo: &str) -> String {
    let logo = logo.trim_matches('\n');
    let width = logo.lines().map(|l| l.chars().count()).max().unwrap_or(0);
    logo.lines()
        .map(|l| format!("{l:<width$}"))
        .collect::<Vec<_>>()
        .join("\n")
}
