// brace yourself for another 1000 line file. heres what it does : 
//
// the window. owns the terminal and the session and walks between the screens:
//
//   local:  Running <-> Failed
//   ssh:    Login -> LoggingIn -> Running
//           Running -> Interrupted (cant reach server, keeps trying)
//           Running -> Lost (session killed, countdown then reinitialize)
//           either one -> Running once a new session comes up
//           anywhere -> KeyChanged (server key isnt the one we know, big red box)

mod kiosk;
mod login;

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, Id, Pos2, RichText};
use zeroize::Zeroizing;

use crate::config::{self, fill, Config, LocalOnExit, MenuTrigger, SshOnExit};
use crate::creds::{self, CredentialStore, Secret};
use crate::schema::{split_server, DEFAULT_BINARY, DEFAULT_BINARY_WARNING};
use crate::session::{self, End, Event, HostKeyQuestion, KeyChange, Session, Waker};
use crate::state::{self, Saved, Store, WindowMemory, WindowSize};
use crate::term::input::{ctrl_char_byte, ctrl_key_byte, named_key_bytes, special_key_bytes};
use crate::term::view::TermView;
use crate::term::{Palette, TermGrid};

const RED: Color32 = Color32::from_rgb(255, 110, 110);

/// pin pad keys and the gap between them, the pad is 3 keys wide
const PIN_KEY: egui::Vec2 = egui::vec2(64.0, 48.0);
const PIN_GAP: f32 = 8.0;
const PIN_PAD_WIDTH: f32 = 3.0 * PIN_KEY.x + 2.0 * PIN_GAP;

enum Screen {
    /// ssh setup / login form
    Login,
    /// first connect from the form, the form stays up greyed out meanwhile
    LoggingIn,
    /// reinitialize picked from the menu or a retry button
    Connecting,
    Running,
    /// server cant be reached, next try at retry_at (unless one is already running)
    Interrupted { retry_at: Instant },
    /// session got killed but the server is there, reinitialize at retry_at
    Lost { retry_at: Instant },
    /// the server showed a key we dont know it by
    KeyChanged(KeyScreen),
    /// local program wouldnt start
    Failed(String),
}

struct KeyScreen {
    /// None = the config pins the keys, nothing to accept
    change: Option<KeyChange>,
    /// fingerprint the server showed
    new: String,
    /// came straight from the login form, nothing saved yet
    first: bool,
    /// accept on our own at this point (changed_key_accept_after_secs)
    accept_at: Option<Instant>,
    /// accepting went wrong (cant write known_hosts)
    error: Option<String>,
}

enum Dialog {
    /// confirm, plus the kiosk pin when theres one
    Logout { pin: String, wrong: bool },
    /// only the kiosk pin, then do the thing
    Pin { then: AfterPin, pin: String, wrong: bool },
}

#[derive(Clone, Copy)]
enum AfterPin {
    Exit,
    AcceptKey,
}

#[derive(Clone, Copy)]
enum MenuItem {
    Restart,
    Reinitialize,
    Logout,
    Exit,
}

/// buttons on the status card in the middle
#[derive(Clone, Copy)]
enum CardAction {
    RetryNow,
    RetryLocal,
    Logout,
    Back,
    AcceptKey,
    Exit,
}

struct Menu {
    pos: Pos2,
    pivot: Align2,
    /// opened this frame, so the click that opened it doesnt also close it
    fresh: bool,
}

/// the yellow boxes top right, closable, gone until the next launch
struct Banner {
    text: String,
    /// also over the login screen (which has its own line for the unsafe one)
    on_login: bool,
}

pub struct TacoApp {
    cfg: Config,
    pal: Palette,
    rt: tokio::runtime::Handle,
    wake: Waker,

    grid: TermGrid,
    parser: vte::Parser,
    view: TermView,
    session: Option<Session>,
    screen: Screen,

    store: Store,
    /// keeps the window size for the next launch, None when it isnt remembered (see remember_window_size)
    window: Option<WindowMemory>,
    /// the setup that worked last, None until the first login (and after logout).
    /// full values while running, whats on disk depends on the remember checkboxes
    saved: Option<Saved>,
    /// password of the current login, for reconnects. the store is for relaunches
    password: Option<Secret>,
    creds: Box<dyn CredentialStore>,
    form: login::Form,

    host_key: Option<HostKeyQuestion>,
    menu: Option<Menu>,
    dialog: Option<Dialog>,
    /// status bar and on screen keyboard: kiosks, and phones and tablets always
    bar: Option<kiosk::Kiosk>,
    /// the pin and the locked login, only a real kiosk gets those
    kiosk: bool,
    /// unsafe credential store in use and the config wants that said
    unsafe_warning: bool,
    banners: Vec<Banner>,
    close: bool,
}

