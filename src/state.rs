// what the setup screen remembers between launches, until logout. and the window size, logout or not.
// no passwords in here, those go through creds.rs

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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

/// where Store goes when theres no config dir, android hands the app a private one at startup
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

#[cfg(target_os = "android")]
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

#[derive(Clone)]
pub struct Store {
    /// None when theres neither a config dir nor a DATA_DIR, then nothing sticks
    dir: Option<PathBuf>,
}

impl Store {
    pub fn new(app_id: &str) -> Self {
        Store { dir: dirs::config_dir().or_else(|| DATA_DIR.get().cloned()).map(|d| d.join(app_id)) }
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

    /// the window size from last time, survives logout
    fn window_file(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join("window.toml"))
    }

    pub fn load_window(&self) -> Option<WindowSize> {
        let text = fs::read_to_string(self.window_file()?).ok()?;
        let size: WindowSize = toml::from_str(&text).ok()?;
        // hand edited nonsense would make a window of no size at all
        let sane = |v: f32| v.is_finite() && v >= 1.0;
        (sane(size.width) && sane(size.height)).then_some(size)
    }

    pub fn save_window(&self, size: WindowSize) -> io::Result<()> {
        let Some(file) = self.window_file() else { return Ok(()) };
        let text = toml::to_string(&size).map_err(io::Error::other)?;
        write_private(&file, &text)
    }
}

/// inner size of the window in points
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: f32,
    pub height: f32,
}

/// how long the window has to stay one size before that goes to disk, dragging a corner is a lot of sizes
const WINDOW_SETTLE: Duration = Duration::from_millis(500);

/// writes the window size down for the next launch, once it stops changing
pub struct WindowMemory {
    store: Store,
    /// on disk already, or the size the window started at. None until the first look
    kept: Option<WindowSize>,
    /// a size that isnt kept yet, and since when the window has it
    latest: Option<(WindowSize, Instant)>,
}

impl WindowMemory {
    pub fn new(store: Store) -> Self {
        WindowMemory { store, kept: None, latest: None }
    }

    /// the window's size this frame, None while its fullscreen, maximized or minimized (thats not the size to come back to).
    /// returns how soon it wants another look, when theres a size waiting to settle
    pub fn seen(&mut self, size: Option<WindowSize>, now: Instant) -> Option<Duration> {
        let size = size?;
        let Some(kept) = self.kept else {
            // where the window starts isnt news, the next launch would start there anyway
            self.kept = Some(size);
            return None;
        };
        if size == kept {
            self.latest = None;
            return None;
        }
        let since = match self.latest {
            Some((latest, since)) if latest == size => since,
            _ => {
                self.latest = Some((size, now));
                now
            }
        };
        let left = WINDOW_SETTLE.saturating_sub(now - since);
        if left.is_zero() {
            self.flush();
            return None;
        }
        Some(left)
    }

    /// onto disk with whatever hasnt settled yet, for when the app quits
    pub fn flush(&mut self) {
        let Some((size, _)) = self.latest.take() else { return };
        if let Err(e) = self.store.save_window(size) {
            eprintln!("cant save the window size: {e}");
        }
        self.kept = Some(size);
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

    fn temp_store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("tacoshell-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Store { dir: Some(dir) }
    }

    #[test]
    fn window_size_on_disk() {
        let store = temp_store("window-disk");
        assert_eq!(store.load_window(), None);
        let size = WindowSize { width: 812.5, height: 600.0 };
        store.save_window(size).unwrap();
        assert_eq!(store.load_window(), Some(size));
        // logout forgets the setup, not the window
        store.clear();
        assert_eq!(store.load_window(), Some(size));
        fs::write(store.window_file().unwrap(), "width = 0.0\nheight = 600.0\n").unwrap();
        assert_eq!(store.load_window(), None);
        fs::write(store.window_file().unwrap(), "garbage").unwrap();
        assert_eq!(store.load_window(), None);
        let _ = fs::remove_dir_all(store.dir.unwrap());
    }

    #[test]
    fn window_size_settles() {
        let store = temp_store("window-settle");
        let mut memory = WindowMemory::new(store.clone());
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let (start, dragged, done) = (
            WindowSize { width: 800.0, height: 600.0 },
            WindowSize { width: 900.0, height: 650.0 },
            WindowSize { width: 1000.0, height: 700.0 },
        );

        // the size it started at doesnt get written
        assert_eq!(memory.seen(Some(start), at(0)), None);
        assert_eq!(memory.seen(Some(start), at(2000)), None);
        assert_eq!(store.load_window(), None);

        // mid drag nothing is written, it waits for the window to sit still
        assert_eq!(memory.seen(Some(dragged), at(2000)), Some(WINDOW_SETTLE));
        assert!(memory.seen(Some(done), at(2100)).is_some());
        assert!(memory.seen(Some(done), at(2400)).is_some());
        assert_eq!(store.load_window(), None);
        assert_eq!(memory.seen(Some(done), at(2600)), None);
        assert_eq!(store.load_window(), Some(done));

        // fullscreen and co dont count, and quitting keeps what hadnt settled yet
        assert_eq!(memory.seen(None, at(3000)), None);
        assert!(memory.seen(Some(start), at(3000)).is_some());
        assert_eq!(memory.seen(None, at(4000)), None);
        memory.flush();
        assert_eq!(store.load_window(), Some(start));

        // resized and put back before it settled: nothing to write
        assert!(memory.seen(Some(done), at(5000)).is_some());
        assert_eq!(memory.seen(Some(start), at(5100)), None);
        memory.flush();
        assert_eq!(store.load_window(), Some(start));
        let _ = fs::remove_dir_all(store.dir.unwrap());
    }
}
