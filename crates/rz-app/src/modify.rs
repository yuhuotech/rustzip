//! Changing an archive that already exists: adding files and deleting entries.
//!
//! Edits are staged beside the original, checked, and only then swapped in.
//! Eligible ZIP deletions, renames and replacements reuse compressed entries; other edits unpack
//! into a scratch folder and repack. Failure or cancellation preserves the original.

use rz_core::{
    clean_path, is_within, CancelToken, CreateOptions, Entry, EntryKind, Error, NoProgress,
    Progress, ProgressSink, Result,
};
use rz_engine::{compress, extract, safe_join, ExtractOptions};
use rz_formats::OutputFormat;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum Edit {
    /// Remove these entries (and everything inside the folders among them).
    Delete(Vec<String>),
    /// Copy these files and folders into the folder `into` of the archive (`""` for the top).
    Add { into: String, sources: Vec<PathBuf> },
    /// Give an entry (file or folder) another name, keeping it in the same folder.
    Rename { path: String, new_name: String },
    /// Put the content of a file on disk in place of an existing file entry. Used to save
    /// a file that was opened from the archive and edited.
    Replace { entry: String, with: PathBuf },
}

/// Whether `name` can be the name of one item: no folders, no tricks.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.trim() == name
        && !name.contains(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'])
        && !name.chars().any(char::is_control)
}

#[derive(Debug, Default)]
pub struct EditReport {
    /// Entries in the archive after the change.
    pub files: u64,
    /// Their total size when unpacked.
    pub bytes: u64,
}

/// The format an archive is written back in, from the name the reader reports for it
/// (`"zip"`, `"7z"`, `"tar.gz"`...). `None` for formats that cannot be rewritten.
pub fn edit_format(detected: &str) -> Option<OutputFormat> {
    use rz_formats::Compression;
    Some(match detected {
        "zip" => OutputFormat::Zip,
        "7z" => OutputFormat::SevenZ,
        "tar" => OutputFormat::Tar(Compression::None),
        "tar.gz" => OutputFormat::Tar(Compression::Gzip),
        "tar.xz" => OutputFormat::Tar(Compression::Xz),
        "tar.zst" => OutputFormat::Tar(Compression::Zstd),
        "tar.bz2" => OutputFormat::Tar(Compression::Bzip2),
        "tar.lz4" => OutputFormat::Tar(Compression::Lz4),
        _ => return None,
    })
}

/// Maps one phase of the work onto its half of the overall progress bar.
struct Half<'a> {
    inner: &'a mut dyn ProgressSink,
    /// Which half this is: 0 for unpacking, 1 for packing.
    phase: u64,
}

impl ProgressSink for Half<'_> {
    fn update(&mut self, p: Progress) {
        // Both halves cover the same data, so the bar simply doubles the totals.
        self.inner.update(Progress {
            files_done: p.files_done + self.phase * p.files_total,
            files_total: p.files_total * 2,
            bytes_done: p.bytes_done + self.phase * p.bytes_total,
            bytes_total: p.bytes_total * 2,
        });
    }
}

/// The scratch folder is removed however the function ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A bounded set of OS-level advisory locks coordinates edits across processes.
/// Hashed, striped names keep user paths out of the lock files and bound cache growth.
const EDIT_LOCK_STRIPES: usize = 256;

fn archive_lock_path(archive: &Path, root: &Path) -> PathBuf {
    let canonical = archive
        .canonicalize()
        .unwrap_or_else(|_| archive.to_path_buf());
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let stripe = usize::from(digest[0]) % EDIT_LOCK_STRIPES;
    root.join(format!("archive-edit-{stripe:02x}.lock"))
}

fn open_archive_lock(archive: &Path, root: &Path) -> Result<File> {
    fs::create_dir_all(root)?;
    #[cfg(unix)]
    rz_platform::apply_mode(root, 0o700)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(archive_lock_path(archive, root))?)
}

fn archive_lock(archive: &Path, cancel: &CancelToken) -> Result<File> {
    let root = crate::settings::config_dir()
        .ok_or_else(|| {
            Error::Unsupported(
                "a user settings directory is required to coordinate archive edits".into(),
            )
        })?
        .join("archive-locks");
    let target = archive
        .canonicalize()
        .unwrap_or_else(|_| archive.to_path_buf());
    archive_lock_at(archive, &root, cancel, || {
        #[cfg(test)]
        if let (Ok(expected), Ok(marker)) = (
            std::env::var("RZ_TEST_EDIT_LOCK_ARCHIVE"),
            std::env::var("RZ_TEST_EDIT_LOCK_WAIT_MARKER"),
        ) {
            if Path::new(&expected)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(expected))
                == target
            {
                let _ = fs::write(marker, b"waiting");
            }
        }
        #[cfg(not(test))]
        let _ = &target;
    })
}