impl TacoApp {
    pub fn new(cc: &eframe::CreationContext, cfg: Config, rt: tokio::runtime::Handle, kiosk: bool) -> Self {
        let ctx = cc.egui_ctx.clone();
        let wake: Waker = Arc::new(move || ctx.request_repaint());

        // no keyboard or right click to fall back on on a phone, so the bar and the osk come without kiosk mode.
        // the pin and the locked login dont, a phone is someones own
        let touch = kiosk || cfg!(any(target_os = "ios", target_os = "android"));
        if touch {
            // fingers are fatter than mouse pointers
            cc.egui_ctx.all_styles_mut(|s| {
                s.spacing.button_padding = egui::vec2(14.0, 10.0);
                s.spacing.interact_size.y = 40.0;
                s.spacing.item_spacing = egui::vec2(10.0, 10.0);
            });
        }

        let store = Store::new(&cfg.app.id);
        let saved = cfg.ssh.as_ref().and_then(|_| store.load());
        let creds = creds::open(cfg.ssh.as_ref().map(|s| s.credentials).unwrap_or_default(), store.credentials());
        let form = login::Form::new(&cfg, saved.as_ref());

        let mut banners = Vec::new();
        if cfg.app.binary == DEFAULT_BINARY {
            eprintln!("warning: {DEFAULT_BINARY_WARNING}");
            banners.push(Banner { text: DEFAULT_BINARY_WARNING.into(), on_login: true });
        }
        let unsafe_warning = cfg
            .ssh
            .as_ref()
            .is_some_and(|s| s.credentials == config::Credentials::Unsafe && s.unsafe_warning);
        if unsafe_warning {
            eprintln!("warning: {}", cfg.text.unsafe_credentials);
            banners.push(Banner { text: cfg.text.unsafe_credentials.clone(), on_login: false });
        }

        let mut app = TacoApp {
            pal: Palette {
                fg: config::color(&cfg.terminal.foreground),
                bg: config::color(&cfg.terminal.background),
            },
            rt,
            wake,
            grid: TermGrid::new(cfg.terminal.rows() as usize, cfg.terminal.cols() as usize),
            parser: vte::Parser::new(),
            view: TermView::new(cfg.terminal.font_size),
            session: None,
            screen: Screen::Login,
            window: crate::remember_window_size(&cfg, kiosk).then(|| WindowMemory::new(store.clone())),
            store,
            saved,
            password: None,
            creds,
            form,
            host_key: None,
            menu: None,
            dialog: None,
            bar: touch.then(|| kiosk::Kiosk::new(&cfg.kiosk)),
            kiosk,
            unsafe_warning,
            banners,
            close: false,
            cfg,
        };

        if app.cfg.local.is_some() {
            app.start_local();
        } else if app.setup_complete() {
            // only goes anywhere when the store remembers passwords across launches
            // (unsafe, later a keyring), with the memory store its the login screen
            app.attempt(Screen::Connecting);
        }
        app
    }

    fn reset_term(&mut self) {
        self.grid.reset();
        self.parser = vte::Parser::new();
        self.view.release_all();
    }

    fn ssh(&self) -> Option<&config::Ssh> {
        self.cfg.ssh.as_ref()
    }

    // ---- local ----

