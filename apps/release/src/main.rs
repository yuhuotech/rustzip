//! Release tooling, run by the maintainer and by the release workflow.
//!
//!     rz-release keygen <private-key-file>
//!     rz-release sign <version> <platform> <file> <url>        (key in $RZ_UPDATE_KEY)
//!     rz-release manifest <version> <notes-file> <asset.json>...
//!
//! The private key is 32 random bytes as 64 hex characters. It is only ever read from the
//! environment variable `RZ_UPDATE_KEY` (a repository secret in CI), never from a file in
//! the repository.

use ed25519_dalek::{Signer, SigningKey};
use rz_app::update::{hex, signed_message, Asset, Manifest};
use sha2::{Digest, Sha256};
use std::process::ExitCode;

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
        _ => Err(
            "usage: rz-release keygen <file> | sign <version> <platform> <file> <url> | \
                  manifest <version> <notes-file> <asset.json>..."
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
}
