// please send help ... this is the central config, one toml file that decides everything
//
// this file actually gets compiled twice: once by build.rs 
// (which reads and checks the toml and bakes it into the binary, so a broken config fails the build and not the app)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub app: App,
    #[serde(default)]
    pub terminal: Terminal,
    /// wrap a program on this machine
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<Local>,
    /// or log into a server over ssh and run it there
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<Ssh>,
    #[serde(default)]
    pub login: Login,
    #[serde(default)]
    pub menu: Menu,
    #[serde(default)]
    pub kiosk: Kiosk,
    #[serde(default)]
    pub text: Text,
    #[serde(default)]
    pub packaging: Packaging,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    /// window title and the name in app menus
    pub name: String,
    /// reverse dns id, names the settings folder and the macos bundle
    pub id: String,
    /// what the installed binary gets called
    #[serde(default = "default_binary")]
    pub binary: String,
    /// your app's version, for the package and --version. defaults to tacoshells own version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub description: String,
    /// png, relative to the config file. build.rs embeds it and clears this
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// what `make deb` / `make pkg` write into the package, nothing the app itself actually uses but debian needs to shut the fuck up
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Packaging {
    /// Use this format "Name <mail@example.com>". defaults to git user.name / user.email of whoever builds the package if unset
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maintainer: Option<String>,
    /// license of your app, SPDX name ("MIT", "GPL-3.0-or-later")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// a few yap lines for package managers. default: app.description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Terminal {
    pub font_size: f32,
    /// starting window size in cells, the window can be resized after
    /// todo : should make use eframes ability to rememeber window size between sessions
    pub cols: u16,
    pub rows: u16,
    pub foreground: String,
    pub background: String,
    pub term: String,
}

