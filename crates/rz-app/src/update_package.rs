//! Restricted staging for signed Windows update packages.
//!
//! Staging alone does not authorize installation. The platform updater must
//! independently check the signed task and installation identity before use.

use crate::update::{verify_asset_reader, Asset};
use rz_core::{CancelToken, Error, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAX_FILES: usize = 256;
const MAX_EXPANDED: u64 = 1024 * 1024 * 1024;
const MAX_LAYOUT: u64 = 4096;
const REQUIRED: [&str; 5] = [
    "rustzip.exe",
    "rz-updater.exe",
    "rz-pdf-renderer.exe",
    "rz-video-renderer.exe",
    "update-layout.json",
];

pub struct StagingDirectory(std::path::PathBuf);
impl StagingDirectory {
    fn new(parent: &Path) -> Result<Self> {
        Self::create(parent, ".rustzip-update-")
    }
    pub fn create(parent: &Path, prefix: &str) -> Result<Self> {
        if prefix.contains(['/', '\\']) {
            return Err(Error::UnsafePath("invalid update directory prefix".into()));
        }
        let path = parent.join(format!(
            "{prefix}{}",
            crate::update_install::transaction_id()?
        ));
        rz_platform::update::private_directory(&path)?;
        Ok(Self(path))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn keep(mut self) -> std::path::PathBuf {
        std::mem::take(&mut self.0)
    }
}
impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Debug, Clone, serde::Serialize)]
pub struct Layout {
    pub schema: u32,
    pub version: String,
    pub platform: String,
}

pub const LINUX_BINARIES: [&str; 4] = [
    "rustzip",
    "rz-updater",
    "rz-pdf-renderer",
    "rz-video-renderer",
];
pub const LINUX_RESOURCES: [&str; 7] = [
    "install.sh",
    "rustzip.desktop",
    "rustzip-servicemenu.desktop",
    "rustzip-compress.nemo_action",
    "rustzip-extract.nemo_action",
    "rustzip-nautilus-compress",
    "rustzip-nautilus-extract-here",
];

fn linux_name(name: &str) -> bool {
    LINUX_BINARIES.contains(&name)
        || LINUX_RESOURCES.contains(&name)
        || name == "update-layout.json"
        || name.strip_prefix("icons/").is_some_and(|n| {
            [
                "16.png", "32.png", "48.png", "64.png", "128.png", "256.png", "512.png",
            ]
            .contains(&n)
        })
        || (!name.contains('/') && license_name(&format!("licenses/{name}")))
}

