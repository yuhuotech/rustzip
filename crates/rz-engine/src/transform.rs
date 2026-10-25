//! Streaming conversion and conservative recovery, published only after verification.

use crate::progress::Tracker;
use crate::{safe_join, ExtractOptions};
use rz_core::{
    clean_path, CancelToken, CreateOptions, Entry, EntryKind, Error, NewEntry, ProgressSink, Result,
};
use rz_formats::OutputFormat;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// A bounded report: counts include all issues, detail retains the first thousand.
#[derive(Debug, Clone, Default)]
pub struct TransformReport {
    pub files: u64,
    pub bytes: u64,
    pub skipped: u64,
    pub issues: Vec<String>,
}

impl TransformReport {
    fn issue(&mut self, text: String) {
        if self.issues.len() < 1000 {
            self.issues.push(text);
        }
    }
}

/// Conversion and recovery options; input and output passwords are independent.
#[derive(Clone)]
pub struct TransformOptions {
    pub format: OutputFormat,
    pub create: CreateOptions,
    pub limits: ExtractOptions,
    pub encoding: Option<String>,
}

impl std::fmt::Debug for TransformOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformOptions")
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl Drop for TransformOptions {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(pw) = &mut self.create.password {
            pw.zeroize();
        }
        if let Some(pw) = &mut self.limits.password {
            pw.zeroize();
        }
    }
}

