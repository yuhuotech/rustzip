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
        Self::entry(INDEX)
            .and_then(|e| e.get_password().ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
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

impl SecretStore for KeychainStore {
    fn get(&self, label: &str) -> Option<String> {
        Self::entry(label)?.get_password().ok()
    }

    fn set(&self, label: &str, secret: &str) -> Result<()> {
        Self::entry(label)
            .ok_or_else(|| keychain_error("unavailable"))?
            .set_password(secret)
            .map_err(keychain_error)?;
        let mut labels = Self::read_index();
        if !labels.iter().any(|l| l == label) {
            labels.push(label.to_string());
            Self::write_index(&labels)?;
        }
        Ok(())
    }

    fn delete(&self, label: &str) -> Result<()> {
        if let Some(e) = Self::entry(label) {
            // Deleting what is not there is fine.
            let _ = e.delete_credential();
        }
        let mut labels = Self::read_index();
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