/// Stages a signed official Linux tarball without executing its install script.
#[allow(clippy::too_many_arguments)]
pub fn stage_linux_update(
    mut archive: std::fs::File,
    parent: &Path,
    version: &str,
    asset: &Asset,
    public_key: &[u8; 32],
    cancel: &CancelToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<StagingDirectory> {
    let reject = |message: &str| Error::Corrupt(format!("update package: {message}"));
    if !matches!(asset.platform.as_str(), "linux-x86_64" | "linux-aarch64") {
        return Err(reject("wrong platform"));
    }
    archive.seek(SeekFrom::Start(0))?;
    let verified = verify_asset_reader(
        CancellableReader {
            file: &mut archive,
            cancel,
        },
        version,
        asset,
        public_key,
    );
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    verified?;
    archive.seek(SeekFrom::Start(0))?;
    let mut tar = tar::Archive::new(flate2::read::MultiGzDecoder::new(archive));
    let staging = StagingDirectory::new(parent)?;
    rz_platform::update::installation_directory(&staging.path().join("icons"))?;
    let prefix = format!("RustZIP-{version}-{}/", asset.platform);
    let mut names = HashSet::new();
    let mut total = 0u64;
    let mut count = 0;
    for entry in tar.entries()?.raw(true) {
        cancel.check()?;
        count += 1;
        if count > MAX_FILES {
            return Err(reject("too many update files"));
        }
        let mut entry = entry?;
        let raw = entry.path_bytes();
        let name = std::str::from_utf8(&raw).map_err(|_| reject("invalid filename"))?;
        let name = name
            .strip_prefix(&prefix)
            .ok_or_else(|| reject("unexpected package root"))?
            .to_owned();
        let kind = entry.header().entry_type();
        if kind.is_dir() && matches!(name.as_str(), "" | "icons" | "icons/") {
            continue;
        }
        if !kind.is_file() || !linux_name(&name) || !names.insert(name.clone()) {
            return Err(reject("unexpected, duplicate or special file"));
        }
        let size = entry.size();
        if name == "update-layout.json" && size > MAX_LAYOUT {
            return Err(reject("layout is too large"));
        }
        if total.checked_add(size).is_none_or(|n| n > MAX_EXPANDED) {
            return Err(reject("expanded size limit"));
        }
        let output = staging.path().join(&name);
        let mut target = rz_platform::update::create_update_file(&output, false)?;
        let mut written = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            cancel.check()?;
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > size {
                return Err(reject("entry size mismatch"));
            }
            target.write_all(&buffer[..n])?;
            progress(total + written, 0);
        }
        if written != size {
            return Err(reject("truncated file"));
        }
        target.sync_all()?;
        rz_platform::apply_mode(
            &output,
            if LINUX_BINARIES.contains(&name.as_str())
                || matches!(
                    name.as_str(),
                    "install.sh"
                        | "rustzip-servicemenu.desktop"
                        | "rustzip-nautilus-compress"
                        | "rustzip-nautilus-extract-here"
                )
            {
                0o755
            } else {
                0o644
            },
        )?;
        total += written;
    }
    let mut remainder = tar.into_inner();
    let mut padding = 0usize;
    let mut buffer = [0u8; 65536];
    loop {
        cancel.check()?;
        let count = remainder.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        padding += count;
        if padding > 1024 * 1024 || buffer[..count].iter().any(|b| *b != 0) {
            return Err(reject("unexpected trailing tar data"));
        }
    }
    if LINUX_BINARIES
        .iter()
        .chain(LINUX_RESOURCES.iter())
        .any(|n| !names.contains(*n))
        || !names.contains("update-layout.json")
    {
        return Err(reject("required files are missing"));
    }
    for size in [16, 32, 48, 64, 128, 256, 512] {
        if !names.contains(&format!("icons/{size}.png")) {
            return Err(reject("required icons are missing"));
        }
    }
    validate_layout(staging.path(), version, &asset.platform)?;
    for name in LINUX_BINARIES {
        let mut header = [0u8; 64];
        File::open(staging.path().join(name))?.read_exact(&mut header)?;
        let machine = u16::from_le_bytes([header[18], header[19]]);
        if &header[..4] != b"\x7fELF"
            || header[4] != 2
            || header[5] != 1
            || machine
                != if asset.platform == "linux-x86_64" {
                    62
                } else {
                    183
                }
        {
            return Err(reject("Linux executable architecture mismatch"));
        }
    }
    Ok(staging)
}

pub fn validate_layout(directory: &Path, version: &str, platform: &str) -> Result<Layout> {
    let mut bytes = Vec::new();
    std::fs::File::open(directory.join("update-layout.json"))?
        .take(MAX_LAYOUT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LAYOUT {
        return Err(Error::Corrupt("update layout too large".into()));
    }
    let layout: Layout = serde_json::from_slice(&bytes)
        .map_err(|_| Error::Corrupt("invalid update layout".into()))?;
    if layout.schema != 1 || layout.version != version || layout.platform != platform {
        return Err(Error::Corrupt("update layout identity mismatch".into()));
    }
    Ok(layout)
}

struct CancellableReader<'a> {
    file: &'a mut std::fs::File,
    cancel: &'a CancelToken,
}

impl Read for CancellableReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::other("update verification cancelled"));
        }
        self.file.read(buffer)
    }
}

