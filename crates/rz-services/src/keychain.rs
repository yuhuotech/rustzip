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

    fn entry(label: &str) -> Option<Entry> {
        Entry::new(SERVICE, label).ok()
    }

    fn read_index() -> Vec<String> {
        Self::try_read_index().unwrap_or_default()
    }

    fn try_read_index() -> Result<Vec<String>> {
        let Some(text) = read_secret(INDEX)? else {
            return Ok(Vec::new());
        };
        serde_json::from_str(&text).map_err(|_| keychain_error("invalid password index"))
    }

    fn write_index(labels: &[String]) -> Result<()> {
        let json = serde_json::to_string(labels).map_err(|e| Error::Corrupt(e.to_string()))?;
        Self::entry(INDEX)
            .ok_or_else(|| keychain_error("unavailable"))?
            .set_password(&json)
            .map_err(keychain_error)
    }
}

fn keychain_error(e: impl std::fmt::Display) -> Error {
    Error::Io(std::io::Error::other(format!("keychain: {e}")))
}

fn read_secret(label: &str) -> Result<Option<String>> {
    match KeychainStore::entry(label)
        .ok_or_else(|| keychain_error("unavailable"))?
        .get_password()
    {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(keychain_error(e)),
    }
}

static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl SecretStore for KeychainStore {
    fn try_get(&self, label: &str) -> Result<Option<String>> {
        read_secret(label)
    }
    fn try_labels(&self) -> Result<Vec<String>> {
        Self::try_read_index()
    }
    fn is_persistent(&self) -> bool {
        true
    }
    fn get(&self, label: &str) -> Option<String> {
        Self::entry(label)?.get_password().ok()
    }

    fn set(&self, label: &str, secret: &str) -> Result<()> {
        if label == INDEX {
            return Err(keychain_error("reserved label"));
        }
        let _guard = WRITE_LOCK.lock().map_err(|_| keychain_error("busy"))?;
        let mut labels = Self::try_read_index()?;
        let previous = read_secret(label)?.map(zeroize::Zeroizing::new);
        Self::entry(label)
            .ok_or_else(|| keychain_error("unavailable"))?
            .set_password(secret)
            .map_err(keychain_error)?;
        if !labels.iter().any(|l| l == label) {
            labels.push(label.to_string());
            if let Err(e) = Self::write_index(&labels) {
                if let Some(entry) = Self::entry(label) {
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
        let mut labels = Self::try_read_index()?;
        if let Some(e) = Self::entry(label) {
            // Deleting what is not there is fine.
            match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(keychain_error(e)),
            }
        }
        labels.retain(|l| l != label);
        Self::write_index(&labels)
    }

    fn labels(&self) -> Vec<String> {
        Self::read_index()
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
        let store = KeychainStore;
        let label = "rustzip-test-entry.zip";
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
    }
}
