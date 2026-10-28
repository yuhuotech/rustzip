//! Passwords in the system keychain: Keychain Services on macOS, Credential Manager on
//! Windows, the Secret Service on Linux.
//!
//! A keychain cannot list its entries, so the labels are kept in one extra entry that
//! holds a JSON list. The labels are archive names, never paths.

use keyring::Entry;
use rz_app::passwords::SecretStore;
use rz_core::{Error, Result};

const SERVICE: &str = "RustZIP";
const INDEX: &str = "__labels__";

pub struct KeychainStore;

impl KeychainStore {
    /// Whether the system keychain can be used here. Headless Linux has no Secret Service.
    pub fn available() -> bool {
        Entry::store_status().is_ok()
    }
}

struct Backend<'a> {
    service: &'a str,
}

impl Backend<'_> {
    fn entry(&self, label: &str) -> Option<Entry> {
        Entry::new(self.service, label).ok()
    }

    fn read_index(&self) -> Vec<String> {
        self.try_read_index().unwrap_or_default()
    }

    fn try_read_index(&self) -> Result<Vec<String>> {
        let Some(text) = self.read_secret(INDEX)? else {
            return Ok(Vec::new());
        };
        serde_json::from_str(&text).map_err(|_| keychain_error("invalid password index"))
    }

    fn write_index(&self, labels: &[String]) -> Result<()> {
        let json = serde_json::to_string(labels).map_err(|e| Error::Corrupt(e.to_string()))?;
        self.entry(INDEX)
            .ok_or_else(|| keychain_error("unavailable"))?
            .set_password(&json)
            .map_err(keychain_error)
    }

    fn read_secret(&self, label: &str) -> Result<Option<String>> {
        match self
            .entry(label)
            .ok_or_else(|| keychain_error("unavailable"))?
            .get_password()
        {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_error(e)),
        }
    }
}

fn keychain_error(e: impl std::fmt::Display) -> Error {
    Error::Io(std::io::Error::other(format!("keychain: {e}")))
}

static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl SecretStore for Backend<'_> {
    fn try_get(&self, label: &str) -> Result<Option<String>> {
        self.read_secret(label)
    }
    fn try_labels(&self) -> Result<Vec<String>> {
        self.try_read_index()
    }
    fn is_persistent(&self) -> bool {
        true
    }
    fn get(&self, label: &str) -> Option<String> {
        self.entry(label)?.get_password().ok()
    }

    fn set(&self, label: &str, secret: &str) -> Result<()> {
        if label == INDEX {
            return Err(keychain_error("reserved label"));
        }
        let _guard = WRITE_LOCK.lock().map_err(|_| keychain_error("busy"))?;
        let mut labels = self.try_read_index()?;
        let previous = self.read_secret(label)?.map(zeroize::Zeroizing::new);
        self.entry(label)
            .ok_or_else(|| keychain_error("unavailable"))?
            .set_password(secret)
            .map_err(keychain_error)?;
        if !labels.iter().any(|l| l == label) {
            labels.push(label.to_string());
            if let Err(e) = self.write_index(&labels) {
                if let Some(entry) = self.entry(label) {
                    if let Some(value) = previous {
                        let _ = entry.set_password(&value);
                    } else {
                        let _ = entry.delete_credential();
                    }
                }
                return Err(e);
            }
        }
        Ok(())
    }

    fn delete(&self, label: &str) -> Result<()> {
        if label == INDEX {
            return Err(keychain_error("reserved label"));
        }
        let _guard = WRITE_LOCK.lock().map_err(|_| keychain_error("busy"))?;
        let mut labels = self.try_read_index()?;
        if let Some(e) = self.entry(label) {
            // Deleting what is not there is fine.
            match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(keychain_error(e)),
            }
        }
        labels.retain(|l| l != label);
        self.write_index(&labels)
    }

    fn labels(&self) -> Vec<String> {
        self.read_index()
    }
}

impl SecretStore for KeychainStore {
    fn try_get(&self, label: &str) -> Result<Option<String>> {
        Backend { service: SERVICE }.try_get(label)
    }
    fn try_labels(&self) -> Result<Vec<String>> {
        Backend { service: SERVICE }.try_labels()
    }
    fn is_persistent(&self) -> bool {
        true
    }
    fn get(&self, label: &str) -> Option<String> {
        Backend { service: SERVICE }.get(label)
    }
    fn set(&self, label: &str, secret: &str) -> Result<()> {
        Backend { service: SERVICE }.set(label, secret)
    }
    fn delete(&self, label: &str) -> Result<()> {
        Backend { service: SERVICE }.delete(label)
    }
    fn labels(&self) -> Vec<String> {
        Backend { service: SERVICE }.labels()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Touches the real keychain, which a CI runner or a locked screen may not offer, so it
    /// runs only on request: `cargo test -p rz-services -- --ignored`.
    #[test]
    #[ignore = "uses the system keychain"]
    fn round_trip_through_the_real_keychain() {
        assert!(KeychainStore::available());
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let service = format!("RustZIP-test-{}-{nonce}", std::process::id());
        let label = "test.zip";
        let store = Backend { service: &service };
        let cleanup = DeleteTestPassword(&service, label);
        store
            .set(label, "p\u{e4}ssw\u{f6}rd \u{5bc6}\u{7801}")
            .unwrap();
        assert_eq!(
            store.get(label).as_deref(),
            Some("p\u{e4}ssw\u{f6}rd \u{5bc6}\u{7801}")
        );
        assert!(store.labels().iter().any(|l| l == label));
        store.delete(label).unwrap();
        assert_eq!(store.get(label), None);
        assert!(!store.labels().iter().any(|l| l == label));
        drop(cleanup);
        assert!(matches!(
            Entry::new(&service, INDEX).unwrap().get_password(),
            Err(keyring::Error::NoEntry)
        ));
    }

    struct DeleteTestPassword<'a>(&'a str, &'a str);

    impl Drop for DeleteTestPassword<'_> {
        fn drop(&mut self) {
            let store = Backend { service: self.0 };
            let _ = store.delete(self.1);
            if let Some(index) = store.entry(INDEX) {
                let _ = index.delete_credential();
            }
        }
    }
}
