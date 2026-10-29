//! Release tooling, run by the maintainer and by the release workflow.
//!
//!     rz-release keygen <private-key-file>
//!     rz-release sign <version> <platform> <file> <url>        (key in $RZ_UPDATE_KEY)
//!     rz-release manifest <version> <notes-file> <asset.json>...
//!     rz-release verify <update.json> <asset-directory> [test-public-key-hex]
//!
//! The private key is 32 random bytes as 64 hex characters. It is only ever read from the
//! environment variable `RZ_UPDATE_KEY` (a repository secret in CI), never from a file in
//! the repository.

use ed25519_dalek::{Signer, SigningKey};
use rz_app::update::{hex, parse_manifest, signed_message, verify_asset, Asset, Manifest};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

const APP_UPDATE_PUBLIC_KEY: [u8; 32] = include!("../../../crates/rz-services/src/update_key.in");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["keygen", out] => keygen(out),
        ["sign", version, platform, file, url] => sign(version, platform, file, url),
        ["manifest", version, notes, assets @ ..] if !assets.is_empty() => {
            manifest(version, notes, assets)
        }
        ["verify", manifest_file, asset_dir] => {
            verify_manifest_file(manifest_file, asset_dir, None)
        }
        ["verify", manifest_file, asset_dir, public_key] => {
            verify_manifest_file(manifest_file, asset_dir, Some(public_key))
        }
        _ => Err(
            "usage: rz-release keygen <file> | sign <version> <platform> <file> <url> | \
                  manifest <version> <notes-file> <asset.json>... | \
                  verify <update.json> <asset-dir> [test-public-key-hex]"
                .to_string(),
        ),
    };
    match result {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("rz-release: {e}");
            ExitCode::FAILURE
        }
    }
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Writes a new private key to `path` (owner-only on Unix) and prints the public half
/// as the Rust array that goes into `crates/rz-services/src/update_key.in`.
fn keygen(path: &str) -> Result<String, String> {
    if std::path::Path::new(path).exists() {
        return Err(format!("{path} exists; refusing to overwrite a key"));
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("no randomness: {e}"))?;
    write_private(path, hex(&seed).as_bytes())?;
    let public = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    let bytes: Vec<String> = public.iter().map(|b| format!("0x{b:02x}")).collect();
    Ok(format!("[{}]", bytes.join(", ")))
}

#[cfg(unix)]
fn write_private(path: &str, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.write_all(data).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn write_private(path: &str, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| e.to_string())
}

fn signing_key() -> Result<SigningKey, String> {
    let text = std::env::var("RZ_UPDATE_KEY").map_err(|_| "RZ_UPDATE_KEY is not set")?;
    let seed = unhex32(&text).ok_or("RZ_UPDATE_KEY must be 64 hex characters")?;
    Ok(SigningKey::from_bytes(&seed))
}

fn sign(version: &str, platform: &str, file: &str, url: &str) -> Result<String, String> {
    sign_with(&signing_key()?, version, platform, file, url)
}

fn sign_with(
    key: &SigningKey,
    version: &str,
    platform: &str,
    file: &str,
    url: &str,
) -> Result<String, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    let sha256 = hex(&Sha256::digest(&bytes));
    let signature = key.sign(&signed_message(version, platform, &sha256));
    let asset = Asset {
        platform: platform.to_string(),
        url: url.to_string(),
        size: bytes.len() as u64,
        sha256,
        signature: hex(&signature.to_bytes()),
    };
    serde_json::to_string(&asset).map_err(|e| e.to_string())
}

fn manifest(version: &str, notes_file: &str, asset_files: &[&str]) -> Result<String, String> {
    let notes = std::fs::read_to_string(notes_file).unwrap_or_default();
    let mut assets = Vec::new();
    for f in asset_files {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
        assets.push(serde_json::from_str::<Asset>(&text).map_err(|e| format!("{f}: {e}"))?);
    }
    let manifest = Manifest {
        version: version.trim_start_matches('v').to_string(),
        notes,
        assets,
    };
    serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())
}

fn verify_manifest_file(
    manifest_file: &str,
    asset_dir: &str,
    public_key_hex: Option<&str>,
) -> Result<String, String> {
    let bytes = std::fs::read(manifest_file).map_err(|e| format!("{manifest_file}: {e}"))?;
    let manifest = parse_manifest(&bytes).map_err(|e| e.to_string())?;
    let test_key = public_key_hex
        .map(|text| {
            unhex32(text).ok_or_else(|| "test public key must be 64 hex characters".to_string())
        })
        .transpose()?;
    let public_key = test_key.as_ref().unwrap_or(&APP_UPDATE_PUBLIC_KEY);
    let platforms = verify_manifest_assets(&manifest, Path::new(asset_dir), public_key)?;
    Ok(format!("verified update assets: {}", platforms.join(", ")))
}

