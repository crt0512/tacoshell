// where the password lives between reconnects
//
//   memory  only while the app runs: reconnects dont ask again, relaunching does
//   unsafe  base64x2 in a file next to known_hosts, survives relaunches. Very unsafe indeed dont use unless you are trying to compile for Windows CE lol

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use data_encoding::BASE64;
use zeroize::Zeroizing;

use crate::schema;
use crate::state::write_private;

pub type Secret = Zeroizing<String>;

pub trait CredentialStore {
    fn get(&self, account: &str) -> Option<Secret>;
    fn set(&mut self, account: &str, secret: Secret);
    fn forget(&mut self, account: &str);
}

/// file is where the unsafe store keeps its passwords, None falls back to memory
pub fn open(kind: schema::Credentials, file: Option<PathBuf>) -> Box<dyn CredentialStore> {
    match (kind, file) {
        (schema::Credentials::Memory, _) | (schema::Credentials::Unsafe, None) => Box::new(Memory::default()),
        (schema::Credentials::Unsafe, Some(file)) => Box::new(UnsafeFile::open(file)),
    }
}

/// the key a password is stored under
pub fn account(username: &str, host: &str, port: u16) -> String {
    format!("{username}@{host}:{port}")
}

#[derive(Default)]
struct Memory(HashMap<String, Secret>);

impl CredentialStore for Memory {
    fn get(&self, account: &str) -> Option<Secret> {
        self.0.get(account).cloned()
    }

    fn set(&mut self, account: &str, secret: Secret) {
        self.0.insert(account.to_string(), secret);
    }

    fn forget(&mut self, account: &str) {
        self.0.remove(account);
    }
}

/// one line per account: "user@host:port" = "base64(base64(password))"
struct UnsafeFile {
    file: PathBuf,
    entries: BTreeMap<String, String>,
}

impl UnsafeFile {
    fn open(file: PathBuf) -> Self {
        let entries = fs::read_to_string(&file)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        UnsafeFile { file, entries }
    }

    fn save(&self) {
        let result = if self.entries.is_empty() {
            fs::remove_file(&self.file).or_else(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(e),
            })
        } else {
            toml::to_string(&self.entries)
                .map_err(std::io::Error::other)
                .and_then(|text| write_private(&self.file, &text))
        };
        // not being able to remember is annoying, not fatal: next launch asks again
        if let Err(e) = result {
            eprintln!("cant write {}: {e}", self.file.display());
        }
    }
}

impl CredentialStore for UnsafeFile {
    fn get(&self, account: &str) -> Option<Secret> {
        let once = Zeroizing::new(BASE64.decode(self.entries.get(account)?.as_bytes()).ok()?);
        let twice = Zeroizing::new(BASE64.decode(&once).ok()?);
        String::from_utf8(twice.to_vec()).ok().map(Zeroizing::new)
    }

    fn set(&mut self, account: &str, secret: Secret) {
        let once = Zeroizing::new(BASE64.encode(secret.as_bytes()));
        self.entries.insert(account.to_string(), BASE64.encode(once.as_bytes()));
        self.save();
    }

    fn forget(&mut self, account: &str) {
        if self.entries.remove(account).is_some() {
            self.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("tacoshell-creds-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("credentials");
        let acct = account("crt", "example", 22);

        let mut store = open(schema::Credentials::Unsafe, Some(file.clone()));
        assert!(store.get(&acct).is_none());
        store.set(&acct, Zeroizing::new("salsa verde".into()));

        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.contains("salsa"), "plain password in the file: {text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // a fresh store (next launch) still knows it
        let mut store = open(schema::Credentials::Unsafe, Some(file.clone()));
        assert_eq!(store.get(&acct).as_deref().map(String::as_str), Some("salsa verde"));

        store.forget(&acct);
        assert!(store.get(&acct).is_none());
        assert!(!file.exists(), "empty store should leave no file behind");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_forgets_on_relaunch() {
        let mut store = open(schema::Credentials::Memory, None);
        store.set("a", Zeroizing::new("b".into()));
        assert!(store.get("a").is_some());
        assert!(open(schema::Credentials::Memory, None).get("a").is_none());
    }
}