    fn start_local(&mut self) {
        self.session = None;
        self.reset_term();
        let Some(local) = &self.cfg.local else { return };

        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let spawn = session::local::Spawn {
                program: &local.program,
                args: &local.args,
                term: &self.cfg.terminal.term,
                cols: self.grid.cols as u16,
                rows: self.grid.rows as u16,
            };
            self.session = Some(session::local::spawn(spawn, self.wake.clone()));
            self.screen = Screen::Running;
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            let error = "running local programs isnt possible on this platform";
            self.screen = Screen::Failed(fill(&self.cfg.text.start_failed, &[("program", &local.program), ("error", error)]));
        }
    }

    // ---- ssh ----

    fn server_of(&self, typed: &str) -> String {
        let preset = self.ssh().and_then(|s| s.server.clone());
        preset.unwrap_or_else(|| typed.trim().to_string())
    }

    fn port(&self) -> u16 {
        self.ssh().map_or(22, |s| s.port)
    }

    /// the saved setup has everything a login needs (what wasnt remembered is empty)
    fn setup_complete(&self) -> bool {
        self.saved
            .as_ref()
            .is_some_and(|s| !self.server_of(&s.server).is_empty() && !s.username.is_empty())
    }

    fn target(&self, server: &str, username: &str, password: Secret, login_shell: bool) -> Result<session::ssh::Target, String> {
        let ssh = self.ssh().ok_or("not an ssh app")?;
        let (host, port) = split_server(server, ssh.port)?;
        let command = match &ssh.shell_toggle {
            Some(_) if login_shell => None,
            _ => ssh.command.clone(),
        };
        Ok(session::ssh::Target {
            host,
            port,
            username: username.to_string(),
            password,
            command,
            term: self.cfg.terminal.term.clone(),
            cols: self.grid.cols as u16,
            rows: self.grid.rows as u16,
            known_hosts: self.store.known_hosts(),
            pinned: ssh.host_keys.clone(),
            accept_new: ssh.accept_new_host_keys,
            connect_timeout: Duration::from_secs(ssh.connect_timeout_secs),
            keepalive: Duration::from_secs(ssh.keepalive_secs),
            keepalive_max: ssh.keepalive_max,
        })
    }

    /// connect button on the login screen
    fn login(&mut self) {
        let text = &self.cfg.text;
        let server = self.server_of(&self.form.server);
        let username = self.form.username.trim().to_string();
        let error = if server.is_empty() {
            Some(text.missing_server.clone())
        } else if username.is_empty() {
            Some(text.missing_username.clone())
        } else if self.pin_field_shown() && !self.form.pin.is_empty() && !state::pin_ok(&self.form.pin) {
            Some(text.bad_pin.clone())
        } else {
            None
        };
        if error.is_some() {
            self.form.error = error;
            self.screen = Screen::Login;
            return;
        }

        let password = Zeroizing::new(self.form.password.to_string());
        match self.target(&server, &username, password, self.form.login_shell) {
            Ok(target) => {
                self.form.error = None;
                self.session = Some(session::ssh::connect(&self.rt, target, self.wake.clone()));
                self.screen = Screen::LoggingIn;
            }
            Err(e) => {
                self.form.error = Some(e);
                self.screen = Screen::Login;
            }
        }
    }

    /// first login worked, remember it (as much as the checkboxes allow) until logout
    fn remember_login(&mut self) {
        let server = self.server_of(&self.form.server);
        let preset = self.ssh().is_some_and(|s| s.server.is_some());
        let kiosk_pin = if self.pin_field_shown() && !self.form.pin.is_empty() {
            Some(state::hash_pin(&self.form.pin))
        } else {
            self.saved.as_ref().and_then(|s| s.kiosk_pin.clone())
        };
        let saved = Saved {
            // a baked in server isnt ours to remember, a new build might change it
            server: if preset { String::new() } else { server.clone() },
            username: self.form.username.trim().to_string(),
            login_shell: self.form.login_shell,
            remember_server: self.form.remember_server,
            remember_username: self.form.remember_username,
            kiosk_pin,
        };

        let password = std::mem::take(&mut self.form.password);
        if let Ok((host, port)) = split_server(&server, self.port()) {
            let account = creds::account(&saved.username, &host, port);
            // without server and user theres nothing to find the password by next launch anyway
            if (preset || saved.remember_server) && saved.remember_username {
                self.creds.set(&account, password.clone());
            } else {
                self.creds.forget(&account);
            }
        }
        self.password = Some(password);
        self.form.pin = Zeroizing::default();
        let _ = self.store.save(&saved);
        self.saved = Some(saved);
    }

    /// new session from the saved setup, screen = what to show meanwhile.
    /// false (and back to login) if theres nothing to connect with
    fn attempt(&mut self, screen: Screen) -> bool {
        self.session = None;
        let Some(saved) = self.saved.clone().filter(|_| self.setup_complete()) else {
            self.back_to_login(None);
            return false;
        };
        let server = self.server_of(&saved.server);
        let target = split_server(&server, self.port()).and_then(|(host, port)| {
            let password = self
                .password
                .clone()
                .or_else(|| self.creds.get(&creds::account(&saved.username, &host, port)))
                .ok_or_else(String::new)?;
            self.target(&server, &saved.username, password, saved.login_shell)
        });
        match target {
            Ok(target) => {
                self.session = Some(session::ssh::connect(&self.rt, target, self.wake.clone()));
                self.screen = screen;
                true
            }
            Err(e) => {
                self.back_to_login((!e.is_empty()).then_some(e));
                false
            }
        }
    }

    fn back_to_login(&mut self, error: Option<String>) {
        self.session = None;
        self.host_key = None;
        self.screen = Screen::Login;
        self.form.error = error;
        self.form.password = Zeroizing::default();
        if self.setup_complete() {
            self.form.focus = login::Field::Password;
        }
        self.form.refocus = true;
    }

    fn forget_password(&mut self) {
        self.password = None;
        if let Some(saved) = &self.saved
            && let Ok((host, port)) = split_server(&self.server_of(&saved.server), self.port())
        {
            self.creds.forget(&creds::account(&saved.username, &host, port));
        }
    }

    /// forgets the password, and unless logout_keeps_setup the server and username with it
    fn logout(&mut self) {
        self.session = None;
        self.forget_password();
        if !self.logout_keeps_setup() {
            self.saved = None;
            self.store.clear();
        }
        self.form = login::Form::new(&self.cfg, self.saved.as_ref());
        self.reset_term();
        self.back_to_login(None);
    }

    fn logout_keeps_setup(&self) -> bool {
        self.ssh().is_some_and(|s| s.logout_keeps_setup)
    }

    fn pin_field_shown(&self) -> bool {
        self.kiosk && self.saved.is_none()
    }

    /// the kiosk pin, if we're a kiosk and one was set
    fn kiosk_pin(&self) -> Option<String> {
        self.saved.as_ref().filter(|_| self.kiosk).and_then(|s| s.kiosk_pin.clone())
    }

    fn server_label(&self) -> String {
        match &self.saved {
            Some(s) if !self.server_of(&s.server).is_empty() => self.server_of(&s.server),
            _ => self.server_of(&self.form.server),
        }
    }

    // ---- changed server keys ----

    fn key_changed(&mut self, change: Option<KeyChange>, new: String, first: bool) {
        let accept_at = self
            .ssh()
            .filter(|s| change.is_some() && s.changed_key_accept && s.changed_key_accept_after_secs > 0)
            .map(|s| Instant::now() + Duration::from_secs(s.changed_key_accept_after_secs));
        self.screen = Screen::KeyChanged(KeyScreen { change, new, first, accept_at, error: None });
    }

    /// someone (or the timeout) said the new key is fine: into known_hosts with it, then connect again
    fn accept_key(&mut self) {
        let Screen::KeyChanged(k) = &mut self.screen else { return };
        let Some(change) = k.change.clone() else { return };
        let first = k.first;
        let server = if first { self.server_of(&self.form.server) } else { self.server_label() };
        let result = split_server(&server, self.port())
            .and_then(|(host, port)| session::ssh::replace_host_key(&host, port, &change.file, &change.new_key));
        if let Err(e) = result {
            if let Screen::KeyChanged(k) = &mut self.screen {
                k.error = Some(e);
                k.accept_at = None;
            }
            return;
        }
        self.retry_key();
    }

    fn retry_key(&mut self) {
        let Screen::KeyChanged(k) = &self.screen else { return };
        if k.first {
            self.login();
        } else {
            self.attempt(Screen::Connecting);
        }
    }

    // ---- events ----

    fn pump(&mut self, ctx: &egui::Context) {
        let events: Vec<Event> = match &self.session {
            Some(s) => s.events().collect(),
            None => Vec::new(),
        };
        for event in events {
            match event {
                Event::Output(bytes) => self.parser.advance(&mut self.grid, &bytes),
                Event::Connected => {
                    if matches!(self.screen, Screen::LoggingIn) {
                        self.remember_login();
                    }
                    self.reset_term();
                    self.screen = Screen::Running;
                }
                Event::HostKey(q) => self.host_key = Some(q),
                Event::Ended(end) => self.ended(end),
            }
        }

        let now = Instant::now();
        let due = match &self.screen {
            Screen::Interrupted { retry_at } | Screen::Lost { retry_at } if self.session.is_none() => Some(*retry_at),
            Screen::KeyChanged(KeyScreen { accept_at: Some(at), .. }) if self.dialog.is_none() => Some(*at),
            _ => None,
        };
        let Some(due) = due else { return };
        if now < due {
            ctx.request_repaint_after((due - now).min(Duration::from_millis(250)));
            return;
        }
        match std::mem::replace(&mut self.screen, Screen::Login) {
            Screen::KeyChanged(k) => {
                self.screen = Screen::KeyChanged(k);
                self.accept_key();
            }
            keep => {
                self.attempt(keep);
            }
        }
    }

    fn ended(&mut self, end: End) {
        self.session = None;
        self.host_key = None;
        let text = self.cfg.text.clone();

        if let Some(local) = &self.cfg.local {
            let program = local.program.clone();
            match end {
                End::NotFound => self.screen = Screen::Failed(fill(&text.program_missing, &[("program", &program)])),
                End::Failed(error) => {
                    self.screen = Screen::Failed(fill(&text.start_failed, &[("program", &program), ("error", &error)]));
                }
                _ => match local.on_exit {
                    LocalOnExit::Close => self.close = true,
                    LocalOnExit::Restart => self.start_local(),
                },
            }
            return;
        }

        let Some(ssh) = self.cfg.ssh.clone() else { return };
        let first = matches!(self.screen, Screen::LoggingIn);
        let server = self.server_label();
        let now = Instant::now();
        match end {
            End::Exited if !first => match ssh.on_exit {
                SshOnExit::Close => self.close = true,
                SshOnExit::Reinitialize => {
                    self.attempt(Screen::Connecting);
                }
                SshOnExit::Login => self.back_to_login(None),
            },
            End::AuthFailed => {
                self.forget_password();
                self.back_to_login(Some(text.auth_failed));
            }
            End::HostKeyDeclined => self.back_to_login(Some(text.host_key_declined)),
            End::HostKeyChanged(change) => {
                let new = change.new.clone();
                self.key_changed(Some(change), new, first);
            }
            End::HostKeyMismatch { new } => self.key_changed(None, new, first),
            End::Failed(error) => self.back_to_login(Some(fill(&text.failed, &[("error", &error)]))),
            End::Unreachable(error) if first => {
                self.back_to_login(Some(fill(&text.unreachable, &[("server", &server), ("error", &error)])))
            }
            End::Lost | End::Exited | End::NotFound if first => {
                self.back_to_login(Some(fill(&text.failed, &[("error", "the server closed the connection")])))
            }
            End::Unreachable(_) => {
                self.screen = Screen::Interrupted { retry_at: now + Duration::from_secs(ssh.resume_every_secs) };
            }
            End::Lost | End::Exited | End::NotFound => {
                self.screen = Screen::Lost { retry_at: now + Duration::from_secs(ssh.reinit_after_secs) };
            }
        }
    }

    // ---- screens ----

    fn login_look(&self, busy: Option<String>, warning: bool) -> login::Look<'_> {
        login::Look {
            cfg: &self.cfg,
            busy,
            locked: self.kiosk && self.setup_complete(),
            show_pin: self.pin_field_shown(),
            warning: (warning && self.unsafe_warning).then_some(self.cfg.text.unsafe_credentials.as_str()),
        }
    }

    fn login_screen(&mut self, ui: &mut egui::Ui) {
        let busy = matches!(self.screen, Screen::LoggingIn)
            .then(|| fill(&self.cfg.text.connecting, &[("server", &self.server_of(&self.form.server))]));
        let (locked, show_pin) = (self.kiosk && self.setup_complete(), self.pin_field_shown());
        // spelled out instead of login_look() so only cfg is borrowed, not the form
        let look = login::Look {
            cfg: &self.cfg,
            busy,
            locked,
            show_pin,
            warning: self.unsafe_warning.then_some(self.cfg.text.unsafe_credentials.as_str()),
        };
        if login::show(ui, &mut self.form, look) {
            self.login();
        }
    }

    fn terminal_screen(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let (cols, rows) = self.view.fit(rect);
        if cols > 0 && (cols as usize != self.grid.cols || rows as usize != self.grid.rows) {
            self.grid.resize(rows as usize, cols as usize);
            if let Some(s) = &self.session {
                s.resize(cols, rows);
            }
        }
        ui.allocate_rect(rect, egui::Sense::hover());
        self.view.paint(ui.painter(), rect, &self.grid, &self.pal);

        let typing = matches!(self.screen, Screen::Running)
            && self.menu.is_none()
            && self.dialog.is_none()
            && self.host_key.is_none();
        if typing {
            let trigger = self.cfg.menu.open_on;
            // banners, the menu, a floating keyboard: clicks there arent for the app
            let ctx = ui.ctx();
            let covered = ctx
                .input(|i| i.pointer.latest_pos())
                .and_then(|pos| ctx.layer_id_at(pos))
                .is_some_and(|layer| layer.order != egui::Order::Background);
            let (view, grid) = (&mut self.view, &self.grid);
            let bytes = ui.input(|i| {
                let skip_secondary = match trigger {
                    MenuTrigger::RightClick => true,
                    MenuTrigger::ShiftRightClick => i.modifiers.shift,
                };
                let mut bytes = keyboard_bytes(i);
                bytes.extend(view.mouse(i, rect, grid, skip_secondary, covered));
                bytes
            });
            if let Some(s) = &self.session {
                s.write(&bytes);
            }
        } else {
            self.view.release_all();
            if !matches!(self.screen, Screen::Running) {
                ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(170));
                self.status_card(ui.ctx(), rect);
            }
        }
    }

    /// the box in the middle for connecting / interrupted / lost / changed key / failed
    fn status_card(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        let t = &self.cfg.text;
        let trying = self.session.is_some();
        let (retry_button, logout_button) = self.ssh().map_or((false, false), |s| (s.retry_button, s.logout_button));
        let can_logout = logout_button && self.saved.is_some();
        let secs_until = |at: &Instant| (at.saturating_duration_since(Instant::now()).as_secs_f32().ceil() as u64).to_string();

        let mut danger = false;
        let mut body_mono = false;
        let mut note = None;
        let mut buttons = Vec::new();
        let (title, body) = match &self.screen {
            Screen::Connecting => {
                if can_logout {
                    buttons.push((CardAction::Logout, t.menu_logout.clone()));
                }
                (fill(&t.connecting, &[("server", &self.server_label())]), String::new())
            }
            Screen::Interrupted { .. } => {
                if retry_button && !trying {
                    buttons.push((CardAction::RetryNow, t.retry_now.clone()));
                }
                if can_logout {
                    buttons.push((CardAction::Logout, t.menu_logout.clone()));
                }
                (t.interrupted_title.clone(), t.interrupted_body.clone())
            }
            Screen::Lost { retry_at } => {
                if retry_button && !trying {
                    buttons.push((CardAction::RetryNow, t.retry_now.clone()));
                }
                if can_logout {
                    buttons.push((CardAction::Logout, t.menu_logout.clone()));
                }
                (t.lost_title.clone(), fill(&t.lost_body, &[("secs", &secs_until(retry_at))]))
            }
            Screen::KeyChanged(k) => {
                danger = true;
                body_mono = true;
                let server = if k.first { self.server_of(&self.form.server) } else { self.server_label() };
                let body = match &k.change {
                    Some(c) => fill(
                        &t.key_changed_body,
                        &[
                            ("server", &server),
                            ("old", &c.old),
                            ("new", &c.new),
                            ("line", &c.line.to_string()),
                            ("file", &c.file.display().to_string()),
                        ],
                    ),
                    None => fill(&t.key_mismatch_body, &[("server", &server), ("new", &k.new)]),
                };
                if k.change.is_some() && self.ssh().is_some_and(|s| s.changed_key_accept) {
                    buttons.push((CardAction::AcceptKey, t.accept_key.clone()));
                }
                if retry_button {
                    buttons.push((CardAction::RetryNow, t.retry_now.clone()));
                }
                if k.first {
                    buttons.push((CardAction::Back, t.cancel.clone()));
                } else if can_logout {
                    buttons.push((CardAction::Logout, t.menu_logout.clone()));
                }
                note = match (&k.error, &k.accept_at) {
                    (Some(e), _) => Some(e.clone()),
                    (None, Some(at)) => Some(fill(&t.accept_key_in, &[("secs", &secs_until(at))])),
                    _ => None,
                };
                (t.key_changed_title.clone(), body)
            }
            Screen::Failed(msg) => {
                buttons.push((CardAction::RetryLocal, t.retry.clone()));
                buttons.push((CardAction::Exit, t.exit.clone()));
                (String::new(), msg.clone())
            }
            _ => return,
        };
        let spinner = matches!(self.screen, Screen::Connecting | Screen::Interrupted { .. }) || trying;

        let mut clicked = None;
        let frame = if danger {
            egui::Frame::window(&ctx.global_style())
                .fill(Color32::from_rgb(60, 8, 8))
                .stroke(egui::Stroke::new(3.0, Color32::from_rgb(220, 40, 40)))
        } else {
            egui::Frame::window(&ctx.global_style())
        };
        egui::Area::new(Id::new("taco_status"))
            .order(egui::Order::Middle)
            .pivot(Align2::CENTER_CENTER)
            .fixed_pos(rect.center())
            .show(ctx, |ui| {
                frame.inner_margin(24.0).show(ui, |ui| {
                    ui.set_width((rect.width() * 0.8).min(if danger { 720.0 } else { 460.0 }));
                    ui.vertical_centered(|ui| {
                        if !title.is_empty() {
                            let title = RichText::new(title).size(if danger { 26.0 } else { 22.0 }).strong();
                            ui.label(if danger { title.color(RED) } else { title });
                        }
                        if !body.is_empty() {
                            ui.add_space(6.0);
                            let body = RichText::new(body).size(if body_mono { 14.0 } else { 16.0 });
                            ui.label(if body_mono { body.monospace() } else { body });
                        }
                        if let Some(note) = note {
                            ui.add_space(6.0);
                            ui.label(RichText::new(note).size(16.0).color(RED));
                        }
                        if spinner {
                            ui.add_space(8.0);
                            ui.add(egui::Spinner::new().size(24.0));
                        }
                        if !buttons.is_empty() {
                            ui.add_space(10.0);
                            let key = Id::new("taco_status_buttons");
                            let last: Option<f32> = ui.ctx().data(|d| d.get_temp(key));
                            ui.horizontal_wrapped(|ui| {
                                ui.add_space(((ui.available_width() - last.unwrap_or(0.0)) / 2.0).max(0.0));
                                let row = ui.scope(|ui| {
                                    for (action, label) in &buttons {
                                        if ui.button(RichText::new(label).size(16.0)).clicked() {
                                            clicked = Some(*action);
                                        }
                                    }
                                });
                                let width = row.response.rect.width();
                                if last != Some(width) {
                                    ui.ctx().data_mut(|d| d.insert_temp(key, width));
                                    ui.ctx().request_discard("status card buttons measured");
                                }
                            });
                        }
                    });
                });
            });

        match clicked {
            Some(CardAction::RetryNow) => match &mut self.screen {
                Screen::Lost { retry_at } | Screen::Interrupted { retry_at } => {
                    *retry_at = Instant::now();
                    ctx.request_repaint();
                }
                Screen::KeyChanged(_) => self.retry_key(),
                _ => {}
            },
            Some(CardAction::RetryLocal) => self.start_local(),
            Some(CardAction::Logout) => self.dialog = Some(Dialog::Logout { pin: String::new(), wrong: false }),
            Some(CardAction::Back) => self.back_to_login(None),
            Some(CardAction::AcceptKey) => self.with_pin(AfterPin::AcceptKey),
            Some(CardAction::Exit) => self.close = true,
            None => {}
        }
    }

    /// a strip along the top, one line each. above the content rather than over it,
    /// so it never hides a bit of the app. the login screen has its own line for the unsafe one
    fn banners(&mut self, ui: &mut egui::Ui) {
        let login = matches!(self.screen, Screen::Login | Screen::LoggingIn);
        if !self.banners.iter().any(|b| b.on_login || !login) {
            return;
        }
        let mut closed = None;
        egui::Panel::top("taco_banners")
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(90, 60, 0)).inner_margin(egui::vec2(8.0, 4.0)))
            .show(ui, |ui| {
                for (i, banner) in self.banners.iter().enumerate() {
                    if login && !banner.on_login {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if ui.button("×").clicked() {
                            closed = Some(i);
                        }
                        ui.label(RichText::new(&banner.text).color(Color32::from_rgb(255, 220, 140)));
                    });
                }
            });
        if let Some(i) = closed {
            self.banners.remove(i);
        }
    }

    // ---- menu and dialogs ----

    fn menu_items(&self) -> Vec<MenuItem> {
        if self.cfg.local.is_some() {
            return vec![MenuItem::Restart];
        }
        let login = matches!(self.screen, Screen::Login | Screen::LoggingIn);
        let mut items = Vec::new();
        if self.saved.is_some() && !login {
            items.push(MenuItem::Reinitialize);
        }
        if self.saved.is_some() {
            items.push(MenuItem::Logout);
        }
        if login {
            items.push(MenuItem::Exit);
        }
        items
    }

    fn open_menu(&mut self, pos: Pos2, pivot: Align2) {
        if !self.menu_items().is_empty() {
            self.menu = Some(Menu { pos, pivot, fresh: true });
        }
    }

    fn right_click(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.host_key.is_some() {
            return;
        }
        let (pressed, shift, pos) = ctx.input(|i| {
            (i.pointer.button_pressed(egui::PointerButton::Secondary), i.modifiers.shift, i.pointer.interact_pos())
        });
        let wanted = match self.cfg.menu.open_on {
            MenuTrigger::RightClick => pressed,
            MenuTrigger::ShiftRightClick => pressed && shift,
        };
        if let (true, Some(pos)) = (wanted, pos) {
            self.open_menu(pos, Align2::LEFT_TOP);
        }
    }

    fn menu_popup(&mut self, ctx: &egui::Context) {
        let items = self.menu_items();
        let touch = self.bar.is_some();
        let Some(menu) = &mut self.menu else { return };
        let t = &self.cfg.text;

        let mut picked = None;
        let area = egui::Area::new(Id::new("taco_menu"))
            .order(egui::Order::Foreground)
            .pivot(menu.pivot)
            .fixed_pos(menu.pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    let width = if touch { 240.0 } else { 170.0 };
                    ui.set_width(width);
                    for item in items {
                        let label = match item {
                            MenuItem::Restart => &t.menu_restart,
                            MenuItem::Reinitialize => &t.menu_reinitialize,
                            MenuItem::Logout => &t.menu_logout,
                            MenuItem::Exit => &t.menu_exit,
                        };
                        let label = RichText::new(label).size(if touch { 20.0 } else { 15.0 });
                        let button = egui::Button::new(label).frame(false).min_size(egui::vec2(width, 0.0));
                        if ui.add(button).clicked() {
                            picked = Some(item);
                        }
                    }
                });
            })
            .response;

        let dismissed = !menu.fresh
            && ctx.input(|i| (i.pointer.any_pressed() && !area.contains_pointer()) || i.key_pressed(egui::Key::Escape));
        menu.fresh = false;
        if picked.is_some() || dismissed {
            self.menu = None;
        }
        match picked {
            Some(MenuItem::Restart) => self.start_local(),
            Some(MenuItem::Reinitialize) => {
                self.attempt(Screen::Connecting);
            }
            Some(MenuItem::Logout) => self.dialog = Some(Dialog::Logout { pin: String::new(), wrong: false }),
            Some(MenuItem::Exit) => self.with_pin(AfterPin::Exit),
            None => {}
        }
    }

    /// do it now, or after the kiosk pin if theres one
    fn with_pin(&mut self, then: AfterPin) {
        if self.kiosk_pin().is_some() {
            self.dialog = Some(Dialog::Pin { then, pin: String::new(), wrong: false });
        } else {
            self.after_pin(then);
        }
    }

    fn after_pin(&mut self, then: AfterPin) {
        match then {
            AfterPin::Exit => self.close = true,
            AfterPin::AcceptKey => self.accept_key(),
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else { return };
        let stored_pin = self.kiosk_pin();
        let keeps_setup = self.logout_keeps_setup();
        let t = &self.cfg.text;
        let (mut confirm, mut cancel) = (false, false);

        let (is_logout, then, mut pin, mut wrong) = match dialog {
            Dialog::Logout { pin, wrong } => (true, None, pin, wrong),
            Dialog::Pin { then, pin, wrong } => (false, Some(then), pin, wrong),
        };
        let modal = egui::Modal::new(Id::new("taco_dialog")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.vertical_centered(|ui| {
                if is_logout {
                    ui.heading(&t.logout_title);
                    ui.label(if keeps_setup { &t.logout_keeps_setup_body } else { &t.logout_body });
                    ui.add_space(8.0);
                }
                if stored_pin.is_some() {
                    let title = match then {
                        Some(AfterPin::AcceptKey) => &t.pin_to_accept,
                        _ => &t.pin_title,
                    };
                    ui.label(title);
                    // a space while empty so the line keeps its height
                    let dots = if pin.is_empty() { " ".to_string() } else { "•".repeat(pin.len()) };
                    ui.label(RichText::new(dots).monospace().size(26.0));
                    if wrong {
                        ui.label(RichText::new(&t.pin_wrong).color(RED));
                    }
                    ui.add_space(4.0);
                    // a grid doesnt center itself, so it gets a box exactly its width and the box gets centered
                    let layout = egui::Layout::top_down(egui::Align::Min);
                    ui.allocate_ui_with_layout(egui::vec2(PIN_PAD_WIDTH, 0.0), layout, |ui| {
                        confirm |= pin_pad(ui, &mut pin, &t.ok);
                    });
                    ui.add_space(8.0);
                }
                // buttons share the pads width between them, so they line up with it
                let labels: Vec<&String> = if is_logout { vec![&t.logout_confirm, &t.cancel] } else { vec![&t.cancel] };
                let width = (PIN_PAD_WIDTH - PIN_GAP * (labels.len() - 1) as f32) / labels.len() as f32;
                let height = ui.spacing().interact_size.y;
                let layout = egui::Layout::left_to_right(egui::Align::Min);
                ui.allocate_ui_with_layout(egui::vec2(PIN_PAD_WIDTH, height), layout, |ui| {
                    ui.spacing_mut().item_spacing.x = PIN_GAP;
                    for (i, label) in labels.iter().enumerate() {
                        let clicked = ui.add_sized([width, height], egui::Button::new(label.as_str())).clicked();
                        match (is_logout, i) {
                            (true, 0) => confirm |= clicked,
                            _ => cancel |= clicked,
                        }
                    }
                });
            });
        });
        cancel |= modal.should_close();

        if cancel {
            return;
        }
        if confirm {
            match &stored_pin {
                Some(stored) if !state::check_pin(stored, &pin) => {
                    wrong = true;
                    pin.clear();
                }
                _ => {
                    return match then {
                        Some(then) => self.after_pin(then),
                        None => self.logout(),
                    };
                }
            }
        }
        self.dialog = Some(match then {
            Some(then) => Dialog::Pin { then, pin, wrong },
            None => Dialog::Logout { pin, wrong },
        });
    }

    fn host_key_dialog(&mut self, ctx: &egui::Context) {
        let Some(q) = &self.host_key else { return };
        let t = &self.cfg.text;
        let body = fill(&t.host_key_body, &[("server", &self.server_label()), ("fingerprint", &q.fingerprint)]);
        let mut answer = None;
        let modal = egui::Modal::new(Id::new("taco_host_key")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.heading(&t.host_key_title);
            ui.add_space(4.0);
            ui.label(RichText::new(body).monospace());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(&t.trust).clicked() {
                    answer = Some(true);
                }
                if ui.button(&t.cancel).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        // only take the question once theres an answer, dropping it counts as a no
        if let Some(trust) = answer
            && let Some(q) = self.host_key.take()
        {
            q.answer(trust);
        }
    }

    // ---- on screen keyboard ----

    fn osk(&mut self, press: kiosk::Press) {
        use kiosk::Press;

        if let Some(Dialog::Logout { pin, .. } | Dialog::Pin { pin, .. }) = &mut self.dialog {
            match press {
                Press::Text(s) if s.chars().all(|c| c.is_ascii_digit()) => pin.push_str(&s),
                Press::Special(n) if n == "bksp" => {
                    pin.pop();
                }
                _ => {}
            }
            return;
        }
        if self.host_key.is_some() || self.menu.is_some() {
            return;
        }

        match self.screen {
            Screen::Login => {
                let fields = self.login_look(None, false).fields();
                if !fields.contains(&self.form.focus) {
                    self.form.focus = fields[0];
                }
                let focus = self.form.focus;
                match press {
                    Press::Text(s) => self.form.field_mut(focus).push_str(&s),
                    Press::Special(n) => match n.as_str() {
                        "bksp" => {
                            self.form.field_mut(focus).pop();
                        }
                        "space" => self.form.field_mut(focus).push(' '),
                        "tab" => {
                            let at = fields.iter().position(|f| *f == focus).unwrap_or(0);
                            self.form.focus = fields[(at + 1) % fields.len()];
                        }
                        "enter" => self.login(),
                        _ => {}
                    },
                    Press::Ctrl(_) => {}
                }
                self.form.refocus = true;
            }
            Screen::Running => {
                let bytes = match press {
                    Press::Text(s) => s.into_bytes(),
                    Press::Ctrl(c) => ctrl_char_byte(c).map(|b| vec![b]).unwrap_or_default(),
                    Press::Special(n) => named_key_bytes(&n).unwrap_or_default(),
                };
                if let Some(s) = &self.session {
                    s.write(&bytes);
                }
            }
            _ => {}
        }
    }

    fn remember_window(&mut self, ctx: &egui::Context) {
        let Some(memory) = &mut self.window else { return };
        let size = ctx.input(|i| {
            let v = i.viewport();
            let stretched = [v.fullscreen, v.maximized, v.minimized].into_iter().any(|s| s == Some(true));
            let rect = v.inner_rect.filter(|_| !stretched)?;
            Some(WindowSize { width: rect.width(), height: rect.height() })
        });
        if let Some(wait) = memory.seen(size, Instant::now()) {
            ctx.request_repaint_after(wait);
        }
    }
}

