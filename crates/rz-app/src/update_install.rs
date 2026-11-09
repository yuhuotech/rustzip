//! Managed installation records and recoverable update transactions.

use crate::update_package::{self, LINUX_BINARIES};
use rz_core::{CancelToken, Error, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallKind {
    Windows,
    Linux,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installation {
    pub schema: u32,
    pub kind: InstallKind,
    pub root: PathBuf,
    pub machine: bool,
    #[serde(default)]
    pub prefix: Option<PathBuf>,
    #[serde(default)]
    pub data_home: Option<PathBuf>,
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || rz_platform::update::is_link_or_reparse(&metadata)
        || metadata.len() > 65536
    {
        return Err(Error::Corrupt("invalid update record".into()));
    }
    let mut bytes = Vec::new();
    rz_platform::update::open_regular(path)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(Error::Corrupt("update record too large".into()));
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Corrupt("invalid update record".into()))
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::UnsafePath("missing record parent".into()))?;
    let temporary = parent.join(format!(".rustzip-record-{}", transaction_id()?));
    let result = (|| {
        let mut file = rz_platform::update::create_update_file(&temporary, true)?;
        serde_json::to_writer(&mut file, value).map_err(|e| Error::Corrupt(e.to_string()))?;
        file.sync_all()?;
        drop(file);
        rz_platform::update::durable_rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn transaction_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| Error::Corrupt("could not obtain update randomness".into()))?;
    Ok(crate::update::hex(&bytes))
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Installation {
    pub fn read(root: &Path) -> Result<Self> {
        let installation: Self = read_json(&root.join("rustzip-install.json"))?;
        installation.validate(root)?;
        Ok(installation)
    }

    pub fn validate(&self, root: &Path) -> Result<()> {
        if self.schema != 1 || !self.root.is_absolute() || !rz_platform::same_file(&self.root, root)
        {
            return Err(Error::UnsafePath(
                "installation record does not match its location".into(),
            ));
        }
        if self.kind == InstallKind::Linux {
            let prefix = self
                .prefix
                .as_ref()
                .ok_or_else(|| Error::Corrupt("missing installation prefix".into()))?;
            let data = self
                .data_home
                .as_ref()
                .ok_or_else(|| Error::Corrupt("missing installation data location".into()))?;
            if !prefix.is_absolute()
                || !data.is_absolute()
                || prefix.join("lib/rustzip") != self.root
            {
                return Err(Error::UnsafePath(
                    "invalid Linux installation layout".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn locate(executable: &Path) -> Result<Self> {
        let executable = executable.canonicalize()?;
        if rz_platform::update::externally_managed(&executable) {
            return Err(Error::Unsupported("This installation is managed by its distribution channel. Update it through that channel.".into()));
        }
        for root in executable.ancestors().skip(1).take(5) {
            if root.join("rustzip-install.json").is_file() {
                let installation = Self::read(root)?;
                if rz_platform::same_file(&installation.executable(), &executable) {
                    return Ok(installation);
                }
            }
        }
        Err(Error::Unsupported("This installation is managed by another channel. Install the official RustZIP package to use in-app updates.".into()))
    }

    pub fn executable(&self) -> PathBuf {
        match self.kind {
            InstallKind::Windows => self.root.join("rustzip.exe"),
            InstallKind::Linux => self.root.join("current/rustzip"),
        }
    }

    pub fn helper(&self) -> PathBuf {
        match self.kind {
            InstallKind::Windows => self.root.join("rz-updater.exe"),
            InstallKind::Linux => self.root.join("current/rz-updater"),
        }
    }

    pub fn journal(&self) -> PathBuf {
        match self.kind {
            InstallKind::Linux => self.root.join("transaction.json"),
            InstallKind::Windows => {
                let digest = blake3::hash(self.root.as_os_str().as_encoded_bytes());
                self.root
                    .parent()
                    .unwrap_or(&self.root)
                    .join(format!(".rustzip-{}.json", &digest.to_hex()[..16]))
            }
        }
    }

    pub fn lock(&self) -> Result<File> {
        let lock_path = self.journal().with_extension("lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        file.try_lock().map_err(|_| {
            Error::Unsupported(
                "Another RustZIP update is already in progress. Wait for it to finish.".into(),
            )
        })?;
        Ok(file)
    }

    pub fn verify_registration(&self) -> Result<()> {
        if self.kind == InstallKind::Windows
            && !rz_platform::same_file(
                &self.root,
                &rz_platform::update::registered_windows_root(self.machine)?,
            )
        {
            return Err(Error::UnsafePath(
                "Windows installation registration does not match".into(),
            ));
        }
        Ok(())
    }
}

pub fn copy_tree(source: &Path, target: &Path, cancel: &CancelToken) -> Result<()> {
    rz_platform::update::installation_directory(target)?;
    copy_tree_into(source, target, cancel)
}

fn copy_tree_into(source: &Path, target: &Path, cancel: &CancelToken) -> Result<()> {
    let mut budget = CopyBudget {
        entries: 0,
        bytes: 0,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(180),
    };
    copy_contents(source, target, cancel, 0, &mut budget)
}

struct CopyBudget {
    entries: usize,
    bytes: u64,
    deadline: std::time::Instant,
}

fn copy_contents(
    source: &Path,
    target: &Path,
    cancel: &CancelToken,
    depth: usize,
    budget: &mut CopyBudget,
) -> Result<()> {
    if depth > 32 {
        return Err(Error::LimitExceeded(
            "installation directory is too deep".into(),
        ));
    }
    for entry in fs::read_dir(source)? {
        cancel.check()?;
        budget.entries += 1;
        if budget.entries > 100000 || std::time::Instant::now() > budget.deadline {
            return Err(Error::LimitExceeded(
                "installation copy exceeded its work budget".into(),
            ));
        }
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        let destination = target.join(entry.file_name());
        if rz_platform::update::is_link_or_reparse(&metadata) {
            return Err(Error::UnsafePath(
                "installation contains an unexpected link".into(),
            ));
        }
        if metadata.is_dir() {
            rz_platform::update::installation_directory(&destination)?;
            copy_contents(&entry.path(), &destination, cancel, depth + 1, budget)?;
        } else if metadata.is_file() {
            let mut input = rz_platform::update::open_regular(&entry.path())?;
            let mut output = rz_platform::update::create_update_file(&destination, true)?;
            let mut buffer = [0u8; 65536];
            loop {
                cancel.check()?;
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                budget.bytes = budget.bytes.saturating_add(n as u64);
                if budget.bytes > 8 * 1024 * 1024 * 1024
                    || std::time::Instant::now() > budget.deadline
                {
                    return Err(Error::LimitExceeded(
                        "installation copy exceeded its size or time budget".into(),
                    ));
                }
                output.write_all(&buffer[..n])?;
            }
            output.sync_all()?;
            drop(output);
            rz_platform::update::copy_update_permissions(&entry.path(), &destination)?;
        } else {
            return Err(Error::UnsafePath(
                "installation contains a special file".into(),
            ));
        }
    }
    rz_platform::update::copy_update_permissions(source, target)?;
    rz_platform::update::sync_directory(target)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Prepared,
    BackedUp,
    Published,
    Confirmed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transaction {
    pub schema: u32,
    pub id: String,
    pub version: String,
    pub phase: Phase,
    pub previous: String,
    pub updater_pid: u32,
    pub updater_started: u64,
    pub previous_licenses: Vec<String>,
    pub next_licenses: Vec<String>,
}

struct PreparationGuard {
    path: PathBuf,
    retained: bool,
}
impl Drop for PreparationGuard {
    fn drop(&mut self) {
        if !self.retained {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

impl Transaction {
    pub fn read(installation: &Installation) -> Result<Self> {
        let record: Self = read_json(&installation.journal())?;
        if record.schema != 1
            || !valid_id(&record.id)
            || semver::Version::parse(&record.version).is_err()
        {
            return Err(Error::Corrupt("invalid update transaction".into()));
        }
        if installation.kind == InstallKind::Linux && !valid_release(&record.previous) {
            return Err(Error::UnsafePath("invalid previous release".into()));
        }
        if record.previous_licenses.len() > 256
            || record.next_licenses.len() > 256
            || record
                .previous_licenses
                .iter()
                .chain(&record.next_licenses)
                .any(|name| !valid_license(name))
        {
            return Err(Error::UnsafePath("invalid update license records".into()));
        }
        Ok(record)
    }

    pub fn persist(&self, installation: &Installation) -> Result<()> {
        write_json(&installation.journal(), self)
    }

    fn next(&self, installation: &Installation) -> PathBuf {
        match installation.kind {
            InstallKind::Linux => installation.root.join("releases").join(&self.id),
            InstallKind::Windows => installation
                .root
                .parent()
                .unwrap_or(&installation.root)
                .join(format!(".RustZIP-next-{}", self.id)),
        }
    }
    fn backup(&self, installation: &Installation) -> PathBuf {
        installation
            .root
            .parent()
            .unwrap_or(&installation.root)
            .join(format!(".RustZIP-old-{}", self.id))
    }

    pub fn prepare(
        installation: &Installation,
        staged: &Path,
        version: &str,
        id: &str,
        cancel: &CancelToken,
    ) -> Result<Self> {
        if !valid_id(id) {
            return Err(Error::Corrupt(
                "invalid update transaction identifier".into(),
            ));
        }
        let identity = rz_platform::update::process(std::process::id())
            .ok_or_else(|| Error::Corrupt("could not identify updater process".into()))?;
        let mut transaction = Self {
            schema: 1,
            id: id.into(),
            version: version.into(),
            phase: Phase::Prepared,
            previous: String::new(),
            updater_pid: identity.pid,
            updater_started: identity.started,
            previous_licenses: Vec::new(),
            next_licenses: Vec::new(),
        };
        let next = transaction.next(installation);
        rz_platform::update::installation_directory(&next)?;
        let mut guard = PreparationGuard {
            path: next.clone(),
            retained: false,
        };
        match installation.kind {
            InstallKind::Linux => {
                transaction.previous_licenses = license_names(&installation.root.join("current"))?;
                transaction.next_licenses = license_names(staged)?;
                transaction.validate_license_links(installation)?;
                transaction.previous = fs::read_link(installation.root.join("current"))?
                    .to_str()
                    .ok_or_else(|| Error::Corrupt("invalid current release".into()))?
                    .into();
                if !valid_release(&transaction.previous) {
                    return Err(Error::UnsafePath("invalid current release link".into()));
                }
                copy_tree_into(staged, &next, cancel)?;
                rz_platform::apply_mode(&next, 0o755)?;
            }
            InstallKind::Windows => {
                copy_tree_into(&installation.root, &next, cancel)?;
                for entry in fs::read_dir(staged)? {
                    let entry = entry?;
                    let destination = next.join(entry.file_name());
                    if entry.file_type()?.is_dir() {
                        if !destination.exists() {
                            rz_platform::update::installation_directory(&destination)?;
                        }
                        for child in fs::read_dir(entry.path())? {
                            let child = child?;
                            let output = destination.join(child.file_name());
                            if output.exists() {
                                fs::remove_file(&output)?;
                            }
                            let mut input = File::open(child.path())?;
                            let mut target =
                                rz_platform::update::create_update_file(&output, true)?;
                            std::io::copy(&mut input, &mut target)?;
                            target.sync_all()?;
                        }
                    } else {
                        if destination.exists() {
                            fs::remove_file(&destination)?;
                        }
                        let mut input = File::open(entry.path())?;
                        let mut target =
                            rz_platform::update::create_update_file(&destination, true)?;
                        std::io::copy(&mut input, &mut target)?;
                        target.sync_all()?;
                    }
                }
            }
        }
        transaction.persist(installation)?;
        guard.retained = true;
        Ok(transaction)
    }

    pub fn publish(&mut self, installation: &Installation) -> Result<()> {
        match installation.kind {
            InstallKind::Linux => {
                self.phase = Phase::BackedUp;
                self.persist(installation)?;
                self.publish_license_links(installation)?;
                switch_linux(installation, &format!("releases/{}", self.id))?;
            }
            InstallKind::Windows => {
                rz_platform::update::durable_rename(
                    &installation.root,
                    &self.backup(installation),
                )?;
                self.phase = Phase::BackedUp;
                self.persist(installation)?;
                rz_platform::update::durable_rename(&self.next(installation), &installation.root)?;
                rz_platform::update::sync_directory(
                    installation.root.parent().unwrap_or(&installation.root),
                )?;
            }
        }
        self.phase = Phase::Published;
        self.persist(installation)
    }

    pub fn confirm(&mut self, installation: &Installation) -> Result<()> {
        self.phase = Phase::Confirmed;
        self.persist(installation)?;
        Ok(())
    }

    pub fn rollback(&self, installation: &Installation) -> Result<()> {
        match installation.kind {
            InstallKind::Linux => {
                if self.phase != Phase::Prepared {
                    switch_linux(installation, &self.previous)?;
                }
            }
            InstallKind::Windows => {
                let backup = self.backup(installation);
                if backup.is_dir() {
                    if installation.root.exists() {
                        let next = self.next(installation);
                        if next.exists() {
                            fs::remove_dir_all(&next)?;
                        }
                        rz_platform::update::durable_rename(&installation.root, &next)?;
                    }
                    rz_platform::update::durable_rename(&backup, &installation.root)?;
                }
            }
        }
        self.cleanup(installation)
    }

    pub fn cleanup(&self, installation: &Installation) -> Result<()> {
        if installation.kind == InstallKind::Linux {
            let (remove, keep) = if self.phase == Phase::Confirmed {
                (&self.previous_licenses, &self.next_licenses)
            } else {
                (&self.next_licenses, &self.previous_licenses)
            };
            let documents = license_directory(installation)?;
            for name in remove.iter().filter(|name| !keep.contains(name)) {
                let path = documents.join(name);
                let target = installation.root.join("current").join(name);
                if fs::read_link(&path).ok().as_ref() == Some(&target) {
                    fs::remove_file(path)?;
                }
            }
            if documents.exists() {
                rz_platform::update::sync_directory(&documents)?;
            }
        }
        let disposable = match installation.kind {
            InstallKind::Linux => {
                if self.phase == Phase::Confirmed {
                    installation.root.join(&self.previous)
                } else {
                    self.next(installation)
                }
            }
            InstallKind::Windows => {
                if self.phase == Phase::Confirmed {
                    self.backup(installation)
                } else {
                    self.next(installation)
                }
            }
        };
        if disposable.exists() {
            fs::remove_dir_all(disposable)?;
        }
        if installation.journal().exists() {
            fs::remove_file(installation.journal())?;
        }
        Ok(())
    }

    fn validate_license_links(&self, installation: &Installation) -> Result<()> {
        let directory = license_directory(installation)?;
        if installation.machine {
            rz_platform::update::validate_system_directory(&directory)?;
        }
        for name in &self.next_licenses {
            let path = directory.join(name);
            if fs::symlink_metadata(&path).is_ok()
                && fs::read_link(&path).ok() != Some(installation.root.join("current").join(name))
            {
                return Err(Error::UnsafePath(
                    "a license destination belongs to another file".into(),
                ));
            }
        }
        Ok(())
    }

    fn publish_license_links(&self, installation: &Installation) -> Result<()> {
        if self.next_licenses.is_empty() && self.previous_licenses.is_empty() {
            return Ok(());
        }
        self.validate_license_links(installation)?;
        let directory = license_directory(installation)?;
        for name in &self.next_licenses {
            let path = directory.join(name);
            if fs::symlink_metadata(&path).is_err() {
                rz_platform::update::link_file(
                    &installation.root.join("current").join(name),
                    &path,
                )?;
            }
        }
        rz_platform::update::sync_directory(&directory)?;
        Ok(())
    }
}

fn valid_license(name: &str) -> bool {
    name.len() <= 120
        && name.ends_with(".txt")
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn license_names(directory: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if let Some(name) = entry
            .file_name()
            .to_str()
            .filter(|name| valid_license(name))
        {
            names.push(name.to_owned());
        }
    }
    if names.len() > 256 {
        return Err(Error::LimitExceeded("too many license files".into()));
    }
    names.sort();
    Ok(names)
}

fn license_directory(installation: &Installation) -> Result<PathBuf> {
    Ok(installation
        .prefix
        .as_ref()
        .ok_or_else(|| Error::Corrupt("missing Linux installation prefix".into()))?
        .join("share/doc/rustzip"))
}

fn valid_release(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(n)) if n == "releases")
        && matches!(components.next(), Some(Component::Normal(n)) if n.to_str().is_some_and(valid_id))
        && components.next().is_none()
}

fn switch_linux(installation: &Installation, release: &str) -> Result<()> {
    if !valid_release(release) || !installation.root.join(release).is_dir() {
        return Err(Error::UnsafePath("invalid release switch".into()));
    }
    let temporary = installation
        .root
        .join(format!("current-{}", transaction_id()?));
    rz_platform::update::link_directory(Path::new(release), &temporary)?;
    if let Err(error) =
        rz_platform::update::durable_rename(&temporary, &installation.root.join("current"))
    {
        let _ = fs::remove_file(temporary);
        return Err(error.into());
    }
    rz_platform::update::sync_directory(&installation.root)?;
    Ok(())
}

pub fn install_linux(
    source: &Path,
    prefix: &Path,
    data_home: &Path,
    uninstall: bool,
) -> Result<()> {
    let prefix = absolute(prefix)?;
    let root = prefix.join("lib/rustzip");
    if uninstall {
        let installation = Installation::read(&root)?;
        let _lock = installation.lock()?;
        for (destination, target) in linux_links(&installation)? {
            if fs::read_link(&destination).ok().as_ref() == Some(&target) {
                fs::remove_file(destination)?;
            }
        }
        fs::remove_dir_all(&root)?;
        return Ok(());
    }
    fs::create_dir_all(&prefix)?;
    let prefix = prefix.canonicalize()?;
    let existing = prefix.join("lib/rustzip/rustzip-install.json").exists();
    if !existing && prefix.join("bin/rustzip").exists() {
        let old = prefix.join("bin/rustzip");
        if rz_platform::update::externally_managed(&old)
            || !prefix
                .join("share/doc/rustzip/format-readers-licenses.txt")
                .is_file()
            || !prefix.join("share/applications/rustzip.desktop").is_file()
        {
            return Err(Error::Unsupported("The selected prefix contains an installation from another channel. Remove it with its original package manager first.".into()));
        }
    }
    fs::create_dir_all(data_home)?;
    let data_home = data_home.canonicalize()?;
    let root = prefix.join("lib/rustzip");
    fs::create_dir_all(root.join("releases"))?;
    let installation = Installation {
        schema: 1,
        kind: InstallKind::Linux,
        root,
        machine: rz_platform::update::elevated(),
        prefix: Some(prefix),
        data_home: Some(data_home),
    };
    let _lock = installation.lock()?;
    if installation.root.join("rustzip-install.json").exists() {
        let previous = Installation::read(&installation.root)?;
        if previous.data_home != installation.data_home {
            return Err(Error::Unsupported("The existing installation uses a different XDG data directory. Use its original installation paths.".into()));
        }
    }
    let platform = crate::update::current_platform();
    update_package::validate_layout(source, env!("CARGO_PKG_VERSION"), &platform)?;
    let id = transaction_id()?;
    let release = installation.root.join("releases").join(&id);
    copy_tree(source, &release, &CancelToken::new())?;
    for (destination, target) in linux_links_for(&installation, &release)? {
        if let Ok(metadata) = fs::symlink_metadata(&destination) {
            if metadata.is_dir() {
                fs::remove_dir_all(&release)?;
                return Err(Error::UnsafePath(
                    "an integration destination is an existing directory".into(),
                ));
            }
            if metadata.file_type().is_symlink() && fs::read_link(&destination)? != target {
                fs::remove_dir_all(&release)?;
                return Err(Error::UnsafePath(
                    "an integration destination belongs to another installation".into(),
                ));
            }
        }
    }
    write_json(
        &installation.root.join("rustzip-install.json"),
        &installation,
    )?;
    switch_linux(&installation, &format!("releases/{id}"))?;
    for (destination, target) in linux_links(&installation)? {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Ok(metadata) = fs::symlink_metadata(&destination) {
            if metadata.is_dir() {
                return Err(Error::UnsafePath(
                    "a desktop integration path is a directory".into(),
                ));
            }
            fs::remove_file(&destination)?;
        }
        rz_platform::update::link_file(&target, &destination)?;
    }
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn linux_links(installation: &Installation) -> Result<Vec<(PathBuf, PathBuf)>> {
    linux_links_for(installation, &installation.root.join("current"))
}

fn linux_links_for(installation: &Installation, listing: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let prefix = installation
        .prefix
        .as_ref()
        .ok_or_else(|| Error::Corrupt("missing prefix".into()))?;
    let data = installation
        .data_home
        .as_ref()
        .ok_or_else(|| Error::Corrupt("missing data directory".into()))?;
    let current = installation.root.join("current");
    let mut links = Vec::new();
    for binary in LINUX_BINARIES {
        links.push((prefix.join("bin").join(binary), current.join(binary)));
    }
    links.push((
        prefix.join("share/applications/rustzip.desktop"),
        current.join("rustzip.desktop"),
    ));
    for directory in ["kio/servicemenus", "kservices5/ServiceMenus"] {
        links.push((
            data.join(directory).join("rustzip-servicemenu.desktop"),
            current.join("rustzip-servicemenu.desktop"),
        ));
    }
    for name in [
        "rustzip-compress.nemo_action",
        "rustzip-extract.nemo_action",
    ] {
        links.push((data.join("nemo/actions").join(name), current.join(name)));
    }
    for (destination, source) in [
        (
            "RustZIP - Compress selected items",
            "rustzip-nautilus-compress",
        ),
        (
            "RustZIP - Extract archives here",
            "rustzip-nautilus-extract-here",
        ),
    ] {
        links.push((
            data.join("nautilus/scripts").join(destination),
            current.join(source),
        ));
    }
    links.push((
        data.join("nautilus/scripts/.rustzip/rustzip"),
        prefix.join("bin/rustzip"),
    ));
    for size in [16, 32, 48, 64, 128, 256, 512] {
        links.push((
            prefix.join(format!(
                "share/icons/hicolor/{size}x{size}/apps/rustzip.png"
            )),
            current.join(format!("icons/{size}.png")),
        ));
    }
    for entry in fs::read_dir(listing)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|s| s.ends_with(".txt"))
        {
            links.push((
                prefix.join("share/doc/rustzip").join(entry.file_name()),
                current.join(entry.file_name()),
            ));
        }
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_install(parent: &Path) -> Installation {
        let root = parent.join("app");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("rustzip.exe"), b"old program").unwrap();
        fs::write(root.join("unins000.exe"), b"uninstaller").unwrap();
        fs::write(root.join("custom.txt"), b"retain me").unwrap();
        fs::create_dir(root.join("licenses")).unwrap();
        fs::write(root.join("licenses/custom.txt"), b"retain license").unwrap();
        let installation = Installation {
            schema: 1,
            kind: InstallKind::Windows,
            root,
            machine: false,
            prefix: None,
            data_home: None,
        };
        write_json(
            &installation.root.join("rustzip-install.json"),
            &installation,
        )
        .unwrap();
        installation
    }

    fn new_files(parent: &Path) -> PathBuf {
        let staged = parent.join("payload");
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("rustzip.exe"), b"new program").unwrap();
        fs::create_dir(staged.join("licenses")).unwrap();
        fs::write(staged.join("licenses/new.txt"), b"new license").unwrap();
        staged
    }

    #[test]
    fn windows_directory_swap_preserves_uninstaller_and_unmanaged_files() {
        let temp = tempfile::tempdir().unwrap();
        let installation = windows_install(temp.path());
        let staged = new_files(temp.path());
        let mut transaction = Transaction::prepare(
            &installation,
            &staged,
            "0.2.0",
            &transaction_id().unwrap(),
            &CancelToken::new(),
        )
        .unwrap();
        transaction.publish(&installation).unwrap();
        assert_eq!(
            fs::read(installation.root.join("rustzip.exe")).unwrap(),
            b"new program"
        );
        assert_eq!(
            fs::read(installation.root.join("unins000.exe")).unwrap(),
            b"uninstaller"
        );
        assert_eq!(
            fs::read(installation.root.join("custom.txt")).unwrap(),
            b"retain me"
        );
        assert!(installation.root.join("licenses/custom.txt").exists());
        transaction.confirm(&installation).unwrap();
        transaction.cleanup(&installation).unwrap();
        assert!(!installation.journal().exists());
        assert!(!transaction.backup(&installation).exists());
    }

    #[test]
    fn windows_recovers_even_if_process_stops_between_rename_and_journal_write() {
        let temp = tempfile::tempdir().unwrap();
        let installation = windows_install(temp.path());
        let staged = new_files(temp.path());
        let transaction = Transaction::prepare(
            &installation,
            &staged,
            "0.2.0",
            &transaction_id().unwrap(),
            &CancelToken::new(),
        )
        .unwrap();
        fs::rename(&installation.root, transaction.backup(&installation)).unwrap();
        let recovered = Transaction::read(&installation).unwrap();
        recovered.rollback(&installation).unwrap();
        assert_eq!(
            fs::read(installation.root.join("rustzip.exe")).unwrap(),
            b"old program"
        );
        assert!(!installation.journal().exists());
    }

    #[test]
    fn published_windows_update_can_be_rolled_back_repeatedly() {
        let temp = tempfile::tempdir().unwrap();
        let installation = windows_install(temp.path());
        let mut transaction = Transaction::prepare(
            &installation,
            &new_files(temp.path()),
            "0.2.0",
            &transaction_id().unwrap(),
            &CancelToken::new(),
        )
        .unwrap();
        transaction.publish(&installation).unwrap();
        transaction.rollback(&installation).unwrap();
        transaction.rollback(&installation).unwrap();
        assert_eq!(
            fs::read(installation.root.join("rustzip.exe")).unwrap(),
            b"old program"
        );
    }

    #[test]
    fn installation_lock_is_released_when_the_owner_drops_it() {
        let temp = tempfile::tempdir().unwrap();
        let installation = windows_install(temp.path());
        let first = installation.lock().unwrap();
        assert!(installation.lock().is_err());
        drop(first);
        assert!(installation.lock().is_ok());
    }

    #[test]
    fn failed_preparation_never_deletes_a_preexisting_staging_directory() {
        let temp = tempfile::tempdir().unwrap();
        let installation = windows_install(temp.path());
        let id = transaction_id().unwrap();
        let existing = temp.path().join(format!(".RustZIP-next-{id}"));
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("retain.txt"), b"preexisting data").unwrap();
        assert!(Transaction::prepare(
            &installation,
            &new_files(temp.path()),
            "0.2.0",
            &id,
            &CancelToken::new()
        )
        .is_err());
        assert_eq!(
            fs::read(existing.join("retain.txt")).unwrap(),
            b"preexisting data"
        );
    }

    #[cfg(windows)]
    #[test]
    fn installation_copy_rejects_directory_junctions() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let outside = temp.path().join("outside");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("private.txt"), b"must not be copied").unwrap();
        let junction = source.join("junction");
        let status = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(copy_tree(&source, &temp.path().join("target"), &CancelToken::new()).is_err());
        assert!(!temp.path().join("target/junction/private.txt").exists());
        fs::remove_dir(junction).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn linux_switch_and_rollback_keep_a_complete_current_version() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lib/rustzip");
        let old = transaction_id().unwrap();
        fs::create_dir_all(root.join("releases").join(&old)).unwrap();
        fs::write(root.join("releases").join(&old).join("rustzip"), b"old").unwrap();
        std::os::unix::fs::symlink(format!("releases/{old}"), root.join("current")).unwrap();
        let installation = Installation {
            schema: 1,
            kind: InstallKind::Linux,
            root,
            machine: false,
            prefix: Some(temp.path().to_path_buf()),
            data_home: Some(temp.path().join("share")),
        };
        let staged = temp.path().join("payload");
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("rustzip"), b"new").unwrap();
        let mut transaction = Transaction::prepare(
            &installation,
            &staged,
            "0.2.0",
            &transaction_id().unwrap(),
            &CancelToken::new(),
        )
        .unwrap();
        transaction.publish(&installation).unwrap();
        assert_eq!(fs::read(installation.executable()).unwrap(), b"new");
        transaction.rollback(&installation).unwrap();
        assert_eq!(fs::read(installation.executable()).unwrap(), b"old");
        assert!(installation.root.join(format!("releases/{old}")).exists());
    }

    #[cfg(unix)]
    #[test]
    fn transaction_cannot_redirect_recovery_outside_the_installation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lib/rustzip");
        fs::create_dir_all(&root).unwrap();
        let installation = Installation {
            schema: 1,
            kind: InstallKind::Linux,
            root,
            machine: false,
            prefix: Some(temp.path().to_path_buf()),
            data_home: Some(temp.path().join("share")),
        };
        let transaction = Transaction {
            schema: 1,
            id: transaction_id().unwrap(),
            version: "0.2.0".into(),
            phase: Phase::Published,
            previous: "../../victim".into(),
            updater_pid: 0,
            updater_started: 0,
            previous_licenses: Vec::new(),
            next_licenses: Vec::new(),
        };
        transaction.persist(&installation).unwrap();
        assert!(Transaction::read(&installation).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn linux_license_links_follow_commit_and_rollback_without_touching_user_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lib/rustzip");
        let old = transaction_id().unwrap();
        fs::create_dir_all(root.join("releases").join(&old)).unwrap();
        fs::write(
            root.join("releases").join(&old).join("old-license.txt"),
            b"old license",
        )
        .unwrap();
        std::os::unix::fs::symlink(format!("releases/{old}"), root.join("current")).unwrap();
        let installation = Installation {
            schema: 1,
            kind: InstallKind::Linux,
            root,
            machine: false,
            prefix: Some(temp.path().to_owned()),
            data_home: Some(temp.path().join("share")),
        };
        let documents = license_directory(&installation).unwrap();
        fs::create_dir_all(&documents).unwrap();
        std::os::unix::fs::symlink(
            installation.root.join("current/old-license.txt"),
            documents.join("old-license.txt"),
        )
        .unwrap();
        fs::write(documents.join("my-notes.txt"), b"keep my notes").unwrap();
        let staged = temp.path().join("staged");
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("new-license.txt"), b"new license").unwrap();
        let mut first = Transaction::prepare(
            &installation,
            &staged,
            "0.2.0",
            &transaction_id().unwrap(),
            &CancelToken::new(),
        )
        .unwrap();
        first.publish(&installation).unwrap();
        assert!(documents.join("new-license.txt").is_file());
        first.rollback(&installation).unwrap();
        assert!(documents.join("old-license.txt").is_file());
        assert!(fs::symlink_metadata(documents.join("new-license.txt")).is_err());
        let mut second = Transaction::prepare(
            &installation,
            &staged,
            "0.2.0",
            &transaction_id().unwrap(),
            &CancelToken::new(),
        )
        .unwrap();
        second.publish(&installation).unwrap();
        second.confirm(&installation).unwrap();
        second.cleanup(&installation).unwrap();
        assert!(documents.join("new-license.txt").is_file());
        assert!(fs::symlink_metadata(documents.join("old-license.txt")).is_err());
        assert_eq!(
            fs::read(documents.join("my-notes.txt")).unwrap(),
            b"keep my notes"
        );
    }
}
