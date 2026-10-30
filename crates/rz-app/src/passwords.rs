//! Passwords: a book of remembered passwords and a way to find which one opens an archive.
//!
//! Where passwords are stored is the platform's business (Keychain, Credential
//! Manager, Secret Service), so storage sits behind [`SecretStore`]. A store
//! backed by the system keychain belongs to the GUI layer.

use rz_core::{CancelToken, EntryKind, Error, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

pub trait SecretStore: Send + Sync {
    /// Fallible access for management screens: a locked store is not an empty store.
    fn try_get(&self, label: &str) -> Result<Option<String>> {
        Ok(self.get(label))
    }
    fn try_labels(&self) -> Result<Vec<String>> {
        Ok(self.labels())
    }
    fn is_persistent(&self) -> bool {
        false
    }
    fn get(&self, label: &str) -> Option<String>;
    fn set(&self, label: &str, secret: &str) -> Result<()>;
    fn delete(&self, label: &str) -> Result<()>;
    fn labels(&self) -> Vec<String>;
}

impl<T: SecretStore + ?Sized> SecretStore for Arc<T> {
    fn try_get(&self, label: &str) -> Result<Option<String>> {
        (**self).try_get(label)
    }

    fn try_labels(&self) -> Result<Vec<String>> {
        (**self).try_labels()
    }

    fn is_persistent(&self) -> bool {
        (**self).is_persistent()
    }

    fn get(&self, label: &str) -> Option<String> {
        (**self).get(label)
    }

    fn set(&self, label: &str, secret: &str) -> Result<()> {
        (**self).set(label, secret)
    }

    fn delete(&self, label: &str) -> Result<()> {
        (**self).delete(label)
    }

    fn labels(&self) -> Vec<String> {
        (**self).labels()
    }
}

/// A store that lives only in memory: for tests and for users who choose not to save passwords.
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<String, Zeroizing<String>>>);

impl SecretStore for MemoryStore {
    fn try_get(&self, label: &str) -> Result<Option<String>> {
        self.0
            .lock()
            .map(|m| m.get(label).map(|v| v.to_string()))
            .map_err(|_| Error::Corrupt("password store poisoned".into()))
    }

    fn try_labels(&self) -> Result<Vec<String>> {
        self.0
            .lock()
            .map(|m| m.keys().cloned().collect())
            .map_err(|_| Error::Corrupt("password store poisoned".into()))
    }
    fn get(&self, label: &str) -> Option<String> {
        self.0.lock().ok()?.get(label).map(|v| v.to_string())
    }