impl eframe::App for TacoApp {
    // runs even while the window is hidden, so output never piles up
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump(ctx);
        self.remember_window(ctx);
        if self.close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.view.measure(&ctx);

        // panels first, the central panel gets what they leave
        self.banners(ui);
        let mut kiosk_out = kiosk::Output::default();
        if let Some(k) = &mut self.bar {
            kiosk_out = k.show(ui, &self.cfg.text);
        }
        if let Some((pos, pivot)) = kiosk_out.menu {
            self.open_menu(pos, pivot);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(self.pal.bg))
            .show(ui, |ui| match self.screen {
                Screen::Login | Screen::LoggingIn => self.login_screen(ui),
                _ => self.terminal_screen(ui),
            });

        for press in kiosk_out.presses {
            self.osk(press);
        }
        self.right_click(&ctx);
        self.menu_popup(&ctx);
        self.host_key_dialog(&ctx);
        self.dialogs(&ctx);
        // the osk does the typing on phones, a focused login field would otherwise pop the system keyboard up over it
        #[cfg(any(target_os = "ios", target_os = "android"))]
        ctx.output_mut(|o| o.ime = None);
        if self.close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    // a resize right before quitting hasnt settled yet
    fn on_exit(&mut self) {
        if let Some(memory) = &mut self.window {
            memory.flush();
        }
    }
}