impl Default for Terminal {
    fn default() -> Self {
        Terminal {
            font_size: 14.0,
            cols: 120,
            rows: 35,
            foreground: "#cccccc".into(),
            background: "#181818".into(),
            term: "xterm-256color".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Local {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub on_exit: LocalOnExit,
    /// debian packages `program` comes from, `make deb` puts them in Depends
    #[serde(default)]
    pub depends: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalOnExit {
    #[default]
    Close,
    Restart,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ssh {
    /// "host" or "host:port"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// used when the server has no port of its own
    pub port: u16,
    /// run this instead of the users login shell
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// checkbox on the setup screen: checked = login shell, unchecked = run `command`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_toggle: Option<ShellToggle>,
    pub host_keys: Vec<String>,
    pub accept_new_host_keys: bool,
    pub changed_key_accept: bool,
    pub changed_key_accept_after_secs: u64,
    pub retry_button: bool,
    pub logout_button: bool,
    pub connect_timeout_secs: u64,
    /// after this many seconds of silence we poke the server
    pub keepalive_secs: u64,
    /// and after this many unanswered pokes the connection counts as interrupted
    pub keepalive_max: usize,
    pub resume_every_secs: u64,
    /// countdown before reinitializing a session that got killed
    pub reinit_after_secs: u64,
    pub on_exit: SshOnExit,
    pub credentials: Credentials,
    pub unsafe_warning: bool,
}

impl Default for Ssh {
    fn default() -> Self {
        Ssh {
            server: None,
            port: 22,
            command: None,
            shell_toggle: None,
            host_keys: Vec::new(),
            accept_new_host_keys: false,
            changed_key_accept: true,
            changed_key_accept_after_secs: 0,
            retry_button: true,
            logout_button: true,
            connect_timeout_secs: 10,
            keepalive_secs: 5,
            keepalive_max: 3,
            resume_every_secs: 3,
            reinit_after_secs: 5,
            on_exit: SshOnExit::Close,
            credentials: Credentials::Memory,
            unsafe_warning: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellToggle {
    pub label: String,
    #[serde(default = "yes")]
    pub default: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshOnExit {
    #[default]
    Close,
    Reinitialize,
    Login,
}

/// where the password lives. an os keyring goes here later
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Credentials {
    /// only while the app runs, relaunching asks again
    #[default]
    Memory,
    /// base64x2 "security" ... yea ... i was lazy will do better one day maybe. TODO
    Unsafe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Login {
    /// ascii art above the login form
    pub logo: String,
    /// read from file instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_file: Option<String>,
    /// logo color, defaults to the terminal foreground
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_color: Option<String>,
    pub title: String,
    /// "remember server" checkbox. without it the server is always remembered
    pub remember_server_checkbox: bool,
    /// "remember username" checkbox. same deal
    pub remember_username_checkbox: bool,
}

impl Default for Login {
    fn default() -> Self {
        Login {
            logo: String::new(),
            logo_file: None,
            logo_color: None,
            title: String::new(),
            remember_server_checkbox: true,
            remember_username_checkbox: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Menu {
    pub open_on: MenuTrigger,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuTrigger {
    /// right click opens the menu, the app never sees right clicks
    #[default]
    RightClick,
    /// right click goes to the app, shift + right click opens the menu
    ShiftRightClick,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Kiosk {
    /// always kiosk. otherwise only with --kiosk
    pub enabled: bool,
    pub fullscreen: bool,
    /// shrink the terminal to make room for the keyboard, or put it over the terminal
    pub keyboard_mode: KeyboardMode,
    /// keyboard rows, keys split by spaces. {name} is a special key, see KEY_NAMES
    pub keyboard: Vec<String>,
    /// same rows with shift held, must line up key for key with `keyboard`
    pub keyboard_shift: Vec<String>,
}

impl Default for Kiosk {
    /// Yea ... this is how I did the keyboard layout ... sorry
    fn default() -> Self {
        let rows = |r: &[&str]| r.iter().map(|s| s.to_string()).collect();
        Kiosk {
            enabled: false,
            fullscreen: true,
            keyboard_mode: KeyboardMode::Shrink,
            keyboard: rows(&[
                "{esc} 1 2 3 4 5 6 7 8 9 0 - = {bksp}",
                "{tab} q w e r t y u i o p [ ] \\",
                "{ctrl} a s d f g h j k l ; ' {enter}",
                "{shift} z x c v b n m , . / {up} {del}",
                "` {space} {left} {down} {right}",
            ]),
            keyboard_shift: rows(&[
                "{esc} ! @ # $ % ^ & * ( ) _ + {bksp}",
                "{tab} Q W E R T Y U I O P { } |",
                "{ctrl} A S D F G H J K L : \" {enter}",
                "{shift} Z X C V B N M < > ? {pgup} {del}",
                "~ {space} {home} {pgdn} {end}",
            ]),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardMode {
    /// in this mode the terminal gets smaller while the keyboard is up
    #[default]
    Shrink,
    /// in this mode the keyboard floats over the bottom of the terminal, which keeps terminals size.
    Overlay,
}

/// Todo offer multple keyboard layouts/input methods for situations like perhaps a cash register that doesnt need a full QWERTY layout but a custom numeric keypad towards the side perhaps

/// special keys the on screen keyboard knows, written as {name}
pub const KEY_NAMES: &[&str] = &[
    "esc", "tab", "bksp", "enter", "shift", "ctrl", "space", "up", "down", "left", "right",
    "home", "end", "pgup", "pgdn", "del", "ins", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8",
    "f9", "f10", "f11", "f12",
];

/// btw alot of the OSK was written quickly by an llm because I wanted to clean up, then got distracted finishing this to use my own task management system lmao

/// one key of the on screen keyboard
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OskKey {
    Char(String),
    Special(String),
}

/// "{name}" is special if name is on the list, everything else is typed as is
/// (so a lone "{" still types a brace)
pub fn parse_osk_row(row: &str) -> Result<Vec<OskKey>, String> {
    row.split_whitespace()
        .map(|tok| {
            if tok.len() > 2 && tok.starts_with('{') && tok.ends_with('}') {
                let name = &tok[1..tok.len() - 1];
                if KEY_NAMES.contains(&name) {
                    Ok(OskKey::Special(name.to_string()))
                } else {
                    Err(format!("unknown key {tok}, known ones: {}", KEY_NAMES.join(", ")))
                }
            } else {
                Ok(OskKey::Char(tok.to_string()))
            }
        })
        .collect()
}

/// every string the app shows. {placeholders} get filled in where it says so
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Text {
    pub server: String,
    pub username: String,
    pub password: String,
    pub kiosk_pin: String,
    pub connect: String,
    /// {server}
    pub connecting: String,
    pub missing_server: String,
    pub missing_username: String,
    pub bad_pin: String,
    /// {server} {error}
    pub unreachable: String,
    pub auth_failed: String,
    /// {error}
    pub failed: String,
    pub host_key_title: String,
    /// {server} {fingerprint}
    pub host_key_body: String,
    pub trust: String,
    pub cancel: String,
    pub host_key_declined: String,
    pub key_changed_title: String,
    /// {server} {old} {new} {line} {file}
    pub key_changed_body: String,
    /// {server} {new}, for keys pinned in the config
    pub key_mismatch_body: String,
    pub accept_key: String,
    /// {secs}
    pub accept_key_in: String,
    pub pin_to_accept: String,
    pub interrupted_title: String,
    pub interrupted_body: String,
    pub lost_title: String,
    /// {secs}
    pub lost_body: String,
    pub retry_now: String,
    /// {program} {error}
    pub start_failed: String,
    /// {program}. local mode, put install instructions here
    pub program_missing: String,
    pub retry: String,
    pub exit: String,
    pub menu_restart: String,
    pub menu_reinitialize: String,
    pub menu_logout: String,
    pub menu_exit: String,
    pub logout_title: String,
    pub logout_body: String,
    pub logout_confirm: String,
    pub pin_title: String,
    pub pin_wrong: String,
    pub ok: String,
    pub kiosk_menu: String,
    pub kiosk_keyboard: String,
    pub unsafe_credentials: String,
    pub remember_server: String,
    pub remember_username: String,
}

impl Default for Text {
    fn default() -> Self {
        Text {
            server: "Server".into(),
            username: "Username".into(),
            password: "Password".into(),
            kiosk_pin: "Kiosk PIN (optional)".into(),
            connect: "Connect".into(),
            connecting: "Connecting to {server}...".into(),
            missing_server: "Enter a server".into(),
            missing_username: "Enter a username".into(),
            bad_pin: "The PIN has to be 4 to 16 digits".into(),
            unreachable: "Can't reach {server}: {error}".into(),
            auth_failed: "Wrong username or password".into(),
            failed: "Connection failed: {error}".into(),
            host_key_title: "New server".into(),
            host_key_body: "This is the first time connecting to {server}.\nMake sure its key fingerprint is:\n\n{fingerprint}".into(),
            trust: "Trust".into(),
            cancel: "Cancel".into(),
            host_key_declined: "Server not trusted".into(),
            key_changed_title: "WARNING: THE SERVER'S IDENTITY HAS CHANGED".into(),
            key_changed_body: "{server} identified itself with a different key than last time.\n\
                Someone could be listening in on this connection (a man in the middle attack),\n\
                or the server was reinstalled. Don't continue unless you know which one it is.\n\n\
                Known key: {old}\n\
                New key:   {new}".into(),
            key_mismatch_body: "{server} identified itself with a key this app doesn't trust.\n\
                Someone could be listening in on this connection (a man in the middle attack).\n\n\
                Key: {new}".into(),
            accept_key: "Accept new key".into(),
            accept_key_in: "Accepting the new key in {secs}s".into(),
            pin_to_accept: "Enter PIN to accept the new key".into(),
            interrupted_title: "Connection Interrupted...".into(),
            interrupted_body: "Trying to resume connection".into(),
            lost_title: "Connection Lost...".into(),
            lost_body: "Reinitializing in {secs}s".into(),
            retry_now: "Retry now".into(),
            start_failed: "Couldn't start {program}: {error}".into(),
            program_missing: "Couldn't find {program}.\nIt has to be installed for this app to work.".into(),
            retry: "Retry".into(),
            exit: "Exit".into(),
            menu_restart: "Restart".into(),
            menu_reinitialize: "Reinitialize".into(),
            menu_logout: "Logout".into(),
            menu_exit: "Exit".into(),
            logout_title: "Logout?".into(),
            logout_body: "This forgets the server and login, you'll have to set it up again.".into(),
            logout_confirm: "Logout".into(),
            pin_title: "Enter PIN".into(),
            pin_wrong: "Wrong PIN".into(),
            ok: "OK".into(),
            kiosk_menu: "Menu".into(),
            kiosk_keyboard: "Keyboard".into(),
            unsafe_credentials: "Passwords are saved unprotected on this device (development build)".into(),
            remember_server: "Remember server".into(),
            remember_username: "Remember username".into(),
        }
    }
}

/// what app.binary is when nobody changed it. builds with it get warned at, a
/// shipped app shouldnt be called tacoshell
pub const DEFAULT_BINARY: &str = "tacoshell";

/// shown in the app when app.binary is still DEFAULT_BINARY. not in [text] on purpose,
/// it isnt meant to be switched off, only dismissed
pub const DEFAULT_BINARY_WARNING: &str = "Please use a different binary name than tacoshell! \
If you don't know what this is about, contact the maintainer of your application.";

fn default_binary() -> String {
    DEFAULT_BINARY.into()
}

fn yes() -> bool {
    true
}

/// "#rrggbb" -> [r, g, b]
pub fn parse_color(s: &str) -> Result<[u8; 3], String> {
    let hex = s
        .strip_prefix('#')
        .filter(|h| h.len() == 6 && h.is_ascii())
        .ok_or_else(|| format!("color {s:?} should look like #rrggbb"))?;
    let byte = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| format!("color {s:?} isnt hex"))
    };
    Ok([byte(0)?, byte(2)?, byte(4)?])
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.app.name.trim().is_empty() {
            return Err("app.name is empty".into());
        }
        if self.app.id.trim().is_empty() || self.app.id.contains(['/', '\\']) {
            return Err("app.id has to be something like \"ch.example.app\"".into());
        }
        // it ends up as a debian package name and a file name, keep it boring
        let binary_ok = self.app.binary.len() >= 2
            && self.app.binary.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && self.app.binary.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+-.".contains(c));
        if !binary_ok {
            return Err("app.binary: lowercase letters, digits, + - . only (it's the debian package name too)".into());
        }
        // debian and macos both have opinions, this keeps to what both take
        if let Some(v) = &self.app.version {
            let ok = v.starts_with(|c: char| c.is_ascii_digit())
                && v.chars().all(|c| c.is_ascii_alphanumeric() || ".+~-".contains(c));
            if !ok {
                return Err(format!("app.version {v:?} should look like \"1.2.3\" (starts with a digit, no spaces)"));
            }
        }
        if self.app.description.contains('\n') {
            return Err("app.description is one line, packaging.long_description takes more".into());
        }
        if let Some(m) = &self.packaging.maintainer {
            let shaped = m.find('<').is_some_and(|i| i > 0 && m.trim_end().ends_with('>') && m.contains('@'));
            if !shaped {
                return Err(format!("packaging.maintainer {m:?} should look like \"Name <mail@example.com>\""));
            }
        }
        match (&self.local, &self.ssh) {
            (Some(_), Some(_)) => return Err("pick one of [local] or [ssh], not both".into()),
            (None, None) => return Err("needs either a [local] or an [ssh] section".into()),
            _ => {}
        }
        if let Some(local) = &self.local
            && local.program.trim().is_empty() {
                return Err("local.program is empty".into());
            }
        if let Some(ssh) = &self.ssh {
            if ssh.shell_toggle.is_some() && ssh.command.is_none() {
                return Err("ssh.shell_toggle needs ssh.command, thats what runs when its unchecked".into());
            }
            if let Some(server) = &ssh.server {
                split_server(server, ssh.port)?;
            }
            for fp in &ssh.host_keys {
                if !fp.starts_with("SHA256:") {
                    return Err(format!("ssh.host_keys: {fp:?} should start with SHA256:"));
                }
            }
            if ssh.changed_key_accept_after_secs > 0 && !ssh.changed_key_accept {
                return Err("ssh.changed_key_accept_after_secs needs ssh.changed_key_accept".into());
            }
            if ssh.keepalive_secs == 0 || ssh.connect_timeout_secs == 0 {
                return Err("ssh timeouts have to be at least 1 second".into());
            }
        }
        let t = &self.terminal;
        if !(6.0..=72.0).contains(&t.font_size) {
            return Err("terminal.font_size should be between 6 and 72".into());
        }
        if t.cols < 20 || t.rows < 5 {
            return Err("terminal.cols/rows too small".into());
        }
        parse_color(&t.foreground)?;
        parse_color(&t.background)?;
        if let Some(c) = &self.login.logo_color {
            parse_color(c)?;
        }
        if self.kiosk.keyboard.len() != self.kiosk.keyboard_shift.len() {
            return Err("kiosk.keyboard and kiosk.keyboard_shift need the same rows".into());
        }
        for (i, (plain, shifted)) in self.kiosk.keyboard.iter().zip(&self.kiosk.keyboard_shift).enumerate() {
            let (plain, shifted) = (parse_osk_row(plain)?, parse_osk_row(shifted)?);
            if plain.len() != shifted.len() {
                return Err(format!("kiosk keyboard row {} has a different number of keys with shift", i + 1));
            }
        }
        Ok(())
    }
}

/// "host", "host:port", "[v6]:port" -> (host, port)
pub fn split_server(server: &str, default_port: u16) -> Result<(String, u16), String> {
    let server = server.trim();
    let server = server.strip_prefix("ssh://").unwrap_or(server);
    if server.is_empty() {
        return Err("server is empty".into());
    }
    let bad_port = |p: &str| format!("{p:?} isnt a port");
    if let Some(rest) = server.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or("missing ] in server")?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().map_err(|_| bad_port(p))?,
            None if after.is_empty() => default_port,
            None => return Err(format!("junk after ] in {server:?}")),
        };
        return Ok((host.to_string(), port));
    }
    match server.rsplit_once(':') {
        // more than one colon and no brackets is a bare ipv6 address
        Some((host, port)) if !host.contains(':') => {
            Ok((host.to_string(), port.parse().map_err(|_| bad_port(port))?))
        }
        _ => Ok((server.to_string(), default_port)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal(extra: &str) -> Result<Config, String> {
        let cfg: Config = toml::from_str(&format!("[app]\nname = \"x\"\nid = \"ch.x\"\n{extra}"))
            .map_err(|e| e.to_string())?;
        cfg.validate().map(|_| cfg)
    }

    #[test]
    fn servers() {
        assert_eq!(split_server("host", 22), Ok(("host".into(), 22)));
        assert_eq!(split_server(" host:2222 ", 22), Ok(("host".into(), 2222)));
        assert_eq!(split_server("ssh://host:1", 22), Ok(("host".into(), 1)));
        assert_eq!(split_server("[::1]:2200", 22), Ok(("::1".into(), 2200)));
        assert_eq!(split_server("[::1]", 22), Ok(("::1".into(), 22)));
        assert_eq!(split_server("::1", 22), Ok(("::1".into(), 22)));
        assert!(split_server("host:nope", 22).is_err());
        assert!(split_server("", 22).is_err());
    }

    #[test]
    fn needs_exactly_one_mode() {
        assert!(minimal("").is_err());
        assert!(minimal("[local]\nprogram = \"sh\"\n[ssh]\n").is_err());
        assert!(minimal("[local]\nprogram = \"sh\"\n").is_ok());
        assert!(minimal("[ssh]\n").is_ok());
    }

    #[test]
    fn typos_fail() {
        assert!(minimal("[ssh]\nservr = \"x\"\n").is_err());
        assert!(minimal("[ssh]\n[text]\nconect = \"x\"\n").is_err());
    }

    #[test]
    fn shell_toggle_needs_command() {
        assert!(minimal("[ssh]\nshell_toggle = { label = \"x\" }\n").is_err());
        assert!(minimal("[ssh]\ncommand = \"app\"\nshell_toggle = { label = \"x\" }\n").is_ok());
    }

    #[test]
    fn bad_colors_and_keys() {
        assert!(minimal("[ssh]\n[terminal]\nforeground = \"red\"\n").is_err());
        assert!(minimal("[ssh]\n[kiosk]\nkeyboard = [\"{nope}\"]\nkeyboard_shift = [\"a\"]\n").is_err());
        assert!(minimal("[ssh]\n[kiosk]\nkeyboard = [\"a b\"]\nkeyboard_shift = [\"A\"]\n").is_err());
        assert!(minimal("[ssh]\n[kiosk]\nkeyboard = [\"a {\"]\nkeyboard_shift = [\"A }\"]\n").is_ok());
    }

    /// tacoshell.toml is the reference, every option has to show up in it
    #[test]
    fn reference_config_lists_every_option() {
        let reference = include_str!("../tacoshell.toml");
        let mut cfg = minimal("[local]\nprogram = \"sh\"\n").unwrap();
        cfg.app.icon = Some("x".into());
        cfg.app.version = Some("1.0".into());
        cfg.ssh = Some(Ssh {
            server: Some("x".into()),
            command: Some("x".into()),
            shell_toggle: Some(ShellToggle { label: "x".into(), default: true }),
            ..Default::default()
        });
        cfg.login.logo_file = Some("x".into());
        cfg.login.logo_color = Some("#000000".into());
        // unset options dont show up in the toml, so set every one of them
        cfg.packaging = Packaging {
            maintainer: Some("x".into()),
            license: Some("x".into()),
            homepage: Some("x".into()),
            long_description: Some("x".into()),
        };

        let value = toml::Value::try_from(&cfg).unwrap();
        let mut missing = Vec::new();
        for (section, table) in value.as_table().unwrap() {
            for key in table.as_table().unwrap().keys() {
                // a whole key at the start of a (maybe commented out) line, "exit" isnt "on_exit"
                let listed = reference.lines().any(|l| {
                    let l = l.trim_start().trim_start_matches('#').trim_start();
                    l.strip_prefix(key.as_str()).is_some_and(|rest| rest.trim_start().starts_with('='))
                });
                if !listed {
                    missing.push(format!("{section}.{key}"));
                }
            }
        }
        assert!(missing.is_empty(), "not in tacoshell.toml: {missing:?}");
    }

    #[test]
    fn default_keyboard_is_valid() {
        let k = Kiosk::default();
        for row in k.keyboard.iter().chain(&k.keyboard_shift) {
            parse_osk_row(row).unwrap();
        }
    }
}