    fn set(&self, label: &str, secret: &str) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| Error::Corrupt("password store poisoned".into()))?
            .insert(label.to_string(), Zeroizing::new(secret.to_string()));
        Ok(())
    }

    fn delete(&self, label: &str) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| Error::Corrupt("password store poisoned".into()))?
            .remove(label);
        Ok(())
    }

    fn labels(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// The largest entry read to check a password.
const PROBE_LIMIT: u64 = 64 << 20;
/// Bound automatic password attempts so opening an archive cannot turn into an
/// unbounded sequence of full-entry integrity checks.
pub const MAX_AUTO_PASSWORD_CANDIDATES: usize = 16;

/// Finds which candidate opens `archive`, trying each in order.
///
/// Returns `Ok(None)` when none fits. `Err` is reserved for an archive that cannot
/// be read at all. The check reads a small encrypted file completely, so a wrong
/// password is caught by the archive's own integrity check and not just by luck.
pub fn find_password(archive: &Path, candidates: &[String]) -> Result<Option<String>> {
    let candidates = candidates
        .iter()
        .cloned()
        .map(Zeroizing::new)
        .collect::<Vec<_>>();
    find_password_with_cancel(archive, &candidates, &CancelToken::new())
        .map(|password| password.map(|password| password.to_string()))
}

/// Finds a matching candidate while keeping each attempt bounded and cancellable.
///
/// Password values are zeroized when this call returns. The candidate count is
/// capped even when the caller supplies a larger list.
pub fn find_password_with_cancel(
    archive: &Path,
    candidates: &[Zeroizing<String>],
    cancel: &CancelToken,
) -> Result<Option<Zeroizing<String>>> {
    if candidates.is_empty() {
        return Ok(None);
    }
    let Some(probe) = password_probe(archive, cancel)? else {
        return Ok(None);
    };
    for pw in candidates.iter().take(MAX_AUTO_PASSWORD_CANDIDATES) {
        cancel.check()?;
        let mut reader = rz_formats::open(archive)?;
        reader.set_read_control(cancel.clone(), PROBE_LIMIT);
        reader.set_password(pw.as_str())?;
        let entries = match reader.entries() {
            Ok(e) => e,
            // An encrypted header (7z) rejects a wrong password right here.
            Err(Error::Password) => continue,
            Err(e) => return Err(e),
        };
        let entry_index = match probe {
            PasswordProbe::Header => {
                return Ok(Some(Zeroizing::new(pw.to_string())));
            }
            PasswordProbe::Entry(index) => index,
        };
        let Some(entry) = entries.iter().find(|entry| entry.index == entry_index) else {
            return Err(Error::Corrupt("password probe entry disappeared".into()));
        };
        let ok = match reader.open(entry) {
            Ok(mut data) => {
                let mut sink = [0u8; 64 * 1024];
                loop {
                    cancel.check()?;
                    match data.read(&mut sink) {
                        Ok(0) => break true,
                        Ok(_) => {}
                        Err(_) if cancel.is_cancelled() => return Err(Error::Cancelled),
                        Err(_) => break false,
                    }
                }
            }
            Err(Error::Password | Error::Corrupt(_) | Error::Io(_)) => false,
            Err(e) => return Err(e),
        };
        if ok {
            return Ok(Some(Zeroizing::new(pw.to_string())));
        }
    }
    Ok(None)
}

#[derive(Clone, Copy)]
enum PasswordProbe {
    Header,
    Entry(usize),
}

fn password_probe(archive: &Path, cancel: &CancelToken) -> Result<Option<PasswordProbe>> {
    cancel.check()?;
    let mut reader = rz_formats::open(archive)?;
    reader.set_read_control(cancel.clone(), PROBE_LIMIT);
    let format = reader.format_name();
    let entries = match reader.entries() {
        Ok(entries) => entries,
        Err(Error::Password) => return Ok(Some(PasswordProbe::Header)),
        Err(error) => return Err(error),
    };

    if let Some(entry) = entries
        .iter()
        .filter(|entry| {
            entry.kind == EntryKind::File && entry.encrypted && entry.size <= PROBE_LIMIT
        })
        .min_by_key(|entry| entry.size)
    {
        return Ok(Some(PasswordProbe::Entry(entry.index)));
    }

    if format != "7z" {
        return Ok(None);
    }
    let Some(entry) = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File && entry.size <= PROBE_LIMIT)
        .min_by_key(|entry| entry.size)
    else {
        return Ok(None);
    };
    let index = entry.index;
    let mut data = match reader.open(entry) {
        Ok(data) => data,
        Err(Error::Password | Error::Corrupt(_) | Error::Io(_)) => {
            return Ok(Some(PasswordProbe::Entry(index)));
        }
        Err(error) => return Err(error),
    };
    let mut sink = [0u8; 64 * 1024];
    loop {
        cancel.check()?;
        match data.read(&mut sink) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            // 7z readers do not expose per-file encryption flags, so a failed
            // unpassworded read is the only safe signal to probe this entry.
            Err(_) if cancel.is_cancelled() => return Err(Error::Cancelled),
            Err(_) => return Ok(Some(PasswordProbe::Entry(index))),
        }
    }
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

    /// Collects a bounded list of saved passwords, preferring one archive label.
    ///
    /// Keychain indexes append labels as passwords are added, so reverse iteration
    /// tries newer general-purpose entries first after the archive-specific label.
    pub fn try_candidates(
        &self,
        preferred_label: Option<&str>,
        maximum: usize,
    ) -> Result<Vec<Zeroizing<String>>> {
        self.try_candidates_with_cancel(preferred_label, maximum, &CancelToken::new())
    }

    /// Collects saved candidates while checking cancellation between store operations.
    pub fn try_candidates_with_cancel(
        &self,
        preferred_label: Option<&str>,
        maximum: usize,
        cancel: &CancelToken,
    ) -> Result<Vec<Zeroizing<String>>> {
        cancel.check()?;
        let maximum = maximum.min(MAX_AUTO_PASSWORD_CANDIDATES);
        if maximum == 0 {
            return Ok(Vec::new());
        }

        let mut passwords = Vec::with_capacity(maximum);
        if let Some(label) = preferred_label {
            cancel.check()?;
            if let Some(password) = self.store.try_get(label)? {
                passwords.push(Zeroizing::new(password));
            }
        }

        cancel.check()?;
        let mut labels = self.store.try_labels()?;
        labels.reverse();
        for label in labels {
            cancel.check()?;
            if passwords.len() >= maximum {
                break;
            }
            if Some(label.as_str()) == preferred_label {
                continue;
            }
            if let Some(password) = self.store.try_get(&label)? {
                let password = Zeroizing::new(password);
                if !passwords
                    .iter()
                    .any(|candidate: &Zeroizing<String>| candidate.as_str() == password.as_str())
                {
                    passwords.push(password);
                }
            }
        }
        cancel.check()?;
        Ok(passwords)
    }

    /// Looks for a saved password that opens the archive.
    pub fn find_for(&self, archive: &Path) -> Result<Option<String>> {
        self.find_for_with_cancel(archive, &CancelToken::new())
            .map(|password| password.map(|password| password.to_string()))
    }

    /// Looks for a saved password and allows the caller to cancel the search.
    pub fn find_for_with_cancel(
        &self,
        archive: &Path,
        cancel: &CancelToken,
    ) -> Result<Option<Zeroizing<String>>> {
        let candidates =
            self.try_candidates_with_cancel(None, MAX_AUTO_PASSWORD_CANDIDATES, cancel)?;
        find_password_with_cancel(archive, &candidates, cancel)
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
    fn cancelled_password_search_stops_before_probing_the_archive() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let archive = make(dir.path(), "s.zip", "hunter2");
        let candidates = pws(&["hunter2"])
            .into_iter()
            .map(Zeroizing::new)
            .collect::<Vec<_>>();
        let cancel = CancelToken::new();
        cancel.cancel();

        assert!(matches!(
            find_password_with_cancel(&archive, &candidates, &cancel),
            Err(Error::Cancelled)
        ));
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