fn archive_lock_at(
    archive: &Path,
    root: &Path,
    cancel: &CancelToken,
    mut on_wait: impl FnMut(),
) -> Result<File> {
    let lock = open_archive_lock(archive, root)?;
    loop {
        cancel.check()?;
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => {
                on_wait();
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

/// Distinct names per edit within this process, so concurrent edits never share
/// a scratch folder or a new-archive file even though they share a PID.
fn unique_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

pub fn edit_archive(
    archive: &Path,
    edit: &Edit,
    opts: &ExtractOptions,
    level: u32,
    encoding: Option<&str>,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<EditReport> {
    // Waiting on a different RustZIP process remains cancellable.
    let _serialization = archive_lock(archive, cancel)?;
    // What the file really is decides, not its name: a `.jar` or `.docx` is a zip.
    let mut reader = rz_formats::open(archive)?;
    let format = edit_format(reader.format_name())
        .ok_or_else(|| Error::Unsupported("only zip, 7z and tar archives can be changed".into()))?;
    let parent = archive
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    if format == OutputFormat::Zip
        && encoding.is_none()
        && opts.password.as_deref().is_none_or(str::is_empty)
        && opts.only.is_none()
        && rz_formats::volumes::zip_volume_paths(archive)?.is_none()
    {
        let entries = reader.entries_with_progress(cancel, &mut NoProgress)?;
        if let Some(mut plan) = zip_copy_plan(&entries, edit, opts, parent) {
            let names: Vec<String> = entries.iter().map(|entry| entry.path.clone()).collect();
            if rz_formats::zip_edit::names_match(File::open(archive)?, &names, cancel)? {
                drop(reader);
                plan.level = level;
                return edit_zip_copy(archive, &entries, plan, cancel, progress);
            }
        }
    }

    // Same folder as the archive, so the final swap is a rename on one disk.
    let scratch = Scratch(parent.join(format!(
        ".{name}.rz-edit-{}-{}",
        std::process::id(),
        unique_seq()
    )));
    let _ = fs::remove_dir_all(&scratch.0);
    let stage = scratch.0.join("files");
    fs::create_dir_all(&stage)?;

    // 1. Unpack everything.
    if let Some(enc) = encoding {
        reader.set_encoding(enc)?;
    }
    extract(
        &mut *reader,
        &stage,
        opts,
        cancel,
        &mut Half {
            inner: progress,
            phase: 0,
        },
    )?;
    drop(reader);
    cancel.check()?;

    // 2. Change the unpacked copy.
    match edit {
        Edit::Delete(paths) => {
            for path in paths {
                let target = safe_join(&stage, &clean_path(path))?;
                match fs::symlink_metadata(&target) {
                    Ok(m) if m.is_dir() => fs::remove_dir_all(&target)?,
                    Ok(_) => fs::remove_file(&target)?,
                    // Already gone, for example because a folder above it was deleted.
                    Err(_) => {}
                }
            }
        }
        Edit::Add { into, sources } => {
            let dest = if clean_path(into).is_empty() {
                stage.clone()
            } else {
                safe_join(&stage, &clean_path(into))?
            };
            fs::create_dir_all(&dest)?;
            for src in sources {
                let file_name = src
                    .file_name()
                    .ok_or_else(|| Error::UnsafePath(src.display().to_string()))?;
                let target = dest.join(file_name);
                // A file with the same name is replaced, like dropping it into a folder.
                match fs::symlink_metadata(&target) {
                    Ok(m) if m.is_dir() => fs::remove_dir_all(&target)?,
                    Ok(_) => fs::remove_file(&target)?,
                    Err(_) => {}
                }
                copy_tree(src, &target, cancel)?;
            }
        }
        Edit::Rename { path, new_name } => {
            if !valid_name(new_name) {
                return Err(Error::UnsafePath(new_name.clone()));
            }
            let from = safe_join(&stage, &clean_path(path))?;
            let to = from
                .parent()
                .ok_or_else(|| Error::UnsafePath(path.clone()))?
                .join(new_name);
            if fs::symlink_metadata(&from).is_err() {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    path.clone(),
                )));
            }
            if fs::symlink_metadata(&to).is_ok() {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    new_name.clone(),
                )));
            }
            fs::rename(&from, &to)?;
        }
        Edit::Replace { entry, with } => {
            let target = safe_join(&stage, &clean_path(entry))?;
            // Only an existing file is replaced; a folder or a missing entry means the
            // archive changed since the file was opened, and guessing would lose data.
            match fs::symlink_metadata(&target) {
                Ok(m) if m.is_file() => {}
                _ => {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        entry.clone(),
                    )))
                }
            }
            fs::remove_file(&target)?;
            copy_tree(with, &target, cancel)?;
        }
    }
    cancel.check()?;

    // 3. Pack the result into a new file next to the original.
    let mut top: Vec<PathBuf> = fs::read_dir(&stage)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    top.sort();
    let new_file = parent.join(format!(
        ".{name}.rz-new-{}-{}",
        std::process::id(),
        unique_seq()
    ));
    // Nothing of ours can be at that path; a leftover from a dead run is removed.
    let _ = fs::remove_file(&new_file);
    let create = CreateOptions {
        level,
        password: opts.password.clone(),
        threads: opts.threads,
        ..CreateOptions::default()
    };
    let result = compress(
        &top,
        &new_file,
        format,
        &create,
        cancel,
        &mut Half {
            inner: progress,
            phase: 1,
        },
    )
    .and_then(|_| verify(&new_file, opts.password.as_deref(), cancel));
    let report = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = fs::remove_file(&new_file);
            return Err(e);
        }
    };

    // 4. Swap it in. A rename within one folder replaces the original in one step.
    cancel.check().inspect_err(|_| {
        let _ = fs::remove_file(&new_file);
    })?;
    if let Err(e) = fs::rename(&new_file, archive) {
        let _ = fs::remove_file(&new_file);
        return Err(e.into());
    }
    Ok(report)
}

struct ZipEditPlan {
    names: Vec<Option<String>>,
    replacement: Option<(usize, PathBuf)>,
    level: u32,
}