/// digits 1-9, bksp, 0, ok. true = ok pressed (or enter)
fn pin_pad(ui: &mut egui::Ui, pin: &mut String, ok_label: &str) -> bool {
    let mut ok = false;
    // real keyboards work too
    ui.input(|i| {
        for e in &i.events {
            match e {
                egui::Event::Text(t) => pin.extend(t.chars().filter(|c| c.is_ascii_digit())),
                egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => {
                    pin.pop();
                }
                egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => ok = true,
                _ => {}
            }
        }
    });
    egui::Grid::new("taco_pin_pad").spacing([PIN_GAP, PIN_GAP]).show(ui, |ui| {
        for row in [["1", "2", "3"], ["4", "5", "6"], ["7", "8", "9"], ["⇐", "0", "ok"]] {
            for key in row {
                let label = if key == "ok" { ok_label } else { key };
                let button = egui::Button::new(RichText::new(label).size(20.0)).min_size(PIN_KEY);
                if ui.add(button).clicked() {
                    match key {
                        "⇐" => {
                            pin.pop();
                        }
                        "ok" => ok = true,
                        digit => pin.push_str(digit),
                    }
                }
            }
            ui.end_row();
        }
    });
    pin.truncate(16);
    ok
}

fn keyboard_bytes(input: &egui::InputState) -> Vec<u8> {
    let mut out = Vec::new();
    for event in &input.events {
        match event {
            egui::Event::Text(text) => {
                // only pass printable chars (also partial workaround for crashing on none ascii input like this: ü ö ä)
                let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
                out.extend(filtered.as_bytes());
            }
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                if modifiers.ctrl || modifiers.mac_cmd {
                    if let Some(byte) = ctrl_key_byte(key) {
                        out.push(byte);
                    }
                } else if let Some(bytes) = special_key_bytes(key, modifiers) {
                    out.extend(bytes);
                }
            }
            // egui turns ctrl+c/x/v into clipboard events and swallows the key,
            // so hand them back as the control chars a tui expects
            egui::Event::Copy => out.push(0x03),
            egui::Event::Cut => out.push(0x18),
            egui::Event::Paste(text) => out.extend(text.replace('\n', "\r").as_bytes()),
            _ => {}
        }
    }
    out
}