fn verify_manifest_assets(
    manifest: &Manifest,
    asset_dir: &Path,
    public_key: &[u8; 32],
) -> Result<Vec<String>, String> {
    if manifest.assets.is_empty() {
        return Err("update manifest has no assets".into());
    }
    let mut platforms = BTreeSet::new();
    for asset in &manifest.assets {
        if !platforms.insert(asset.platform.as_str()) {
            return Err(format!("duplicate update platform: {}", asset.platform));
        }
        if !asset.url.starts_with("https://") || asset.url.contains('?') || asset.url.contains('#')
        {
            return Err(format!("unsafe asset URL for {}", asset.platform));
        }
        let file_name = asset
            .url
            .rsplit('/')
            .next()
            .ok_or_else(|| format!("missing asset filename for {}", asset.platform))?;
        if file_name.is_empty()
            || file_name == "."
            || file_name == ".."
            || !file_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            return Err(format!("unsafe asset filename for {}", asset.platform));
        }
        let path = asset_dir.join(file_name);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("missing {} asset: {e}", asset.platform))?;
        if !metadata.file_type().is_file() {
            return Err(format!("{} asset is not a regular file", asset.platform));
        }
        if metadata.len() != asset.size {
            return Err(format!("{} asset size does not match", asset.platform));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{} asset: {e}", asset.platform))?;
        verify_asset(&bytes, &manifest.version, asset, public_key)
            .map_err(|e| format!("{} asset: {e}", asset.platform))?;
    }
    Ok(platforms.into_iter().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use rz_app::update::{select_update, verify_download, Fetcher, Updater};

    struct Static(Vec<u8>);
    impl Fetcher for Static {
        fn get(&self, _: &str, _: u64) -> rz_core::Result<Vec<u8>> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn what_the_tool_signs_is_what_the_app_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("RustZIP.dmg");
        std::fs::write(&file, b"installer bytes").unwrap();
        let key = SigningKey::from_bytes(&[9; 32]);

        let asset_json = sign_with(
            &key,
            "0.2.0",
            "macos-aarch64",
            file.to_str().unwrap(),
            "https://example.com/RustZIP.dmg",
        )
        .unwrap();
        let asset_file = dir.path().join("a.json");
        std::fs::write(&asset_file, &asset_json).unwrap();
        let notes = dir.path().join("notes.txt");
        std::fs::write(&notes, "Faster").unwrap();
        let manifest_json = manifest(
            "v0.2.0",
            notes.to_str().unwrap(),
            &[asset_file.to_str().unwrap()],
        )
        .unwrap();

        let public = key.verifying_key().to_bytes();
        let source = Static(manifest_json.into_bytes());
        let updater = Updater::new(&source, "https://x/update.json", public);
        let update = updater.check("0.1.0", "macos-aarch64").unwrap().unwrap();
        assert_eq!(update.version.to_string(), "0.2.0");
        verify_download(b"installer bytes", &update, &public).unwrap();
        // Other bytes, or another key, are refused.
        assert!(verify_download(b"tampered bytes!", &update, &public).is_err());
        let other = SigningKey::from_bytes(&[1; 32]).verifying_key().to_bytes();
        assert!(verify_download(b"installer bytes", &update, &other).is_err());
        assert!(select_update(
            &serde_json::from_str(
                &manifest(
                    "0.2.0",
                    notes.to_str().unwrap(),
                    &[asset_file.to_str().unwrap()]
                )
                .unwrap()
            )
            .unwrap(),
            "0.2.0",
            "macos-aarch64"
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn keygen_never_overwrites_and_the_key_has_the_right_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k");
        let public = keygen(path.to_str().unwrap()).unwrap();
        assert!(public.starts_with("[0x") && public.matches("0x").count() == 32);
        let seed = unhex32(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let derived = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
        assert!(public.contains(&format!("0x{:02x}", derived[0])));
        assert!(keygen(path.to_str().unwrap()).is_err());
    }

    #[test]
    fn manifest_verification_checks_local_assets_against_the_app_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let file = dir.path().join("RustZIP-0.2.0-macos-aarch64.dmg");
        std::fs::write(&file, b"installer bytes").unwrap();
        let asset_json = sign_with(
            &key,
            "0.2.0",
            "macos-aarch64",
            file.to_str().unwrap(),
            "https://example.com/RustZIP-0.2.0-macos-aarch64.dmg",
        )
        .unwrap();
        let asset: Asset = serde_json::from_str(&asset_json).unwrap();
        let manifest = Manifest {
            version: "0.2.0".into(),
            notes: String::new(),
            assets: vec![asset.clone()],
        };
        let public_key = key.verifying_key().to_bytes();
        let manifest_path = dir.path().join("update.json");
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let public_key_hex = hex(&public_key);
        assert!(verify_manifest_file(
            manifest_path.to_str().unwrap(),
            dir.path().to_str().unwrap(),
            Some(&public_key_hex)
        )
        .is_ok());
        assert_eq!(
            verify_manifest_assets(&manifest, dir.path(), &public_key).unwrap(),
            ["macos-aarch64"]
        );

        std::fs::write(&file, b"tampered bytes!").unwrap();
        assert!(verify_manifest_assets(&manifest, dir.path(), &public_key).is_err());

        let duplicate = Manifest {
            assets: vec![asset.clone(), asset],
            ..manifest
        };
        assert!(verify_manifest_assets(&duplicate, dir.path(), &public_key).is_err());
    }
}