fn zip_copy_plan(
    entries: &[Entry],
    edit: &Edit,
    opts: &ExtractOptions,
    root: &Path,
) -> Option<ZipEditPlan> {
    use std::collections::HashMap;
    let mut paths = HashMap::new();
    let mut total = 0u64;
    for entry in entries {
        let path = clean_path(&entry.path);
        if entry.path.contains('\\')
            || path != entry.path.trim_end_matches('/')
            || entry.encrypted
            || entry.kind == EntryKind::Symlink
            || safe_join(root, &entry.path).is_err()
            || paths.insert(path.to_lowercase(), entry.kind).is_some()
        {
            return None;
        }
        total = total.checked_add(entry.size)?;
        if total > opts.max_total
            || (entry.kind == EntryKind::File
                && entry.size > opts.ratio_check_above
                && entry.size > entry.packed_size?.max(1).saturating_mul(opts.max_ratio))
        {
            return None;
        }
    }
    for path in paths.keys() {
        let mut prefix = path.as_str();
        while let Some((parent, _)) = prefix.rsplit_once('/') {
            if paths.get(parent) == Some(&EntryKind::File) {
                return None;
            }
            prefix = parent;
        }
    }
    match edit {
        Edit::Delete(selected) if !selected.is_empty() => {
            let selected: Vec<String> = selected.iter().map(|path| clean_path(path)).collect();
            if selected
                .iter()
                .any(|path| path.is_empty() || safe_join(root, path).is_err())
            {
                return None;
            }
            let plan: Vec<_> = entries
                .iter()
                .map(|entry| {
                    let path = clean_path(&entry.path);
                    (!selected.iter().any(|target| is_within(&path, target)))
                        .then(|| entry.path.clone())
                })
                .collect();
            plan.iter().any(Option::is_none).then_some(ZipEditPlan {
                names: plan,
                replacement: None,
                level: 0,
            })
        }
        Edit::Rename { path, new_name } if valid_name(new_name) => {
            let target = clean_path(path);
            if target.is_empty() || safe_join(root, &target).is_err() {
                return None;
            }
            let parent = target.rsplit_once('/').map_or("", |(parent, _)| parent);
            let next = if parent.is_empty() {
                new_name.clone()
            } else {
                format!("{parent}/{new_name}")
            };
            let lower = next.to_lowercase();
            let child_prefix = format!("{lower}/");
            if paths.contains_key(&lower)
                || paths.keys().any(|path| path.starts_with(&child_prefix))
            {
                return None;
            }
            let mut changed = false;
            let plan = entries
                .iter()
                .map(|entry| {
                    let old = clean_path(&entry.path);
                    if is_within(&old, &target) {
                        changed = true;
                        let suffix = &old[target.len()..];
                        let trailing = if entry.path.ends_with('/') { "/" } else { "" };
                        Some(format!("{next}{suffix}{trailing}"))
                    } else {
                        Some(entry.path.clone())
                    }
                })
                .collect();
            changed.then_some(ZipEditPlan {
                names: plan,
                replacement: None,
                level: 0,
            })
        }
        Edit::Replace { entry, with } => {
            let target = clean_path(entry);
            if target.is_empty()
                || safe_join(root, &target).is_err()
                || safe_join(root, entry).is_err()
            {
                return None;
            }
            let source = fs::symlink_metadata(with).ok()?;
            if !source.is_file() || source.file_type().is_symlink() || source.len() > opts.max_total
            {
                return None;
            }
            let matched = entries
                .iter()
                .find(|entry| clean_path(&entry.path) == target && entry.kind == EntryKind::File)?;
            let plan = entries
                .iter()
                .map(|entry| Some(entry.path.clone()))
                .collect();
            Some(ZipEditPlan {
                names: plan,
                replacement: Some((matched.index, with.clone())),
                level: 0,
            })
        }
        _ => None,
    }
}

struct StagedFile(PathBuf);

impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct EditPhase<'a> {
    phase: u64,
    files_per_phase: u64,
    progress: &'a mut dyn ProgressSink,
}

impl ProgressSink for EditPhase<'_> {
    fn update(&mut self, progress: Progress) {
        let part = progress
            .bytes_done
            .min(progress.bytes_total)
            .saturating_mul(1_000_000)
            .checked_div(progress.bytes_total)
            .unwrap_or(0);
        self.progress.update(Progress {
            files_done: self.phase * self.files_per_phase
                + progress.files_done.min(self.files_per_phase),
            files_total: self.files_per_phase * 3,
            bytes_done: self.phase * 1_000_000 + part,
            bytes_total: 3_000_000,
        });
    }
}

