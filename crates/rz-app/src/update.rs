//! Update checking and verification. Nothing here touches the network or the
//! installed program: a [`Fetcher`] supplies bytes, and the result is a verified
//! download that the platform installer can apply.
//!
//! Trust comes from an Ed25519 signature made offline with the release key, not
//! from the download server or TLS alone. The signed message binds the version,
//! platform and file hash, so an old release cannot be replayed as a new one and
//! a Windows build cannot be swapped in for a macOS one.

use ed25519_dalek::{Signature, VerifyingKey};
use rz_core::{Error, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Largest manifest we will read.
const MAX_MANIFEST: u64 = 1 << 20;
/// Slack allowed over the size a manifest announces for a download.
const SIZE_SLACK: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// `os-arch`, for example `macos-aarch64` or `windows-x86_64`.
    pub platform: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: String,
    /// Lowercase hex Ed25519 signature over [`signed_message`].
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub assets: Vec<Asset>,
}

/// An update that is newer than the running version and has a build for this platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: Version,
    pub notes: String,
    pub asset: Asset,
}

/// Retrieves bytes from a URL, refusing to read more than `max_bytes`.
pub trait Fetcher {
    fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>>;
}

/// The platform string used in manifests.
pub fn current_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The exact bytes the release key signs for one asset.
pub fn signed_message(version: &str, platform: &str, sha256_hex: &str) -> Vec<u8> {
    format!("rustzip-update-v1\n{version}\n{platform}\n{sha256_hex}").into_bytes()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn parse_manifest(json: &[u8]) -> Result<Manifest> {
    serde_json::from_slice(json).map_err(|e| Error::Corrupt(format!("update manifest: {e}")))
}

/// Picks the update for this platform, if it is newer than `current`.
///
/// Pre-releases are only offered to users already on a pre-release, so stable
/// users are never moved onto a beta by a release-channel mistake.
pub fn select_update(manifest: &Manifest, current: &str, platform: &str) -> Result<Option<Update>> {
    let bad = |what: &str, v: &str| Error::Corrupt(format!("update manifest: bad {what} {v:?}"));
    let current = Version::parse(current).map_err(|_| bad("current version", current))?;
    let offered =
        Version::parse(&manifest.version).map_err(|_| bad("version", &manifest.version))?;
    if offered <= current || (!offered.pre.is_empty() && current.pre.is_empty()) {
        return Ok(None);
    }
    let Some(asset) = manifest.assets.iter().find(|a| a.platform == platform) else {
        return Ok(None);
    };
    if !asset.url.starts_with("https://") {
        return Err(Error::UnsafePath(format!(
            "update URL is not https: {}",
            asset.url
        )));
    }
    Ok(Some(Update {
        version: offered,
        notes: manifest.notes.clone(),
        asset: asset.clone(),
    }))
}

/// Checks size, hash and signature of a downloaded file.
pub fn verify_download(bytes: &[u8], update: &Update, public_key: &[u8; 32]) -> Result<()> {
    let asset = &update.asset;
    let fail = |m: &str| Error::Corrupt(format!("update rejected: {m}"));
    if bytes.len() as u64 != asset.size {
        return Err(fail("size does not match the manifest"));
    }
    let digest = hex(&Sha256::digest(bytes));
    if !digest.eq_ignore_ascii_case(&asset.sha256) {
        return Err(fail("hash does not match the manifest"));
    }
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| fail("invalid release key"))?;
    let sig_bytes: [u8; 64] =
        unhex(&asset.signature).ok_or_else(|| fail("signature is not 64 bytes of hex"))?;
    let sig = Signature::from_bytes(&sig_bytes);
    let message = signed_message(&update.version.to_string(), &asset.platform, &digest);
    key.verify_strict(&message, &sig)
        .map_err(|_| fail("signature is not valid for this release"))
}

pub struct Updater<'a, F: Fetcher> {
    fetcher: &'a F,
    manifest_url: String,
    public_key: [u8; 32],
}

impl<'a, F: Fetcher> Updater<'a, F> {
    pub fn new(fetcher: &'a F, manifest_url: &str, public_key: [u8; 32]) -> Self {
        Self {
            fetcher,
            manifest_url: manifest_url.to_string(),
            public_key,
        }
    }

    pub fn check(&self, current: &str, platform: &str) -> Result<Option<Update>> {
        let json = self.fetcher.get(&self.manifest_url, MAX_MANIFEST)?;
        select_update(&parse_manifest(&json)?, current, platform)
    }

