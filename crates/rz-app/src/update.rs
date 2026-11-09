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
use std::io::{Read, Seek, SeekFrom, Write};

/// Largest manifest we will read.
const MAX_MANIFEST: u64 = 1 << 20;
/// Slack allowed over the size a manifest announces for a download.
const SIZE_SLACK: u64 = 4096;
const MAX_TASK_BYTES: u64 = 64 * 1024;
/// Absolute bound for the unsigned size in a release manifest.
pub const MAX_UPDATE_SIZE: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// `os-arch`, or `macos-arch-zip` / `windows-arch-zip` for an update ZIP.
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

/// Data handed to an independent updater. Paths remain untrusted until the
/// platform updater has checked them against its own installation records.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateTask {
    pub transaction_id: String,
    pub version: String,
    pub asset: Asset,
    pub archive: std::path::PathBuf,
    pub installation: std::path::PathBuf,
    pub parent_pid: u32,
    pub parent_start_time: u64,
}

impl UpdateTask {
    /// Reads a task without allowing an untrusted file to allocate unbounded memory.
    pub fn read(reader: impl Read) -> Result<Self> {
        let mut bytes = Vec::new();
        reader
            .take(MAX_TASK_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(Error::Io)?;
        if bytes.len() as u64 > MAX_TASK_BYTES {
            return Err(Error::LimitExceeded("update task is too large".into()));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::Corrupt(format!("update task is invalid: {e}")))
    }

    /// Checks signed identity and task bounds before platform-specific work.
    pub fn validate(
        &self,
        current_version: &str,
        expected_platform: &str,
        public_key: &[u8; 32],
    ) -> Result<()> {
        let fail = |m: &str| Error::Corrupt(format!("update task rejected: {m}"));
        if self.transaction_id.len() != 32
            || !self
                .transaction_id
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(fail("invalid transaction identifier"));
        }
        if self.parent_pid == 0 || self.parent_start_time == 0 {
            return Err(fail("invalid parent process identity"));
        }
        if !self.archive.is_absolute() || !self.installation.is_absolute() {
            return Err(fail("paths must be absolute"));
        }
        if self.asset.platform != expected_platform {
            return Err(fail("asset platform does not match this updater"));
        }
        let current =
            Version::parse(current_version).map_err(|_| fail("invalid current version"))?;
        let offered = Version::parse(&self.version).map_err(|_| fail("invalid target version"))?;
        if offered <= current || (!offered.pre.is_empty() && current.pre.is_empty()) {
            return Err(fail("target version is not eligible"));
        }
        verify_asset_metadata(&self.version, &self.asset, public_key)
    }
}

/// Retrieves bytes from a URL, refusing to read more than `max_bytes`.
pub trait Fetcher {
    fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>>;

    /// Retrieves bytes while reporting the number received so far.
    fn get_with_progress(
        &self,
        url: &str,
        max_bytes: u64,
        progress: &mut dyn FnMut(u64),
    ) -> Result<Vec<u8>> {
        let bytes = self.get(url, max_bytes)?;
        progress(bytes.len() as u64);
        Ok(bytes)
    }

