use crate::progress::Tracker;
use rz_core::{
    ArchiveWriter, CancelToken, CreateOptions, Error, FileSource, NewEntry, ProgressSink, Result,
};
use rz_formats::OutputFormat;
use rz_platform::{mode_of, mtime_of, same_file};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct CompressReport {
    pub files: u64,
    pub dirs: u64,
    /// Symlinks and other non-regular files that were left out.
    pub skipped: u64,
    pub bytes: u64,
}

struct Item {
    abs: PathBuf,
    meta: NewEntry,
    is_dir: bool,
}

/// Compresses `sources` into `output`.
///
/// The archive is written to a temporary sibling file and renamed on success,
/// so a failed or cancelled run never leaves a half-written archive behind.
pub fn compress(
    sources: &[PathBuf],
    output: &Path,
    format: OutputFormat,
    opts: &CreateOptions,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<CompressReport> {
    let mut report = CompressReport::default();
    let mut items = Vec::new();
    for src in sources {
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::UnsafePath(src.display().to_string()))?
            .to_string();
        walk(src, &name, output, &mut items, &mut report, cancel)?;
    }

    let tmp = temp_path(output);
    let result = write_all(&items, &tmp, format, opts, cancel, progress, &mut report);
    match result {
        Ok(()) => {
            fs::rename(&tmp, output)?;
            Ok(report)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn temp_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(".rz-part");
    output.with_file_name(name)
}

fn walk(
    path: &Path,
    name: &str,
    output: &Path,
    items: &mut Vec<Item>,
    report: &mut CompressReport,
    cancel: &CancelToken,
) -> Result<()> {
    cancel.check()?;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        report.skipped += 1;
        return Ok(());
    }
    // Never put the archive being written into itself.
    if meta.is_file() && (same_file(path, output) || same_file(path, &temp_path(output))) {
        report.skipped += 1;
        return Ok(());
    }
    let new = NewEntry {
        name: name.to_string(),
        size: if meta.is_file() { meta.len() } else { 0 },
        unix_mode: mode_of(&meta),
        modified: mtime_of(&meta),
    };
    if meta.is_dir() {
        items.push(Item {
            abs: path.to_path_buf(),
            meta: new,
            is_dir: true,
        });
        let mut children: Vec<_> = fs::read_dir(path)?.collect::<std::io::Result<_>>()?;
        children.sort_by_key(|c| c.file_name());
        for child in children {
            let child_name = child.file_name().to_string_lossy().into_owned();
            walk(
                &child.path(),
                &format!("{name}/{child_name}"),
                output,
                items,
                report,
                cancel,
            )?;
        }
    } else if meta.is_file() {
        items.push(Item {
            abs: path.to_path_buf(),
            meta: new,
            is_dir: false,
        });
    } else {
        report.skipped += 1;
    }
    Ok(())
}

/// A source file that is opened on first read and closed at its end, so a
/// backend holding thousands of pending sources does not run out of descriptors.
struct SourceFile<'a> {
    path: &'a Path,
    file: Option<File>,
    done: bool,
    tracker: &'a Tracker<'a>,
}

impl Read for SourceFile<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        // Blocks while the task is paused; fails once it is cancelled.
        if self.tracker.check().is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        if self.file.is_none() {
            self.file = Some(File::open(self.path)?);
        }
        let n = match self.file.as_mut() {
            Some(f) => f.read(buf)?,
            None => 0,
        };
        if n == 0 {
            self.file = None;
            self.done = true;
            self.tracker.file_done();
        } else {
            self.tracker.add_bytes(n as u64);
        }
        Ok(n)
    }
}

fn write_all(
    items: &[Item],
    tmp: &Path,
    format: OutputFormat,
    opts: &CreateOptions,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
    report: &mut CompressReport,
) -> Result<()> {
    let bytes_total = items.iter().map(|i| i.meta.size).sum();
    let tracker = Tracker::new(progress, cancel, items.len() as u64, bytes_total);
    let mut writer: Box<dyn ArchiveWriter> = rz_formats::create(tmp, format, opts)?;

    // Directories first: extractors then create parents before their contents.
    for item in items.iter().filter(|i| i.is_dir) {
        cancel.check()?;
        writer.add_dir(&item.meta)?;
        report.dirs += 1;
        tracker.file_done();
    }

    let tracker_ref = &tracker;
    let mut files = items.iter().filter(|i| !i.is_dir).map(|item| {
        let src: Box<dyn Read + Send + '_> = Box::new(SourceFile {
            path: &item.abs,
            file: None,
            done: false,
            tracker: tracker_ref,
        });
        let pair: Result<FileSource<'_>> = Ok((item.meta.clone(), src));
        pair
    });
    if let Err(e) = writer.add_files(&mut files) {
        // A cancelled read surfaces as an I/O error from the format backend.
        return Err(if cancel.is_cancelled() {
            Error::Cancelled
        } else {
            e
        });
    }
    cancel.check()?;
    writer.finish()?;
    report.files = items.iter().filter(|i| !i.is_dir).count() as u64;
    report.bytes = bytes_total;
    tracker.finish();
    Ok(())
}