    /// Downloads the asset and returns its bytes only if they verify.
    pub fn download(&self, update: &Update) -> Result<Vec<u8>> {
        let bytes = self.fetcher.get(
            &update.asset.url,
            update.asset.size.saturating_add(SIZE_SLACK),
        )?;
        verify_download(&bytes, update, &self.public_key)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::HashMap;

    const SEED: [u8; 32] = [7; 32];

    fn key() -> SigningKey {
        SigningKey::from_bytes(&SEED)
    }

    fn pubkey() -> [u8; 32] {
        key().verifying_key().to_bytes()
    }

    fn asset(version: &str, platform: &str, body: &[u8]) -> Asset {
        let sha = hex(&Sha256::digest(body));
        let sig = key().sign(&signed_message(version, platform, &sha));
        Asset {
            platform: platform.into(),
            url: format!("https://dl.example.com/{version}/{platform}"),
            size: body.len() as u64,
            sha256: sha,
            signature: hex(&sig.to_bytes()),
        }
    }

    fn manifest(version: &str, body: &[u8]) -> Manifest {
        Manifest {
            version: version.into(),
            notes: "Faster 7z".into(),
            assets: vec![
                asset(version, "macos-aarch64", body),
                asset(version, "windows-x86_64", body),
            ],
        }
    }

    struct Mock(HashMap<String, Vec<u8>>);
    impl Fetcher for Mock {
        fn get(&self, url: &str, max: u64) -> Result<Vec<u8>> {
            let data = self
                .0
                .get(url)
                .ok_or_else(|| Error::Io(std::io::Error::other("404")))?;
            if data.len() as u64 > max {
                return Err(Error::LimitExceeded("download too large".into()));
            }
            Ok(data.clone())
        }
    }

    fn server(m: &Manifest, body: &[u8]) -> Mock {
        let mut files = HashMap::new();
        files.insert(
            "https://example.com/update.json".to_string(),
            serde_json::to_vec(m).unwrap_or_default(),
        );
        for a in &m.assets {
            files.insert(a.url.clone(), body.to_vec());
        }
        Mock(files)
    }

    #[test]
    fn a_newer_signed_release_is_found_downloaded_and_verified() {
        let body = b"installer bytes".to_vec();
        let m = manifest("0.2.0", &body);
        let net = server(&m, &body);
        let up = Updater::new(&net, "https://example.com/update.json", pubkey());
        let found = up.check("0.1.0", "macos-aarch64").unwrap_or(None);
        let found = found.expect("an update");
        assert_eq!(found.version, Version::new(0, 2, 0));
        assert_eq!(found.notes, "Faster 7z");
        assert_eq!(up.download(&found).ok(), Some(body));
    }

    #[test]
    fn same_older_or_missing_platform_means_no_update() {
        let m = manifest("0.2.0", b"x");
        assert_eq!(select_update(&m, "0.2.0", "macos-aarch64").ok(), Some(None));
        assert_eq!(select_update(&m, "0.3.0", "macos-aarch64").ok(), Some(None));
        assert_eq!(select_update(&m, "0.1.0", "linux-riscv64").ok(), Some(None));
    }

    #[test]
    fn prereleases_are_not_offered_to_stable_users() {
        let m = manifest("0.3.0-beta.1", b"x");
        assert_eq!(select_update(&m, "0.2.0", "macos-aarch64").ok(), Some(None));
        // ...but a user already on a pre-release is.
        let got = select_update(&m, "0.3.0-alpha.2", "macos-aarch64")
            .ok()
            .flatten();
        assert!(got.is_some());
        // And a final release beats its own pre-release.
        let fin = manifest("0.3.0", b"x");
        assert!(select_update(&fin, "0.3.0-rc.1", "macos-aarch64")
            .ok()
            .flatten()
            .is_some());
    }

    #[test]
    fn versions_compare_numerically_not_as_text() {
        let m = manifest("0.10.0", b"x");
        assert!(select_update(&m, "0.9.9", "macos-aarch64")
            .ok()
            .flatten()
            .is_some());
    }

    #[test]
    fn tampered_bytes_are_rejected() {
        let body = b"installer bytes".to_vec();
        let m = manifest("0.2.0", &body);
        let u = select_update(&m, "0.1.0", "macos-aarch64").ok().flatten();
        let u = u.expect("an update");
        let mut evil = body.clone();
        evil[0] ^= 1;
        assert!(verify_download(&evil, &u, &pubkey()).is_err());
        assert!(verify_download(&body[..5], &u, &pubkey()).is_err());
        assert!(verify_download(&body, &u, &pubkey()).is_ok());
    }

    #[test]
    fn a_manifest_that_lies_about_the_hash_is_rejected() {
        let body = b"real".to_vec();
        let evil = b"evil".to_vec();
        let mut m = manifest("0.2.0", &body);
        // The attacker serves their own file and updates size and hash to match.
        m.assets[0].size = evil.len() as u64;
        m.assets[0].sha256 = hex(&Sha256::digest(&evil));
        let u = select_update(&m, "0.1.0", "macos-aarch64").ok().flatten();
        let u = u.expect("an update");
        assert!(
            verify_download(&evil, &u, &pubkey()).is_err(),
            "signature must not cover it"
        );
    }

    #[test]
    fn a_signature_from_another_key_is_rejected() {
        let body = b"installer".to_vec();
        let m = manifest("0.2.0", &body);
        let u = select_update(&m, "0.1.0", "macos-aarch64").ok().flatten();
        let u = u.expect("an update");
        let other = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
        assert!(verify_download(&body, &u, &other).is_err());
    }

    #[test]
    fn an_old_signed_release_cannot_pose_as_a_new_version() {
        let body = b"old installer".to_vec();
        let mut m = manifest("0.1.5", &body);
        m.version = "0.9.0".into(); // the version is part of the signed message
        let u = select_update(&m, "0.1.0", "macos-aarch64").ok().flatten();
        let u = u.expect("an update");
        assert!(verify_download(&body, &u, &pubkey()).is_err());
    }

    #[test]
    fn a_build_for_one_platform_cannot_be_used_for_another() {
        let body = b"mac installer".to_vec();
        let mut m = manifest("0.2.0", &body);
        // Relabel the macOS asset as Windows without a new signature.
        m.assets[0].platform = "windows-x86_64".into();
        m.assets.remove(1);
        let u = select_update(&m, "0.1.0", "windows-x86_64").ok().flatten();
        let u = u.expect("an update");
        assert!(verify_download(&body, &u, &pubkey()).is_err());
    }

    #[test]
    fn malformed_signatures_and_urls_are_rejected() {
        let body = b"x".to_vec();
        let mut m = manifest("0.2.0", &body);
        m.assets[0].signature = "zz".into();
        let u = select_update(&m, "0.1.0", "macos-aarch64").ok().flatten();
        assert!(verify_download(&body, &u.expect("an update"), &pubkey()).is_err());

        let mut m = manifest("0.2.0", &body);
        m.assets[0].url = "http://dl.example.com/plain".into();
        assert!(select_update(&m, "0.1.0", "macos-aarch64").is_err());
        m.assets[0].url = "file:///etc/passwd".into();
        assert!(select_update(&m, "0.1.0", "macos-aarch64").is_err());
    }

    #[test]
    fn broken_manifests_are_errors() {
        assert!(parse_manifest(b"not json").is_err());
        assert!(parse_manifest(br#"{"version":"1.0.0"}"#).is_err());
        let m = Manifest {
            version: "banana".into(),
            notes: String::new(),
            assets: vec![],
        };
        assert!(select_update(&m, "0.1.0", "macos-aarch64").is_err());
        assert!(select_update(&manifest("0.2.0", b"x"), "oops", "macos-aarch64").is_err());
    }

    #[test]
    fn oversized_downloads_are_refused_by_the_fetch_limit() {
        let body = b"small".to_vec();
        let m = manifest("0.2.0", &body);
        let mut net = server(&m, &body);
        // The server sends far more than the manifest announced.
        for a in &m.assets {
            net.0.insert(a.url.clone(), vec![0u8; 100_000]);
        }
        let up = Updater::new(&net, "https://example.com/update.json", pubkey());
        let u = up.check("0.1.0", "macos-aarch64").ok().flatten();
        assert!(up.download(&u.expect("an update")).is_err());
    }

    #[test]
    fn hex_helpers_round_trip() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(unhex::<3>("000fff"), Some([0, 15, 255]));
        assert_eq!(unhex::<3>("000ff"), None);
        assert_eq!(unhex::<1>("é"), None);
    }

    #[test]
    fn platform_string_has_os_and_arch() {
        let p = current_platform();
        assert!(p.contains('-') && !p.starts_with('-'));
    }
}