fn edit_zip_copy(
    archive: &Path,
    entries: &[Entry],
    plan: ZipEditPlan,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<EditReport> {
    let parent = archive
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = archive.file_name().unwrap_or_default().to_string_lossy();
    let replacement = plan
        .replacement
        .map(|(index, path)| (index, path, plan.level));
    let plan = &plan.names;
    let original_size = entries
        .iter()
        .fold(0u64, |n, entry| n.saturating_add(entry.size));
    let mut kept_size = entries
        .iter()
        .zip(plan)
        .filter(|(_, name)| name.is_some())
        .fold(0u64, |n, (entry, _)| n.saturating_add(entry.size));
    if let Some((index, path, _)) = &replacement {
        let old = entries
            .get(*index)
            .ok_or_else(|| Error::Corrupt("invalid ZIP replacement index".into()))?;
        kept_size = kept_size
            .saturating_sub(old.size)
            .saturating_add(fs::metadata(path)?.len());
    }
    let source = File::open(archive)?;
    let original_metadata = source.metadata()?;
    verify_progress(
        archive,
        None,
        cancel,
        original_size,
        entries.len() as u64,
        &mut EditPhase {
            phase: 0,
            files_per_phase: entries.len() as u64,
            progress,
        },
    )?;
    let staged_path = parent.join(format!(
        ".{name}.rz-new-{}-{}",
        std::process::id(),
        unique_seq()
    ));
    let output = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&staged_path)?;
    let staged = StagedFile(staged_path);
    rz_formats::zip_edit::copy_entries(
        source.try_clone()?,
        output,
        plan,
        replacement,
        cancel,
        &mut EditPhase {
            phase: 1,
            files_per_phase: entries.len() as u64,
            progress,
        },
    )?;
    let report = verify_progress(
        &staged.0,
        None,
        cancel,
        kept_size,
        plan.iter().filter(|name| name.is_some()).count() as u64,
        &mut EditPhase {
            phase: 2,
            files_per_phase: entries.len() as u64,
            progress,
        },
    )?;
    cancel.check()?;
    let current = fs::metadata(archive)?;
    let held = source.metadata()?;
    #[cfg(unix)]
    let same_identity = {
        use std::os::unix::fs::MetadataExt;
        current.dev() == held.dev() && current.ino() == held.ino()
    };
    #[cfg(not(unix))]
    let same_identity = true;
    if !same_identity
        || current.len() != original_metadata.len()
        || current.modified().ok() != original_metadata.modified().ok()
        || held.len() != original_metadata.len()
        || held.modified().ok() != original_metadata.modified().ok()
    {
        return Err(Error::Io(std::io::Error::other(
            "archive changed during editing",
        )));
    }
    drop(source);
    fs::rename(&staged.0, archive)?;
    progress.update(Progress {
        files_done: entries.len() as u64 * 3,
        files_total: entries.len() as u64 * 3,
        bytes_done: 3_000_000,
        bytes_total: 3_000_000,
    });
    Ok(report)
}

/// Reads the new archive from end to end, so a damaged result never replaces a good one.
fn verify(path: &Path, password: Option<&str>, cancel: &CancelToken) -> Result<EditReport> {
    verify_progress(path, password, cancel, 0, 0, &mut NoProgress)
}

fn verify_progress(
    path: &Path,
    password: Option<&str>,
    cancel: &CancelToken,
    bytes_total: u64,
    files_total: u64,
    progress: &mut dyn ProgressSink,
) -> Result<EditReport> {
    let mut reader = rz_formats::open(path)?;
    if let Some(pw) = password {
        reader.set_password(pw)?;
    }
    let mut report = EditReport::default();
    let mut buf = vec![0u8; 256 * 1024];
    let mut last = None::<std::time::Instant>;
    reader.for_each(&mut |e, data| {
        cancel.check()?;
        if e.kind == EntryKind::File {
            let mut seen = 0u64;
            loop {
                cancel.check()?;
                let n = data
                    .read(&mut buf)
                    .map_err(|err| Error::Corrupt(format!("{}: {err}", e.path)))?;
                if n == 0 {
                    break;
                }
                seen += n as u64;
                if last.is_none_or(|time| time.elapsed() >= Duration::from_millis(50)) {
                    progress.update(Progress {
                        files_done: report.files,
                        files_total,
                        bytes_done: report.bytes.saturating_add(seen),
                        bytes_total,
                    });
                    last = Some(std::time::Instant::now());
                }
            }
            report.bytes += seen;
        }
        report.files += 1;
        if last.is_none_or(|time| time.elapsed() >= Duration::from_millis(50)) {
            progress.update(Progress {
                files_done: report.files,
                files_total,
                bytes_done: report.bytes,
                bytes_total,
            });
            last = Some(std::time::Instant::now());
        }
        Ok(())
    })?;
    progress.update(Progress {
        files_done: report.files,
        files_total,
        bytes_done: report.bytes,
        bytes_total,
    });
    cancel.check()?;
    Ok(report)
}