    /// Writes a bounded response to a destination while reporting progress.
    fn get_to_writer(
        &self,
        url: &str,
        max_bytes: u64,
        output: &mut dyn Write,
        progress: &mut dyn FnMut(u64),
    ) -> Result<()> {
        let bytes = self.get_with_progress(url, max_bytes, progress)?;
        output.write_all(&bytes).map_err(Error::Io)
    }
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
/// users are never moved onto a beta by a release-channel mistake. macOS and
/// Windows prefer ZIP assets; original platform keys remain readable for compatibility.
pub fn select_update(manifest: &Manifest, current: &str, platform: &str) -> Result<Option<Update>> {
    let bad = |what: &str, v: &str| Error::Corrupt(format!("update manifest: bad {what} {v:?}"));
    let current = Version::parse(current).map_err(|_| bad("current version", current))?;
    let offered =
        Version::parse(&manifest.version).map_err(|_| bad("version", &manifest.version))?;
    if offered <= current || (!offered.pre.is_empty() && current.pre.is_empty()) {
        return Ok(None);
    }
    // Older clients look up the original platform key and therefore keep receiving DMGs.
    let zip_platform = (platform.starts_with("macos-") || platform.starts_with("windows-"))
        .then(|| format!("{platform}-zip"));
    let asset = zip_platform
        .as_ref()
        .and_then(|key| manifest.assets.iter().find(|asset| &asset.platform == key))
        .or_else(|| {
            manifest
                .assets
                .iter()
                .find(|asset| asset.platform == platform)
        });
    let Some(asset) = asset else {
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
    verify_asset(
        bytes,
        &update.version.to_string(),
        &update.asset,
        public_key,
    )
}

/// Checks size, hash and signature for one manifest asset.
pub fn verify_asset(
    bytes: &[u8],
    version: &str,
    asset: &Asset,
    public_key: &[u8; 32],
) -> Result<()> {
    verify_asset_reader(bytes, version, asset, public_key)
}

/// Verifies an asset from a reader without holding the whole file in memory.
///
/// The asset size is not signed, so both the declared size and actual input are
/// bounded independently of the signature.
pub fn verify_asset_reader(
    mut reader: impl Read,
    version: &str,
    asset: &Asset,
    public_key: &[u8; 32],
) -> Result<()> {
    let fail = |m: &str| Error::Corrupt(format!("update rejected: {m}"));
    verify_asset_metadata(version, asset, public_key)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).map_err(Error::Io)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| fail("size overflow"))?;
        if total > asset.size || total > MAX_UPDATE_SIZE {
            return Err(fail("size exceeds the manifest"));
        }
        digest.update(&buffer[..count]);
    }
    if total != asset.size {
        return Err(fail("size does not match the manifest"));
    }
    if !hex(&digest.finalize()).eq_ignore_ascii_case(&asset.sha256) {
        return Err(fail("hash does not match the manifest"));
    }
    Ok(())
}