/// Converts without plaintext staging; recovery stages one entry at a time so a
/// failed checksum never contaminates the new archive. Neither mode overwrites files.
pub fn transform(
    source: &Path,
    output: &Path,
    opts: &TransformOptions,
    recover: bool,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<TransformReport> {
    let result = transform_inner(source, output, opts, recover, cancel, progress);
    if result.is_err() {
        cancel.check()?;
    }
    result
}

fn transform_inner(
    source: &Path,
    output: &Path,
    opts: &TransformOptions,
    recover: bool,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<TransformReport> {
    cancel.check()?;
    if source == output || output.symlink_metadata().is_ok() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "choose a new output file; existing files are never overwritten",
        )));
    }
    if matches!(opts.format, OutputFormat::Single(_)) {
        return Err(Error::Unsupported("choose zip, 7z or a tar archive".into()));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // A recognizable name, so a leftover from a crashed run can be told apart
    // from the user's files; the random part keeps concurrent runs apart.
    let temp_prefix = format!(
        ".{}.rz-",
        output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let temporary = tempfile::Builder::new()
        .prefix(&temp_prefix)
        .suffix(".part")
        .tempfile_in(parent)?;
    let zip_recovery = if recover {
        let mut file = File::open(source)?;
        match rz_formats::sniff(&mut file)? {
            rz_formats::Format::Zip => true,
            rz_formats::Format::SevenZip => false,
            _ => return Err(Error::Unsupported("repair supports ZIP and 7z only".into())),
        }
    } else {
        false
    };
    let mut reader = if zip_recovery {
        None
    } else {
        Some(rz_formats::open(source)?)
    };
    let entries = if zip_recovery {
        Vec::new()
    } else {
        let r = reader.as_mut().ok_or(Error::UnknownFormat)?;
        r.set_read_control(cancel.clone(), opts.limits.max_total);
        if let Some(enc) = &opts.encoding {
            r.set_encoding(enc)?;
        }
        if let Some(pw) = &opts.limits.password {
            r.set_password(pw)?;
        }
        if r.caps().random_access || r.format_name() == "7z" {
            r.entries()?
        } else {
            Vec::new()
        }
    };
    let total = entries.iter().fold(0u64, |n, e| n.saturating_add(e.size));
    if total > opts.limits.max_total {
        return Err(Error::LimitExceeded("total unpacked size".into()));
    }
    let tracker = Tracker::new(
        progress,
        cancel,
        entries.len() as u64 * 2,
        total.saturating_mul(2),
    );
    let mut writer = rz_formats::create(temporary.path(), opts.format, &opts.create)?;
    let report = RefCell::new(TransformReport::default());
    let mut seen: HashMap<String, EntryKind> = HashMap::new();
    let mut explicit = HashSet::new();
    let mut metadata_bytes = 0u64;
    let mut entry_count = 0u64;
    let mut processed = 0u64;
    let mut visit = |entry: &Entry, data: &mut dyn Read| -> Result<()> {
        cancel.check()?;
        let name = clean_path(&entry.path);
        entry_count += 1;
        metadata_bytes = metadata_bytes.saturating_add(name.len() as u64 * 4 + 256);
        if entry_count > 1_000_000 || metadata_bytes > opts.create.memory_budget {
            return Err(Error::LimitExceeded("entry metadata memory budget".into()));
        }
        let check = (|| {
            safe_join(Path::new("."), &entry.path)?;
            if name.split('/').count() > 128 {
                return Err(Error::LimitExceeded("path depth".into()));
            }
            if entry.kind == EntryKind::Symlink {
                return Err(Error::Unsupported(
                    "symbolic links cannot be preserved".into(),
                ));
            }
            if entry.kind == EntryKind::Dir && entry.size != 0 {
                return Err(Error::Corrupt("directory entry contains file data".into()));
            }
            let key = name.to_lowercase();
            if explicit.contains(&key) || (entry.kind != EntryKind::Dir && seen.contains_key(&key))
            {
                return Err(Error::Corrupt("duplicate or conflicting file name".into()));
            }
            let mut ancestor = key.as_str();
            while let Some((parent, _)) = ancestor.rsplit_once('/') {
                if seen.get(parent).is_some_and(|k| *k != EntryKind::Dir) {
                    return Err(Error::Corrupt("file used as a parent directory".into()));
                }
                ancestor = parent;
            }
            if entry.size > opts.limits.ratio_check_above
                && entry
                    .packed_size
                    .is_some_and(|n| entry.size / n.max(1) > opts.limits.max_ratio)
            {
                return Err(Error::LimitExceeded("compression ratio".into()));
            }
            Ok(())
        })();
        if let Err(e) = check {
            if !recover || matches!(e, Error::LimitExceeded(_)) {
                return Err(e);
            }
            processed = processed.saturating_add(entry.size);
            if processed > opts.limits.max_total {
                return Err(Error::LimitExceeded("total recovery size".into()));
            }
            // Sequential decoders must reach the next entry even when this name is unsafe.
            let mut checked = Checked {
                data,
                remaining: entry.size,
                tracker: &tracker,
                cancel,
            };
            let _ = std::io::copy(&mut checked, &mut std::io::sink());
            cancel.check()?;
            report.borrow_mut().skipped += 1;
            report.borrow_mut().issue(format!("{}: {e}", entry.path));
            return Ok(());
        }
        processed = processed.saturating_add(entry.size);
        if processed > opts.limits.max_total {
            return Err(Error::LimitExceeded("total recovery size".into()));
        }
        let meta = NewEntry {
            name: name.clone(),
            size: entry.size,
            unix_mode: entry.unix_mode,
            modified: entry.modified,
        };
        if entry.kind == EntryKind::Dir {
            writer.add_dir(&meta)?;
        } else {
            let mut controlled = Checked {
                data,
                remaining: entry.size,
                tracker: &tracker,
                cancel,
            };
            if recover {
                let mut spool = tempfile::tempfile()?;
                let mut copied = Ok(0u64);
                let mut buffer = vec![0u8; 256 * 1024];
                loop {
                    match controlled.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            // A full disk is an operation failure, never a damaged input entry.
                            spool.write_all(&buffer[..n])?;
                            if let Ok(total) = &mut copied {
                                *total += n as u64;
                            }
                        }
                        Err(e) => {
                            copied = Err(e);
                            break;
                        }
                    }
                }
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                match copied {
                    Ok(n) if n == entry.size => {}
                    Ok(_) => {
                        report.borrow_mut().skipped += 1;
                        report
                            .borrow_mut()
                            .issue(format!("{}: truncated data", entry.path));
                        return Ok(());
                    }
                    Err(e) => {
                        report.borrow_mut().skipped += 1;
                        report.borrow_mut().issue(format!("{}: {e}", entry.path));
                        return Ok(());
                    }
                }
                spool.seek(SeekFrom::Start(0))?;
                writer.add_file(&meta, &mut spool)?;
            } else {
                writer.add_file(&meta, &mut controlled)?;
                // Some writers stop at the declared size; read once more to validate EOF/CRC.
                let mut extra = [0];
                if controlled.remaining != 0 || controlled.read(&mut extra)? != 0 {
                    return Err(Error::Corrupt(
                        "entry size does not match its header".into(),
                    ));
                }
            }
            let mut rep = report.borrow_mut();
            rep.bytes = rep.bytes.saturating_add(entry.size);
        }
        seen.insert(name.to_lowercase(), entry.kind);
        explicit.insert(name.to_lowercase());
        let mut ancestor = name.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            seen.entry(parent.to_lowercase()).or_insert(EntryKind::Dir);
            ancestor = parent;
        }
        report.borrow_mut().files += 1;
        tracker.file_done();
        Ok(())
    };
    if zip_recovery {
        rz_formats::recovery::zip_entries(
            source,
            opts.limits.password.as_deref(),
            opts.encoding.as_deref(),
            cancel,
            &mut visit,
            &mut |text, skipped| {
                let mut r = report.borrow_mut();
                r.skipped += u64::from(skipped);
                r.issue(text);
            },
        )?;
    } else if recover {
        rz_formats::recovery::sevenz_entries(
            source,
            opts.limits.password.as_deref(),
            opts.limits.threads,
            cancel,
            &mut visit,
            &mut |text, skipped| {
                let mut r = report.borrow_mut();
                r.skipped += u64::from(skipped);
                r.issue(text);
            },
        )?;
    } else {
        let mut r = reader.ok_or(Error::UnknownFormat)?;
        if let Err(e) = r.for_each(&mut visit) {
            cancel.check()?;
            return Err(e);
        }
    }
    cancel.check()?;
    if recover && report.borrow().files == 0 {
        return Err(Error::Corrupt(
            "no entries recovered; check the password or try another copy of the source".into(),
        ));
    }
    writer.finish()?;
    cancel.check()?;
    let mut verified = rz_formats::open(temporary.path())?;
    if let Some(pw) = &opts.create.password {
        verified.set_password(pw)?;
    }
    verified.for_each(&mut |entry, data| {
        let mut checked = Checked {
            data,
            remaining: entry.size,
            tracker: &tracker,
            cancel,
        };
        let n = std::io::copy(&mut checked, &mut std::io::sink())?;
        if n != entry.size {
            return Err(Error::Corrupt("output verification failed".into()));
        }
        tracker.file_done();
        Ok(())
    })?;
    drop(verified);
    cancel.check()?;
    temporary
        .persist_noclobber(output)
        .map_err(|e| Error::Io(e.error))?;
    tracker.finish_with_totals(
        report.borrow().files.saturating_mul(2),
        report.borrow().bytes.saturating_mul(2),
    );
    Ok(report.into_inner())
}

struct Checked<'a, 'b> {
    data: &'a mut dyn Read,
    remaining: u64,
    tracker: &'a Tracker<'b>,
    cancel: &'a CancelToken,
}
impl Read for Checked<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cancel.check().map_err(std::io::Error::other)?;
        let cap = buf
            .len()
            .min(self.remaining.saturating_add(1).min(256 * 1024) as usize);
        let n = self.data.read(&mut buf[..cap])?;
        if n as u64 > self.remaining {
            return Err(std::io::Error::other("data exceeds declared size"));
        }
        self.remaining -= n as u64;
        self.tracker.add_bytes(n as u64);
        Ok(n)
    }
}