fn license_name(name: &str) -> bool {
    let Some(name) = name.strip_prefix("licenses/") else {
        return false;
    };
    if name.len() > 120
        || !name.ends_with(".txt")
        || name.starts_with('.')
        || name.contains("..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !matches!(stem.as_bytes(), [b'C', b'O', b'M', b'1'..=b'9'])
        && !matches!(stem.as_bytes(), [b'L', b'P', b'T', b'1'..=b'9'])
}

/// Verifies the signed ZIP and stages its permitted files in a new directory on
/// the target volume. Dropping the result removes that directory. The caller
/// must ensure `parent` is protected from writes by other principals; in
/// particular, an elevated updater must use an administrator-protected parent.
#[allow(clippy::too_many_arguments)]
pub fn stage_windows_update(
    mut archive: std::fs::File,
    parent: &Path,
    version: &str,
    asset: &Asset,
    public_key: &[u8; 32],
    cancel: &CancelToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<StagingDirectory> {
    let reject = |message: &str| Error::Corrupt(format!("update package: {message}"));
    if asset.platform != "windows-x86_64-zip" {
        return Err(reject("wrong platform"));
    }
    archive.seek(SeekFrom::Start(0))?;
    let verification = verify_asset_reader(
        CancellableReader {
            file: &mut archive,
            cancel,
        },
        version,
        asset,
        public_key,
    );
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    verification?;
    archive.seek(SeekFrom::Start(0))?;
    let mut zip =
        zip::ZipArchive::new(archive).map_err(|e| Error::Corrupt(format!("update ZIP: {e}")))?;
    if zip.len() > MAX_FILES {
        return Err(Error::LimitExceeded("too many update files".into()));
    }
    let staging = StagingDirectory::new(parent)?;
    let mut names = HashSet::new();
    let mut declared = 0u64;
    for index in 0..zip.len() {
        cancel.check()?;
        let file = zip
            .by_index(index)
            .map_err(|e| Error::Corrupt(format!("update ZIP: {e}")))?;
        let name = std::str::from_utf8(file.name_raw()).map_err(|_| reject("invalid filename"))?;
        if name == "licenses/" && file.is_dir() {
            continue;
        }
        if file.is_dir()
            || (!REQUIRED.contains(&name) && name != "unrar-license.txt" && !license_name(name))
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err(reject("unexpected or duplicate file"));
        }
        if file.unix_mode().is_some_and(|mode| {
            let kind = mode & 0o170000;
            kind != 0 && kind != 0o100000
        }) {
            return Err(reject("links and special files are not allowed"));
        }
        if !matches!(
            file.compression(),
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
        ) {
            return Err(reject("unsupported update ZIP compression"));
        }
        if name == "update-layout.json" && file.size() > MAX_LAYOUT {
            return Err(reject("layout is too large"));
        }
        declared = declared
            .checked_add(file.size())
            .ok_or_else(|| reject("expanded size overflow"))?;
        if declared > MAX_EXPANDED {
            return Err(Error::LimitExceeded("update ZIP is too large".into()));
        }
    }
    if REQUIRED.iter().any(|name| !names.contains(*name))
        || !names.iter().any(|name| name.starts_with("licenses/"))
    {
        return Err(reject("required files are missing"));
    }
    rz_platform::update::installation_directory(&staging.path().join("licenses"))?;
    let mut done = 0u64;
    for index in 0..zip.len() {
        cancel.check()?;
        let mut file = zip
            .by_index(index)
            .map_err(|e| Error::Corrupt(format!("update ZIP: {e}")))?;
        if file.is_dir() {
            continue;
        }
        let name = std::str::from_utf8(file.name_raw()).map_err(|_| reject("invalid filename"))?;
        let output = staging.path().join(name);
        let mut target = rz_platform::update::create_update_file(&output, false)?;
        let mut buffer = [0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            cancel.check()?;
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            written = written
                .checked_add(count as u64)
                .ok_or_else(|| reject("expanded size overflow"))?;
            if written > file.size() || done.saturating_add(count as u64) > MAX_EXPANDED {
                return Err(reject("entry exceeds its declared size"));
            }
            target.write_all(&buffer[..count])?;
            done += count as u64;
            progress(done, declared);
        }
        if written != file.size() {
            return Err(reject("entry size mismatch"));
        }
        target.sync_all()?;
    }
    let layout_bytes = std::fs::read(staging.path().join("update-layout.json"))?;
    let layout: Layout =
        serde_json::from_slice(&layout_bytes).map_err(|_| reject("invalid layout"))?;
    if layout.schema != 1 || layout.version != version || layout.platform != asset.platform {
        return Err(reject("layout identity mismatch"));
    }
    for name in REQUIRED.into_iter().filter(|name| name.ends_with(".exe")) {
        validate_pe(&staging.path().join(name))?;
    }
    Ok(staging)
}

fn validate_pe(path: &Path) -> Result<()> {
    let mut file = std::fs::File::open(path)?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos)?;
    let offset = u32::from_le_bytes([dos[60], dos[61], dos[62], dos[63]]) as u64;
    if &dos[..2] != b"MZ" || !(64..=1048576).contains(&offset) {
        return Err(Error::Corrupt("invalid Windows update executable".into()));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut pe = [0u8; 6];
    file.read_exact(&mut pe)?;
    if &pe[..4] != b"PE\0\0" || u16::from_le_bytes([pe[4], pe[5]]) != 0x8664 {
        return Err(Error::Corrupt(
            "Windows executable architecture mismatch".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::{hex, signed_message};
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use zip::write::SimpleFileOptions;

    fn package(extra: Option<&str>) -> (std::fs::File, Asset, [u8; 32]) {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for name in REQUIRED {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            if name == "update-layout.json" {
                writer
                    .write_all(br#"{"schema":1,"version":"0.2.0","platform":"windows-x86_64-zip"}"#)
                    .unwrap();
            } else {
                let mut executable = [0u8; 70];
                executable[..2].copy_from_slice(b"MZ");
                executable[60..64].copy_from_slice(&64u32.to_le_bytes());
                executable[64..].copy_from_slice(b"PE\0\0\x64\x86");
                writer.write_all(&executable).unwrap();
            }
        }
        writer
            .start_file("licenses/mit.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"license").unwrap();
        if let Some(name) = extra {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"unexpected").unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        let key = SigningKey::from_bytes(&[7; 32]);
        let sha256 = hex(&Sha256::digest(&bytes));
        let platform = "windows-x86_64-zip";
        let asset = Asset {
            platform: platform.into(),
            url: "https://example.com/update.zip".into(),
            size: bytes.len() as u64,
            signature: hex(&key
                .sign(&signed_message("0.2.0", platform, &sha256))
                .to_bytes()),
            sha256,
        };
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&bytes).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        (file, asset, key.verifying_key().to_bytes())
    }

    #[test]
    fn stages_only_signed_expected_files_and_removes_temporary_data() {
        let parent = tempfile::tempdir().unwrap();
        let (file, asset, key) = package(None);
        let stage = stage_windows_update(
            file,
            parent.path(),
            "0.2.0",
            &asset,
            &key,
            &CancelToken::new(),
            |_, _| {},
        )
        .unwrap();
        assert!(stage.path().join("rustzip.exe").is_file());
        let path = stage.path().to_owned();
        drop(stage);
        assert!(!path.exists());
    }

    #[test]
    fn rejects_escape_overwrite_and_case_collisions() {
        let parent = tempfile::tempdir().unwrap();
        for bad in [
            "../victim",
            "unins000.exe",
            "RUSTZIP.EXE",
            "licenses/CON.txt",
        ] {
            let (file, asset, key) = package(Some(bad));
            assert!(stage_windows_update(
                file,
                parent.path(),
                "0.2.0",
                &asset,
                &key,
                &CancelToken::new(),
                |_, _| {},
            )
            .is_err());
        }
    }

    #[test]
    fn rejects_bad_signatures_and_honors_cancellation() {
        let parent = tempfile::tempdir().unwrap();
        let (file, mut asset, key) = package(None);
        asset.signature = "00".repeat(64);
        assert!(stage_windows_update(
            file,
            parent.path(),
            "0.2.0",
            &asset,
            &key,
            &CancelToken::new(),
            |_, _| {},
        )
        .is_err());
        let (file, asset, key) = package(None);
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            stage_windows_update(
                file,
                parent.path(),
                "0.2.0",
                &asset,
                &key,
                &cancel,
                |_, _| {}
            ),
            Err(Error::Cancelled)
        ));
    }

    fn linux_package(bad: Option<(&str, bool)>) -> (File, Asset, [u8; 32]) {
        let version = "0.2.0";
        let platform = "linux-x86_64";
        let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gzip);
        let mut append = |name: &str, bytes: &[u8], link: bool| {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            if link {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("../../victim").unwrap();
                header.set_size(0);
            } else {
                header.set_size(bytes.len() as u64);
            }
            header.set_cksum();
            tar.append_data(
                &mut header,
                format!("RustZIP-{version}-{platform}/{name}"),
                if link { &[][..] } else { bytes },
            )
            .unwrap();
        };
        let mut elf = [0u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[18..20].copy_from_slice(&62u16.to_le_bytes());
        for binary in LINUX_BINARIES {
            append(binary, &elf, false);
        }
        for resource in LINUX_RESOURCES {
            append(resource, b"resource", false);
        }
        for size in [16, 32, 48, 64, 128, 256, 512] {
            append(&format!("icons/{size}.png"), b"png", false);
        }
        append("license.txt", b"license", false);
        append(
            "update-layout.json",
            br#"{"schema":1,"version":"0.2.0","platform":"linux-x86_64"}"#,
            false,
        );
        if let Some((name, link)) = bad {
            append(name, b"bad", link);
        }
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let sha256 = hex(&Sha256::digest(&bytes));
        let asset = Asset {
            platform: platform.into(),
            url: "https://example.com/linux.tar.gz".into(),
            size: bytes.len() as u64,
            signature: hex(&key
                .sign(&signed_message(version, platform, &sha256))
                .to_bytes()),
            sha256,
        };
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&bytes).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        (file, asset, key.verifying_key().to_bytes())
    }

    #[test]
    fn linux_staging_checks_layout_architecture_and_special_files() {
        let parent = tempfile::tempdir().unwrap();
        let (file, asset, key) = linux_package(None);
        let stage = stage_linux_update(
            file,
            parent.path(),
            "0.2.0",
            &asset,
            &key,
            &CancelToken::new(),
            |_, _| {},
        )
        .unwrap();
        assert!(stage.path().join("rz-updater").exists());
        for bad in [("escape", true), ("rustzip", false), ("etc/passwd", false)] {
            let (file, asset, key) = linux_package(Some(bad));
            assert!(stage_linux_update(
                file,
                parent.path(),
                "0.2.0",
                &asset,
                &key,
                &CancelToken::new(),
                |_, _| {}
            )
            .is_err());
        }
    }
}
