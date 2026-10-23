use crate::safe_path::safe_join;
use rz_core::{ArchiveReader, CancelToken, EntryKind, Error, Progress, ProgressSink, Result};
use rz_platform::{apply_mode, set_mtime};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overwrite {
    /// Fail if the target already exists.
    Error,
    Skip,
    Replace,
}

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub overwrite: Overwrite,
    /// Largest allowed ratio of unpacked to packed bytes for any entry.
    pub max_ratio: u64,
    /// Largest allowed total unpacked size.
    pub max_total: u64,
    pub password: Option<String>,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            overwrite: Overwrite::Error,
            max_ratio: 2000,
            max_total: 1 << 40,
            password: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct ExtractReport {
    pub files: u64,
    pub dirs: u64,
    pub skipped: u64,
    pub bytes: u64,
}

const BUF: usize = 256 * 1024;

pub fn extract(
    reader: &mut dyn ArchiveReader,
    dest: &Path,
    opts: &ExtractOptions,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<ExtractReport> {
    if let Some(pw) = &opts.password {
        reader.set_password(pw)?;
    }
    let entries = reader.entries()?;
    let declared: u64 = entries.iter().map(|e| e.size).fold(0, u64::saturating_add);
    if declared > opts.max_total {
        return Err(Error::LimitExceeded(format!("total size {declared} bytes")));
    }
    fs::create_dir_all(dest)?;

    let mut report = ExtractReport::default();
    let mut p = Progress {
        files_total: entries.len() as u64,
        bytes_total: declared,
        ..Progress::default()
    };
    let mut buf = vec![0u8; BUF];
    // Directory metadata is applied last, so restricted modes do not block writing children.
    let mut dir_meta = Vec::new();

    for entry in &entries {
        cancel.check()?;
        let target = safe_join(dest, &entry.path)?;
        match entry.kind {
            EntryKind::Dir => {
                fs::create_dir_all(&target)?;
                dir_meta.push((target, entry.unix_mode, entry.modified));
                report.dirs += 1;
            }
            EntryKind::Symlink => {
                // Symlinks can point outside the destination; skipped until link policy exists.
                report.skipped += 1;
            }
            EntryKind::File => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                if target.exists() {
                    match opts.overwrite {
                        Overwrite::Error => {
                            return Err(Error::Io(std::io::Error::new(
                                std::io::ErrorKind::AlreadyExists,
                                target.display().to_string(),
                            )))
                        }
                        Overwrite::Skip => {
                            report.skipped += 1;
                            continue;
                        }
                        Overwrite::Replace => {}
                    }
                }
                // Bound the output by the declared size and the compression ratio, so a
                // lying header cannot turn into an unbounded write.
                let limit = match entry.packed_size {
                    Some(packed) => entry.size.min(packed.max(1).saturating_mul(opts.max_ratio)),
                    None => entry.size,
                };
                if entry.size > limit {
                    return Err(Error::LimitExceeded(format!(
                        "{}: ratio too high",
                        entry.path
                    )));
                }
                let mut src = reader.open(entry)?;
                let mut out = open_new(&target)?;
                let mut written = 0u64;
                loop {
                    cancel.check()?;
                    let n = src.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    written += n as u64;
                    if written > limit {
                        return Err(Error::LimitExceeded(format!(
                            "{}: exceeds declared size",
                            entry.path
                        )));
                    }
                    out.write_all(&buf[..n])?;
                    p.bytes_done += n as u64;
                    report.bytes += n as u64;
                }
                drop(out);
                if let Some(mode) = entry.unix_mode {
                    apply_mode(&target, mode)?;
                }
                if let Some(t) = entry.modified {
                    set_mtime(&target, t)?;
                }
                report.files += 1;
            }
        }
        p.files_done += 1;
        progress.update(p);
    }
    // Deepest directories first, so setting a parent's mtime is not undone by its children.
    for (dir, mode, modified) in dir_meta.into_iter().rev() {
        if let Some(mode) = mode {
            apply_mode(&dir, mode | 0o700)?;
        }
        if let Some(t) = modified {
            // Directory times are best effort; some platforms cannot open a directory for writing.
            let _ = set_mtime(&dir, t);
        }
    }
    Ok(report)
}

fn open_new(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?)
}