/// Copies a file or folder, keeping modification times. Symlinks are left out,
/// the same as when creating an archive.
fn copy_tree(from: &Path, to: &Path, cancel: &CancelToken) -> Result<()> {
    cancel.check()?;
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()), cancel)?;
        }
    } else if meta.is_file() {
        fs::copy(from, to)?;
        if let Some(t) = rz_platform::mtime_of(&meta) {
            let _ = rz_platform::set_mtime(to, t);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_core::NoProgress;

    struct EditGate {
        dir: PathBuf,
        role: String,
        stopped: bool,
    }

    impl ProgressSink for EditGate {
        fn update(&mut self, progress: Progress) {
            if !self.stopped
                && progress.files_total > 0
                && progress.files_done >= progress.files_total / 2
            {
                self.stopped = true;
                fs::write(self.dir.join(format!("gate-{}", self.role)), b"staged").unwrap();
                let release = self.dir.join(format!("release-{}", self.role));
                let deadline = std::time::Instant::now() + Duration::from_secs(20);
                while !release.exists() {
                    assert!(std::time::Instant::now() < deadline, "edit gate timed out");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    fn wait_for_file(path: &Path, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        path.exists()
    }

    fn kill_children_on_timeout(children: &mut [std::process::Child], phase: &str) -> ! {
        for child in children {
            let _ = child.kill();
            let _ = child.wait();
        }
        panic!("timed out waiting for cross-process edit {phase}");
    }

    #[test]
    fn concurrent_edits_in_separate_processes_merge_instead_of_losing_one_update() {
        if let Ok(role) = std::env::var("RZ_TEST_EDIT_ROLE") {
            let dir = PathBuf::from(std::env::var("RZ_TEST_EDIT_DIR").unwrap());
            let archive = dir.join("shared.zip");
            let edit = match role.as_str() {
                "a" => Edit::Delete(vec!["data/a.txt".into()]),
                "b" => Edit::Delete(vec!["data/sub/b.txt".into()]),
                _ => panic!("unknown cross-process test role"),
            };
            let mut progress = EditGate {
                dir: dir.clone(),
                role: role.clone(),
                stopped: false,
            };
            edit_archive(
                &archive,
                &edit,
                &opts(None),
                6,
                None,
                &CancelToken::new(),
                &mut progress,
            )
            .unwrap();
            fs::write(dir.join(format!("done-{role}")), b"done").unwrap();
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let archive = make(dir.path(), "shared.zip", None);
        let exe = std::env::current_exe().unwrap();
        let spawn = |role: &str| {
            std::process::Command::new(&exe)
            .arg("--exact")
            .arg("modify::tests::concurrent_edits_in_separate_processes_merge_instead_of_losing_one_update")
            .arg("--nocapture")
            .env("RZ_TEST_EDIT_ROLE", role)
            .env("RZ_TEST_EDIT_DIR", dir.path())
            .env("RZ_TEST_EDIT_LOCK_ARCHIVE", &archive)
            .env("RZ_TEST_EDIT_LOCK_WAIT_MARKER", dir.path().join(format!("wait-{role}")))
            .spawn().unwrap()
        };
        let mut children = vec![spawn("a"), spawn("b")];
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let holder = loop {
            if dir.path().join("gate-a").exists() {
                break "a";
            }
            if dir.path().join("gate-b").exists() {
                break "b";
            }
            if std::time::Instant::now() >= deadline {
                kill_children_on_timeout(&mut children, "first edit staging");
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let waiter = if holder == "a" { "b" } else { "a" };
        if !wait_for_file(
            &dir.path().join(format!("wait-{waiter}")),
            Duration::from_secs(20),
        ) {
            kill_children_on_timeout(&mut children, "second process to wait on the archive lock");
        }
        assert!(
            !dir.path().join(format!("gate-{waiter}")).exists(),
            "both processes staged the same original archive"
        );
        fs::write(dir.path().join(format!("release-{holder}")), b"go").unwrap();
        if !wait_for_file(
            &dir.path().join(format!("done-{holder}")),
            Duration::from_secs(20),
        ) {
            kill_children_on_timeout(&mut children, "first edit publish");
        }
        if !wait_for_file(
            &dir.path().join(format!("gate-{waiter}")),
            Duration::from_secs(20),
        ) {
            kill_children_on_timeout(&mut children, "second edit staging the latest archive");
        }
        fs::write(dir.path().join(format!("release-{waiter}")), b"go").unwrap();
        for index in 0..children.len() {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            loop {
                if let Some(status) = children[index].try_wait().unwrap() {
                    assert!(status.success(), "cross-process edit failed: {status}");
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    kill_children_on_timeout(&mut children, "child process exit");
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(names(&archive, None), ["data/sub/c.txt"]);
        assert!(fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .all(|e| !e.file_name().to_string_lossy().contains(".rz-")));
    }

    #[test]
    fn waiting_for_an_archive_lock_is_cancellable() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        fs::write(&archive, b"archive").unwrap();
        let held = open_archive_lock(&archive, dir.path()).unwrap();
        held.try_lock().unwrap();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_archive = archive.clone();
        let root = dir.path().to_path_buf();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(());
            archive_lock_at(&worker_archive, &root, &worker_cancel, || {}).map(|_| ())
        });
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.cancel();
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
        drop(held);
    }

    fn make(dir: &Path, name: &str, password: Option<&str>) -> PathBuf {
        let src = dir.join("data");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "alpha").unwrap();
        fs::write(src.join("sub/b.txt"), "beta").unwrap();
        fs::write(src.join("sub/c.txt"), "gamma").unwrap();
        let out = dir.join(name);
        compress(
            &[src],
            &out,
            OutputFormat::from_path(&out).unwrap(),
            &CreateOptions {
                password: password.map(str::to_string),
                ..CreateOptions::default()
            },
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        out
    }

    fn names(path: &Path, password: Option<&str>) -> Vec<String> {
        let mut r = rz_formats::open(path).unwrap();
        if let Some(pw) = password {
            r.set_password(pw).unwrap();
        }
        let mut v: Vec<String> = r
            .entries()
            .unwrap()
            .iter()
            .filter(|e| e.kind == EntryKind::File)
            .map(|e| clean_path(&e.path))
            .collect();
        v.sort();
        v
    }

    fn opts(password: Option<&str>) -> ExtractOptions {
        ExtractOptions {
            password: password.map(str::to_string),
            ..ExtractOptions::default()
        }
    }

    fn run(archive: &Path, edit: &Edit, password: Option<&str>) -> Result<EditReport> {
        edit_archive(
            archive,
            edit,
            &opts(password),
            6,
            None,
            &CancelToken::new(),
            &mut NoProgress,
        )
    }

    #[test]
    fn deleting_a_file_and_a_folder_works_in_every_editable_format() {
        for name in ["t.zip", "t.7z", "t.tar", "t.tar.gz"] {
            let tmp = tempfile::tempdir().unwrap();
            let archive = make(tmp.path(), name, None);
            run(&archive, &Edit::Delete(vec!["data/a.txt".into()]), None).unwrap();
            assert_eq!(
                names(&archive, None),
                ["data/sub/b.txt", "data/sub/c.txt"],
                "{name}"
            );
            let rep = run(&archive, &Edit::Delete(vec!["data/sub".into()]), None).unwrap();
            assert_eq!(names(&archive, None), Vec::<String>::new(), "{name}");
            assert_eq!(rep.bytes, 0);
            // No scratch files are left behind.
            let left: Vec<_> = fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with('.'))
                .collect();
            assert!(left.is_empty(), "{name}: {left:?}");
        }
    }

    #[test]
    fn deleting_one_zip_file_preserves_untouched_compressed_entries() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("mixed.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer
            .start_file(
                "keep.txt",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"keep original encoding").unwrap();
        writer
            .start_file("delete.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"remove me").unwrap();
        writer.finish().unwrap();

        run(&archive, &Edit::Delete(vec!["delete.txt".into()]), None).unwrap();
        let mut result = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        let entry = result.by_name("keep.txt").unwrap();
        assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
    }

    #[test]
    fn zip_copy_edit_preserves_archive_and_entry_comments_and_extra_fields() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("metadata.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer.set_comment("archive comment").unwrap();
        writer.set_raw_zip64_extensible_data_sector(Box::from([0x52, 0x5a, 0x31, 0x00]));
        let mut options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .into_full_options()
            .with_file_comment("entry comment");
        options.add_extra_field(0xcafe, b"rustzip", false).unwrap();
        writer.start_file("keep.txt", options).unwrap();
        writer.write_all(b"preserved metadata").unwrap();
        writer
            .start_file("delete.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"remove me").unwrap();
        writer.finish().unwrap();

        run(&archive, &Edit::Delete(vec!["delete.txt".into()]), None).unwrap();
        let expected_extra =
            Some([0xfe, 0xca, 7, 0, b'r', b'u', b's', b't', b'z', b'i', b'p'].to_vec());
        {
            let mut result = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
            assert_eq!(result.comment(), b"archive comment");
            assert_eq!(
                result.raw_zip64_extensible_data_sector(),
                Some(&[0x52, 0x5a, 0x31, 0x00][..])
            );
            let entry = result.by_name("keep.txt").unwrap();
            assert_eq!(entry.comment(), "entry comment");
            assert_eq!(entry.extra_data(), expected_extra);
        }
        run(
            &archive,
            &Edit::Rename {
                path: "keep.txt".into(),
                new_name: "renamed.txt".into(),
            },
            None,
        )
        .unwrap();
        let mut result = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        assert_eq!(result.comment(), b"archive comment");
        assert_eq!(
            result.raw_zip64_extensible_data_sector(),
            Some(&[0x52, 0x5a, 0x31, 0x00][..])
        );
        let entry = result.by_name("renamed.txt").unwrap();
        assert_eq!(entry.comment(), "entry comment");
        assert_eq!(entry.extra_data(), expected_extra);
    }

    #[test]
    fn renaming_one_zip_file_preserves_stored_data() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("rename.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer
            .start_file(
                "old.txt",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"preserved").unwrap();
        writer.finish().unwrap();

        run(
            &archive,
            &Edit::Rename {
                path: "old.txt".into(),
                new_name: "new.txt".into(),
            },
            None,
        )
        .unwrap();
        let mut result = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        let entry = result.by_name("new.txt").unwrap();
        assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
    }

    #[test]
    fn zip_copy_edits_implicit_folders_without_recompressing() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("folders.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        for name in ["folder/one.txt", "folder/sub/two.txt", "other.txt"] {
            writer
                .start_file(
                    name,
                    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(name.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        run(
            &archive,
            &Edit::Rename {
                path: "folder".into(),
                new_name: "moved".into(),
            },
            None,
        )
        .unwrap();
        {
            let mut result = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
            for name in ["moved/one.txt", "moved/sub/two.txt", "other.txt"] {
                assert_eq!(
                    result.by_name(name).unwrap().compression(),
                    zip::CompressionMethod::Stored
                );
            }
        }
        run(
            &archive,
            &Edit::Delete(vec!["moved/sub".into(), "other.txt".into()]),
            None,
        )
        .unwrap();
        assert_eq!(names(&archive, None), ["moved/one.txt"]);
        let mut result = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        assert_eq!(
            result.by_name("moved/one.txt").unwrap().compression(),
            zip::CompressionMethod::Stored
        );
    }

    #[test]
    fn zip_copy_keeps_utf8_names_and_directory_attributes() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("unicode.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer
            .add_directory(
                "资料/",
                SimpleFileOptions::default().unix_permissions(0o755),
            )
            .unwrap();
        writer
            .start_file(
                "资料/保留.txt",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all("内容".as_bytes()).unwrap();
        writer
            .start_file("删除.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"remove").unwrap();
        writer.finish().unwrap();

        run(&archive, &Edit::Delete(vec!["删除.txt".into()]), None).unwrap();
        let mut result = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        let dir = result.by_name("资料/").unwrap();
        assert_eq!(dir.unix_mode().unwrap() & 0o170000, 0o040000);
        drop(dir);
        assert_eq!(
            result.by_name("资料/保留.txt").unwrap().compression(),
            zip::CompressionMethod::Stored
        );
    }

    #[test]
    fn zip_replacement_only_recompresses_the_changed_file() {
        use std::io::{Read, Write};
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("replace.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        writer.set_comment("archive comment").unwrap();
        writer
            .start_file(
                "keep.txt",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"keep").unwrap();
        writer
            .start_file(
                "change.txt",
                SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
                    .into_full_options()
                    .with_file_comment("replacement comment"),
            )
            .unwrap();
        writer.write_all(b"original").unwrap();
        writer.finish().unwrap();
        let source = tmp.path().join("new.txt");
        fs::write(&source, b"new content").unwrap();
        run(
            &archive,
            &Edit::Replace {
                entry: "change.txt".into(),
                with: source,
            },
            None,
        )
        .unwrap();
        let mut result = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        assert_eq!(result.comment(), b"archive comment");
        assert_eq!(
            result.by_name("keep.txt").unwrap().compression(),
            zip::CompressionMethod::Stored
        );
        let mut changed = result.by_name("change.txt").unwrap();
        assert_eq!(changed.compression(), zip::CompressionMethod::Deflated);
        assert_eq!(changed.comment(), "replacement comment");
        let mut content = Vec::new();
        changed.read_to_end(&mut content).unwrap();
        assert_eq!(content, b"new content");
    }

    #[test]
    fn zip_copy_cancel_and_corruption_leave_original_unchanged() {
        use std::io::{Seek, SeekFrom, Write};
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("failure.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        for name in ["keep.txt", "delete.txt"] {
            writer
                .start_file(
                    name,
                    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(b"valid data").unwrap();
        }
        writer.finish().unwrap();
        let before = fs::read(&archive).unwrap();
        let token = CancelToken::new();
        let mut progress = |p: Progress| {
            if p.bytes_done > 1_000_000 && p.files_done * 3 == p.files_total && p.files_total > 0 {
                token.cancel();
            }
        };
        let result = edit_archive(
            &archive,
            &Edit::Delete(vec!["delete.txt".into()]),
            &opts(None),
            6,
            None,
            &token,
            &mut progress,
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(fs::read(&archive).unwrap(), before);
        assert!(!fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".rz-new-")));

        let mut reader = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        let offset = reader.by_name("delete.txt").unwrap().data_start().unwrap();
        drop(reader);
        let mut file = OpenOptions::new().write(true).open(&archive).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(b"X").unwrap();
        drop(file);
        let damaged = fs::read(&archive).unwrap();
        assert!(run(&archive, &Edit::Delete(vec!["delete.txt".into()]), None).is_err());
        assert_eq!(fs::read(&archive).unwrap(), damaged);
    }

    #[cfg(unix)]
    #[test]
    fn zip_copy_does_not_replace_a_changed_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "race.zip", None);
        let replacement = tmp.path().join("replacement.zip");
        fs::copy(&archive, &replacement).unwrap();
        let original = fs::read(&replacement).unwrap();
        let mut swapped = false;
        let mut progress = |p: Progress| {
            if !swapped && p.bytes_done >= 2_000_000 {
                fs::rename(&replacement, &archive).unwrap();
                swapped = true;
            }
        };
        let result = edit_archive(
            &archive,
            &Edit::Delete(vec!["data/a.txt".into()]),
            &opts(None),
            6,
            None,
            &CancelToken::new(),
            &mut progress,
        );
        assert!(swapped);
        assert!(result.is_err());
        assert_eq!(fs::read(&archive).unwrap(), original);
    }

    #[test]
    fn zip_copy_progress_is_monotonic_and_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "progress.zip", None);
        let mut events = Vec::new();
        edit_archive(
            &archive,
            &Edit::Delete(vec!["data/a.txt".into()]),
            &opts(None),
            6,
            None,
            &CancelToken::new(),
            &mut |progress| events.push(progress),
        )
        .unwrap();
        assert!(events.windows(2).all(|pair| {
            pair[0].bytes_done <= pair[1].bytes_done && pair[0].files_done <= pair[1].files_done
        }));
        let last = events.last().unwrap();
        assert_eq!(last.bytes_done, last.bytes_total);
        assert_eq!(last.files_done, last.files_total);
    }

    #[test]
    fn encrypted_zip_edits_keep_using_the_existing_safe_path() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "encrypted.zip", Some("pw"));
        run(
            &archive,
            &Edit::Delete(vec!["data/a.txt".into()]),
            Some("pw"),
        )
        .unwrap();
        assert_eq!(
            names(&archive, Some("pw")),
            ["data/sub/b.txt", "data/sub/c.txt"]
        );
        let mut reader = rz_formats::open(&archive).unwrap();
        assert!(reader
            .entries()
            .unwrap()
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .all(|entry| entry.encrypted));
    }

    #[test]
    fn adding_files_replaces_same_named_ones_and_keeps_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.zip", None);
        let extra = tmp.path().join("new");
        fs::create_dir_all(extra.join("deep")).unwrap();
        fs::write(extra.join("a.txt"), "replaced").unwrap();
        fs::write(extra.join("deep/z.txt"), "zed").unwrap();
        let pic = tmp.path().join("pic.png");
        fs::write(&pic, [1u8, 2, 3]).unwrap();

        let edit = Edit::Add {
            into: "data".into(),
            sources: vec![extra.join("a.txt"), extra.join("deep"), pic],
        };
        let rep = run(&archive, &edit, None).unwrap();
        assert_eq!(
            names(&archive, None),
            [
                "data/a.txt",
                "data/deep/z.txt",
                "data/pic.png",
                "data/sub/b.txt",
                "data/sub/c.txt"
            ]
        );
        // Folders may be counted as entries too, so only a lower bound is certain.
        assert!(rep.files >= 5);
        // The replaced file has the new content.
        let out = tmp.path().join("out");
        let mut r = rz_formats::open(&archive).unwrap();
        extract(
            &mut *r,
            &out,
            &ExtractOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(out.join("data/a.txt")).unwrap(),
            "replaced"
        );
    }

    #[test]
    fn an_encrypted_archive_stays_encrypted() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.7z", Some("pw"));
        run(
            &archive,
            &Edit::Delete(vec!["data/a.txt".into()]),
            Some("pw"),
        )
        .unwrap();
        assert_eq!(
            names(&archive, Some("pw")),
            ["data/sub/b.txt", "data/sub/c.txt"]
        );
        // Without the password the archive still refuses to open its content.
        assert!(run(&archive, &Edit::Delete(vec!["data/sub".into()]), None).is_err());
        assert_eq!(names(&archive, Some("pw")).len(), 2);
    }

    #[test]
    fn a_failed_or_cancelled_edit_leaves_the_original_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.zip", None);
        let before = fs::read(&archive).unwrap();

        // Cancelled before it starts.
        let token = CancelToken::new();
        token.cancel();
        let r = edit_archive(
            &archive,
            &Edit::Delete(vec!["data/a.txt".into()]),
            &opts(None),
            6,
            None,
            &token,
            &mut NoProgress,
        );
        assert!(matches!(r, Err(Error::Cancelled)));
        assert_eq!(fs::read(&archive).unwrap(), before);

        // A path that tries to climb out of the archive is refused.
        let r = run(&archive, &Edit::Delete(vec!["../escape".into()]), None);
        assert!(matches!(r, Err(Error::UnsafePath(_))));
        assert_eq!(fs::read(&archive).unwrap(), before);
    }

    #[test]
    fn renaming_moves_an_entry_and_refuses_clashes_and_bad_names() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.7z", None);
        let rename = |path: &str, to: &str| {
            run(
                &archive,
                &Edit::Rename {
                    path: path.into(),
                    new_name: to.into(),
                },
                None,
            )
        };
        rename("data/a.txt", "first.txt").unwrap();
        // A folder is renamed together with what is inside it.
        rename("data/sub", "more").unwrap();
        assert_eq!(
            names(&archive, None),
            ["data/first.txt", "data/more/b.txt", "data/more/c.txt"]
        );
        let before = fs::read(&archive).unwrap();
        assert!(matches!(
            rename("data/first.txt", "more"),
            Err(Error::Io(_))
        ));
        assert!(matches!(
            rename("data/first.txt", "../x"),
            Err(Error::UnsafePath(_))
        ));
        assert!(matches!(
            rename("data/first.txt", "a/b"),
            Err(Error::UnsafePath(_))
        ));
        assert!(rename("data/none.txt", "x").is_err());
        assert_eq!(fs::read(&archive).unwrap(), before);
        assert!(valid_name("报告 终稿.pdf") && !valid_name(" x") && !valid_name(".."));
    }

    #[test]
    fn replacing_a_file_swaps_its_content_and_keeps_everything_else() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.zip", None);
        let edited = tmp.path().join("whatever-name.txt");
        fs::write(&edited, "edited!").unwrap();
        run(
            &archive,
            &Edit::Replace {
                entry: "data/sub/b.txt".into(),
                with: edited.clone(),
            },
            None,
        )
        .unwrap();
        assert_eq!(names(&archive, None).len(), 3);
        let out = tmp.path().join("out");
        let mut r = rz_formats::open(&archive).unwrap();
        extract(
            &mut *r,
            &out,
            &ExtractOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(out.join("data/sub/b.txt")).unwrap(),
            "edited!"
        );
        assert_eq!(fs::read_to_string(out.join("data/a.txt")).unwrap(), "alpha");
        // An entry that is no longer there is not silently recreated.
        let r = run(
            &archive,
            &Edit::Replace {
                entry: "data/gone.txt".into(),
                with: edited,
            },
            None,
        );
        assert!(r.is_err());
    }

    #[test]
    fn single_file_and_unknown_formats_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let one = tmp.path().join("x.txt");
        fs::write(&one, "hello hello").unwrap();
        let p = tmp.path().join("x.txt.gz");
        compress(
            &[one],
            &p,
            OutputFormat::from_path(&p).unwrap(),
            &CreateOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert!(edit_format("gzip").is_none());
        assert!(edit_format("rar").is_none());
        assert!(edit_format("zip").is_some() && edit_format("tar.zst").is_some());
        assert!(matches!(
            run(&p, &Edit::Delete(vec![]), None),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn what_the_file_is_decides_not_what_it_is_called() {
        // A zip under another name (a .jar, a .docx...) can be changed like any zip.
        let tmp = tempfile::tempdir().unwrap();
        let zip = make(tmp.path(), "t.zip", None);
        let odd = tmp.path().join("library.jar");
        fs::rename(&zip, &odd).unwrap();
        run(&odd, &Edit::Delete(vec!["data/a.txt".into()]), None).unwrap();
        assert_eq!(names(&odd, None), ["data/sub/b.txt", "data/sub/c.txt"]);
        assert!(odd.is_file());
    }
}