/// Validates the signed identity and limits before accessing an asset.
pub fn verify_asset_metadata(version: &str, asset: &Asset, public_key: &[u8; 32]) -> Result<()> {
    let fail = |m: &str| Error::Corrupt(format!("update rejected: {m}"));
    let version = Version::parse(version).map_err(|_| fail("version is invalid"))?;
    if asset.size == 0 || asset.size > MAX_UPDATE_SIZE {
        return Err(Error::LimitExceeded("update asset size is invalid".into()));
    }
    let digest = unhex::<32>(&asset.sha256).ok_or_else(|| fail("hash is not 32 bytes of hex"))?;
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| fail("invalid release key"))?;
    let sig_bytes: [u8; 64] =
        unhex(&asset.signature).ok_or_else(|| fail("signature is not 64 bytes of hex"))?;
    let sig = Signature::from_bytes(&sig_bytes);
    let message = signed_message(&version.to_string(), &asset.platform, &hex(&digest));
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
        self.download_with_progress(update, |_, _| {})
    }

    /// Downloads and verifies an asset, reporting received bytes against its declared size.
    pub fn download_with_progress(
        &self,
        update: &Update,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<Vec<u8>> {
        verify_asset_metadata(&update.version.to_string(), &update.asset, &self.public_key)?;
        let bytes = self.fetcher.get_with_progress(
            &update.asset.url,
            update.asset.size.saturating_add(SIZE_SLACK),
            &mut |received| progress(received, update.asset.size),
        )?;
        verify_download(&bytes, update, &self.public_key)?;
        progress(update.asset.size, update.asset.size);
        Ok(bytes)
    }

    /// Streams an update into a seekable file and verifies it before returning.
    pub fn download_to_file(
        &self,
        update: &Update,
        file: &mut std::fs::File,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<()> {
        verify_asset_metadata(&update.version.to_string(), &update.asset, &self.public_key)?;
        file.set_len(0).map_err(Error::Io)?;
        file.seek(SeekFrom::Start(0)).map_err(Error::Io)?;
        self.fetcher.get_to_writer(
            &update.asset.url,
            update.asset.size.saturating_add(SIZE_SLACK),
            file,
            &mut |received| progress(received, update.asset.size),
        )?;
        file.flush().map_err(Error::Io)?;
        file.seek(SeekFrom::Start(0)).map_err(Error::Io)?;
        verify_asset_reader(
            file,
            &update.version.to_string(),
            &update.asset,
            &self.public_key,
        )?;
        progress(update.asset.size, update.asset.size);
        Ok(())
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
    fn verified_download_reports_received_and_declared_total_bytes() {
        let body = b"signed installer bytes".to_vec();
        let net = server(&manifest("0.2.0", &body), &body);
        let updater = Updater::new(&net, "https://example.com/update.json", pubkey());
        let update = updater
            .check("0.1.0", "macos-aarch64")
            .ok()
            .flatten()
            .expect("an update");
        let mut progress = Vec::new();

        let downloaded = updater
            .download_with_progress(&update, |received, total| {
                progress.push((received, total));
            })
            .expect("the signed update downloads");

        assert_eq!(downloaded, body);
        assert_eq!(
            progress.last(),
            Some(&(body.len() as u64, body.len() as u64))
        );
        assert_eq!(progress.len(), 2);
    }

    #[test]
    fn same_older_or_missing_platform_means_no_update() {
        let m = manifest("0.2.0", b"x");
        assert_eq!(select_update(&m, "0.2.0", "macos-aarch64").ok(), Some(None));
        assert_eq!(select_update(&m, "0.3.0", "macos-aarch64").ok(), Some(None));
        assert_eq!(select_update(&m, "0.1.0", "linux-riscv64").ok(), Some(None));
    }

    #[test]
    fn macos_prefers_zip_and_legacy_clients_can_still_choose_dmg() {
        for platform in ["macos-aarch64", "macos-x86_64"] {
            let dmg = asset("0.2.0", platform, b"legacy dmg");
            let zip = asset("0.2.0", &format!("{platform}-zip"), b"new zip");
            let manifest = Manifest {
                version: "0.2.0".into(),
                notes: String::new(),
                assets: vec![dmg.clone(), zip.clone()],
            };
            let selected = select_update(&manifest, "0.1.5", platform)
                .expect("valid manifest")
                .expect("update available");
            assert_eq!(selected.asset, zip);
            assert!(verify_download(b"new zip", &selected, &pubkey()).is_ok());
            assert!(verify_download(b"legacy dmg", &selected, &pubkey()).is_err());
            assert_eq!(
                manifest.assets.iter().find(|a| a.platform == platform),
                Some(&dmg)
            );

            let legacy = Manifest {
                assets: vec![dmg.clone()],
                ..manifest
            };
            assert_eq!(
                select_update(&legacy, "0.1.5", platform)
                    .expect("valid manifest")
                    .expect("fallback update")
                    .asset,
                dmg
            );
        }
    }

    #[test]
    fn windows_prefers_separately_signed_zip_and_preserves_the_installer_key() {
        let installer = asset("0.2.0", "windows-x86_64", b"old client installer");
        let zip = asset("0.2.0", "windows-x86_64-zip", b"new updater ZIP");
        let manifest = Manifest {
            version: "0.2.0".into(),
            notes: String::new(),
            assets: vec![installer.clone(), zip.clone()],
        };
        let selected = select_update(&manifest, "0.1.6", "windows-x86_64")
            .unwrap()
            .unwrap();
        assert_eq!(selected.asset, zip);
        assert!(verify_download(b"new updater ZIP", &selected, &pubkey()).is_ok());
        assert_eq!(
            manifest
                .assets
                .iter()
                .find(|a| a.platform == "windows-x86_64"),
            Some(&installer)
        );
        let mut relabeled = selected.clone();
        relabeled.asset.platform = "windows-x86_64".into();
        assert!(verify_download(b"new updater ZIP", &relabeled, &pubkey()).is_err());
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
    fn untrusted_metadata_is_checked_before_fetching_an_asset() {
        struct NoFetch;
        impl Fetcher for NoFetch {
            fn get(&self, _: &str, _: u64) -> Result<Vec<u8>> {
                panic!("invalid metadata must not cause a download")
            }
        }

        let mut update = select_update(&manifest("0.2.0", b"x"), "0.1.0", "macos-aarch64")
            .expect("valid manifest")
            .expect("update available");
        let updater = Updater::new(&NoFetch, "https://example.com/update.json", pubkey());
        update.asset.size = MAX_UPDATE_SIZE + 1;
        assert!(updater.download(&update).is_err());
        update.asset.size = 1;
        update.asset.signature = hex(&[0; 64]);
        assert!(updater.download(&update).is_err());
    }

    #[test]
    fn independent_reader_rechecks_the_complete_signed_asset() {
        let body = b"verified update";
        let signed = asset("0.2.0", "linux-x86_64", body);
        assert!(verify_asset_reader(&body[..], "0.2.0", &signed, &pubkey()).is_ok());
        assert!(verify_asset_reader(&body[..body.len() - 1], "0.2.0", &signed, &pubkey()).is_err());
        assert!(
            verify_asset_reader(&b"verified update!"[..], "0.2.0", &signed, &pubkey()).is_err()
        );
        assert!(verify_asset_reader(&body[..], "0.2.1", &signed, &pubkey()).is_err());
    }

    #[test]
    fn task_rejects_wrong_identity_platform_and_replayed_version() {
        let mut task = UpdateTask {
            transaction_id: "0123456789abcdef0123456789abcdef".into(),
            version: "0.2.0".into(),
            asset: asset("0.2.0", "linux-x86_64", b"update"),
            archive: std::env::temp_dir().join("update.tar.gz"),
            installation: std::env::temp_dir().join("rustzip"),
            parent_pid: 1,
            parent_start_time: 1,
        };
        assert!(task.validate("0.1.0", "linux-x86_64", &pubkey()).is_ok());
        assert!(task.validate("0.2.0", "linux-x86_64", &pubkey()).is_err());
        assert!(task.validate("0.1.0", "windows-x86_64", &pubkey()).is_err());
        task.transaction_id = "../escape".into();
        assert!(task.validate("0.1.0", "linux-x86_64", &pubkey()).is_err());
        task.transaction_id = "0123456789abcdef0123456789abcdef".into();
        task.parent_start_time = 0;
        assert!(task.validate("0.1.0", "linux-x86_64", &pubkey()).is_err());
    }

    #[test]
    fn task_input_has_a_fixed_size_limit_and_rejects_unknown_fields() {
        assert!(UpdateTask::read(&vec![b' '; MAX_TASK_BYTES as usize + 1][..]).is_err());
        let task = UpdateTask {
            transaction_id: "0123456789abcdef0123456789abcdef".into(),
            version: "0.2.0".into(),
            asset: asset("0.2.0", "linux-x86_64", b"update"),
            archive: std::env::temp_dir().join("update"),
            installation: std::env::temp_dir().join("rustzip"),
            parent_pid: 1,
            parent_start_time: 1,
        };
        let mut value = serde_json::to_value(task).expect("serialize task");
        value["extra"] = true.into();
        let json = serde_json::to_vec(&value).expect("serialize JSON");
        assert!(UpdateTask::read(&json[..]).is_err());
    }

    #[test]
    fn file_download_is_verified_before_success() {
        let body = b"streamed installer";
        let net = server(&manifest("0.2.0", body), body);
        let updater = Updater::new(&net, "https://example.com/update.json", pubkey());
        let mut update = updater
            .check("0.1.0", "macos-aarch64")
            .expect("manifest")
            .expect("update");
        let mut file = tempfile::tempfile().expect("temporary file");
        updater
            .download_to_file(&update, &mut file, |_, _| {})
            .expect("verified download");
        file.seek(SeekFrom::Start(0)).expect("seek");
        let mut downloaded = Vec::new();
        file.read_to_end(&mut downloaded).expect("read");
        assert_eq!(downloaded, body);

        update.asset.size += 1;
        assert!(updater
            .download_to_file(&update, &mut file, |_, _| {})
            .is_err());
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
