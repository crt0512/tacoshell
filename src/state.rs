// what the setup screen remembers between launches, until logout.
// no passwords in here, those go through creds.rs

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub server: String,
    pub username: String,
    pub login_shell: bool,
    /// the remember checkboxes on the login screen, so they stick too
    pub remember_server: bool,
    pub remember_username: bool,
    /// "salt$sha256", guards logout on a kiosk
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kiosk_pin: Option<String>,
}

impl Default for Saved {
    fn default() -> Self {
        Saved {
            server: String::new(),
            username: String::new(),
            login_shell: false,
            remember_server: true,
            remember_username: true,
            kiosk_pin: None,
        }
    }
}

impl Saved {
    /// what goes to disk, minus what the user didnt want remembered
    pub fn for_disk(&self) -> Saved {
        let mut out = self.clone();
        if !self.remember_server {
            out.server.clear();
        }
        if !self.remember_username {
            out.username.clear();
        }
        out
    }
}

pub struct Store {
    /// None where theres no config dir (some phones), then nothing sticks
    dir: Option<PathBuf>,
}

impl Store {
    pub fn new(app_id: &str) -> Self {
        Store { dir: dirs::config_dir().map(|d| d.join(app_id)) }
    }

    fn setup_file(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join("setup.toml"))
    }

    /// trusted server keys, openssh known_hosts format. survives logout
    pub fn known_hosts(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join("known_hosts"))
    }

    /// passwords, only with credentials = "unsafe"
    pub fn credentials(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join("credentials"))
    }

    pub fn load(&self) -> Option<Saved> {
        let text = fs::read_to_string(self.setup_file()?).ok()?;
        toml::from_str(&text).ok()
    }

    pub fn save(&self, saved: &Saved) -> io::Result<()> {
        let Some(file) = self.setup_file() else { return Ok(()) };
        let text = toml::to_string(&saved.for_disk()).map_err(io::Error::other)?;
        write_private(&file, &text)
    }

    pub fn clear(&self) {
        if let Some(file) = self.setup_file() {
            let _ = fs::remove_file(file);
        }
    }
}

/// only we get to read it (on unix anyway), makes the folder if it has to
pub fn write_private(file: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(file)?.write_all(text.as_bytes())
}

/// same rule as the taskologic screensaver pin, 4 to 16 digits
pub fn pin_ok(pin: &str) -> bool {
    (4..=16).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

pub fn hash_pin(pin: &str) -> String {
    let mut salt = [0u8; 16];
    // no randomness is bad but not locking yourself out of the kiosk bad
    let _ = getrandom::fill(&mut salt);
    let salt = hex(&salt);
    format!("{salt}${}", digest(&salt, pin))
}

pub fn check_pin(stored: &str, pin: &str) -> bool {
    match stored.split_once('$') {
        Some((salt, hash)) => digest(salt, pin) == hash,
        None => false,
    }
}

fn digest(salt: &str, pin: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(pin.as_bytes());
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins() {
        assert!(pin_ok("1234") && pin_ok("1234567890123456"));
        assert!(!pin_ok("123") && !pin_ok("12345678901234567") && !pin_ok("12a4"));
        let stored = hash_pin("1234");
        assert!(check_pin(&stored, "1234"));
        assert!(!check_pin(&stored, "4321"));
        assert!(!stored.contains("1234$"), "salt missing");
        assert_ne!(hash_pin("1234"), stored, "same salt twice");
    }
}
