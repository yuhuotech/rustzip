//! Passwords: a book of remembered passwords and a way to find which one opens an archive.
//!
//! Where passwords are stored is the platform's business (Keychain, Credential
//! Manager, Secret Service), so storage sits behind [`SecretStore`]. A store
//! backed by the system keychain belongs to the GUI layer.

use rz_core::{EntryKind, Error, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

pub trait SecretStore: Send + Sync {
    fn get(&self, label: &str) -> Option<String>;
    fn set(&self, label: &str, secret: &str) -> Result<()>;
    fn delete(&self, label: &str) -> Result<()>;
    fn labels(&self) -> Vec<String>;
}

/// A store that lives only in memory: for tests and for users who choose not to save passwords.
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, label: &str) -> Option<String> {
        self.0.lock().ok()?.get(label).cloned()
    }

    fn set(&self, label: &str, secret: &str) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| Error::Corrupt("password store poisoned".into()))?
            .insert(label.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, label: &str) -> Result<()> {
        if let Ok(mut m) = self.0.lock() {
            m.remove(label);
        }
        Ok(())
    }

    fn labels(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// The largest entry read to check a password; bigger ones are checked on a prefix only
/// when the format verifies passwords before decoding (zip AES).
const PROBE_LIMIT: u64 = 64 << 20;

/// Finds which candidate opens `archive`, trying each in order.
///
/// Returns `Ok(None)` when none fits. `Err` is reserved for an archive that cannot
/// be read at all. The check reads the smallest encrypted file completely, so a
/// wrong password is caught by the archive's own integrity check and not just by luck.
pub fn find_password(archive: &Path, candidates: &[String]) -> Result<Option<String>> {
    for pw in candidates {
        let mut reader = rz_formats::open(archive)?;
        reader.set_password(pw)?;
        let entries = match reader.entries() {
            Ok(e) => e,
            // An encrypted header (7z) rejects a wrong password right here.
            Err(Error::Password) => continue,
            Err(e) => return Err(e),
        };
        let probe = entries
            .iter()
            .filter(|e| e.kind == EntryKind::File && e.encrypted)
            .chain(entries.iter().filter(|e| e.kind == EntryKind::File))
            .filter(|e| e.size <= PROBE_LIMIT)
            .min_by_key(|e| (!e.encrypted, e.size));
        let Some(probe) = probe else {
            // Nothing to decrypt: any password "works", so do not claim one did.
            return Ok(None);
        };
        let ok = match reader.open(probe) {
            Ok(mut data) => {
                let mut sink = [0u8; 64 * 1024];
                loop {
                    match data.read(&mut sink) {
                        Ok(0) => break true,
                        Ok(_) => {}
                        Err(_) => break false,
                    }
                }
            }
            Err(Error::Password | Error::Corrupt(_) | Error::Io(_)) => false,
            Err(e) => return Err(e),
        };
        if ok {
            return Ok(Some(pw.clone()));
        }
    }
    Ok(None)
}

/// Remembered passwords, tried most recently added first.
pub struct PasswordBook<S: SecretStore> {
    store: S,
}

impl<S: SecretStore> PasswordBook<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    pub fn remember(&self, label: &str, secret: &str) -> Result<()> {
        self.store.set(label, secret)
    }

    pub fn forget(&self, label: &str) -> Result<()> {
        self.store.delete(label)
    }

    pub fn labels(&self) -> Vec<String> {
        self.store.labels()
    }

    /// Every saved password, for [`find_password`].
    pub fn candidates(&self) -> Vec<String> {
        self.store
            .labels()
            .iter()
            .filter_map(|l| self.store.get(l))
            .collect()
    }

    /// Looks for a saved password that opens the archive.
    pub fn find_for(&self, archive: &Path) -> Result<Option<String>> {
        find_password(archive, &self.candidates())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_core::{CancelToken, CreateOptions, NoProgress};
    use rz_engine::compress;
    use rz_formats::OutputFormat;
    use std::path::PathBuf;

    fn make(dir: &Path, name: &str, password: &str) -> PathBuf {
        let src = dir.join("src");
        let _ = std::fs::create_dir_all(&src);
        let _ = std::fs::write(src.join("a.txt"), "secret text ".repeat(50));
        let _ = std::fs::write(src.join("b.txt"), "x");
        let out = dir.join(name);
        let opts = CreateOptions {
            password: Some(password.into()),
            threads: 2,
            ..CreateOptions::default()
        };
        let format = OutputFormat::from_path(&out).unwrap_or(OutputFormat::Zip);
        assert!(compress(
            &[src],
            &out,
            format,
            &opts,
            &CancelToken::new(),
            &mut NoProgress
        )
        .is_ok());
        out
    }

    fn pws(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn finds_the_right_password_for_zip_and_7z() {
        for ext in ["zip", "7z"] {
            let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
            let archive = make(dir.path(), &format!("s.{ext}"), "hunter2");
            let got = find_password(&archive, &pws(&["123456", "password", "hunter2", "qwerty"]));
            assert_eq!(got.ok().flatten().as_deref(), Some("hunter2"), "{ext}");
        }
    }

    #[test]
    fn reports_none_when_no_candidate_fits() {
        for ext in ["zip", "7z"] {
            let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
            let archive = make(dir.path(), &format!("s.{ext}"), "hunter2");
            let got = find_password(&archive, &pws(&["123456", "password"]));
            assert_eq!(got.ok(), Some(None), "{ext}");
            assert_eq!(find_password(&archive, &[]).ok(), Some(None), "{ext}");
        }
    }

    #[test]
    fn unreadable_archives_are_errors_not_misses() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let junk = dir.path().join("junk.zip");
        let _ = std::fs::write(&junk, "nope");
        assert!(find_password(&junk, &pws(&["a"])).is_err());
    }

    #[test]
    fn the_book_keeps_passwords_in_its_store() {
        let book = PasswordBook::new(MemoryStore::default());
        assert!(book.remember("work", "w0rk").is_ok());
        assert!(book.remember("home", "h0me").is_ok());
        assert_eq!(book.labels(), ["home", "work"]);
        assert_eq!(book.candidates().len(), 2);
        assert!(book.forget("home").is_ok());
        assert_eq!(book.candidates(), ["w0rk"]);
    }

    #[test]
    fn the_book_finds_a_saved_password_for_an_archive() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let archive = make(dir.path(), "s.zip", "from-the-book");
        let book = PasswordBook::new(MemoryStore::default());
        let _ = book.remember("a", "wrong");
        let _ = book.remember("b", "from-the-book");
        assert_eq!(
            book.find_for(&archive).ok().flatten().as_deref(),
            Some("from-the-book")
        );
    }
}
