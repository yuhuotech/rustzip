use crate::zip_fmt::map_err;
use rz_core::{CancelToken, Error, Progress, ProgressSink, Result};
use std::cell::Cell;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use zip::{ZipArchive, ZipWriter};

struct Source<'a> {
    file: File,
    cancel: &'a CancelToken,
}

impl Read for Source<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.cancel
            .check()
            .map_err(|_| io::Error::other("cancelled"))?;
        self.file.read(buf)
    }
}

impl Seek for Source<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.cancel
            .check()
            .map_err(|_| io::Error::other("cancelled"))?;
        self.file.seek(pos)
    }
}

struct Output<'a> {
    file: File,
    cancel: &'a CancelToken,
    progress: &'a mut dyn ProgressSink,
    files_done: Rc<Cell<u64>>,
    files_total: u64,
    bytes_done: u64,
    bytes_total: u64,
    last: Option<Instant>,
}

impl Write for Output<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.cancel
            .check()
            .map_err(|_| io::Error::other("cancelled"))?;
        let written = self.file.write(data)?;
        self.bytes_done = self.bytes_done.saturating_add(written as u64);
        if self
            .last
            .is_none_or(|last| last.elapsed() >= Duration::from_millis(50))
        {
            self.progress.update(Progress {
                files_done: self.files_done.get(),
                files_total: self.files_total,
                bytes_done: self.bytes_done.min(self.bytes_total),
                bytes_total: self.bytes_total,
            });
            self.last = Some(Instant::now());
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for Output<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.cancel
            .check()
            .map_err(|_| io::Error::other("cancelled"))?;
        self.file.seek(pos)
    }
}

/// Checks that display names exactly match the archive's UTF-8 name bytes.
pub fn names_match(source: File, names: &[String], cancel: &CancelToken) -> Result<bool> {
    let source = Source {
        file: source,
        cancel,
    };
    let mut archive = ZipArchive::new(source).map_err(|error| {
        if cancel.is_cancelled() {
            Error::Cancelled
        } else {
            map_err(error)
        }
    })?;
    if archive.len() != names.len()
        || !archive.comment().is_empty()
        || archive.zip64_comment().is_some()
    {
        return Ok(false);
    }
    for (index, name) in names.iter().enumerate() {
        cancel.check()?;
        let entry = archive.by_index_raw(index).map_err(|error| {
            if cancel.is_cancelled() {
                Error::Cancelled
            } else {
                map_err(error)
            }
        })?;
        if entry.name_raw() != name.as_bytes() || !entry.comment().is_empty() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Copies ZIP entries without recompressing them. The caller validates paths,
/// limits and output content before publishing the staged file.
pub fn copy_entries(
    source: File,
    output: File,
    names: &[Option<String>],
    replacement: Option<(usize, PathBuf, u32)>,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let total_bytes = source.metadata()?.len().max(1);
    let source = Source {
        file: source,
        cancel,
    };
    let mut archive = ZipArchive::new(source).map_err(|error| {
        if cancel.is_cancelled() {
            Error::Cancelled
        } else {
            map_err(error)
        }
    })?;
    if archive.len() != names.len() {
        return Err(Error::Corrupt("ZIP entry count changed".into()));
    }
    let done = Rc::new(Cell::new(0));
    let output = Output {
        file: output,
        cancel,
        progress,
        files_done: Rc::clone(&done),
        files_total: names.iter().filter(|name| name.is_some()).count() as u64,
        bytes_done: 0,
        bytes_total: total_bytes,
        last: None,
    };
    let mut writer = ZipWriter::new(output);
    for (index, name) in names.iter().enumerate() {
        cancel.check()?;
        let Some(name) = name else { continue };
        let entry = archive.by_index_raw(index).map_err(|error| {
            if cancel.is_cancelled() {
                Error::Cancelled
            } else {
                map_err(error)
            }
        })?;
        let copied = if let Some((_, path, level)) =
            replacement.as_ref().filter(|(idx, _, _)| *idx == index)
        {
            drop(entry);
            let meta = std::fs::metadata(path)?;
            let mut options = zip::write::SimpleFileOptions::default()
                .large_file(meta.len() >= u64::from(u32::MAX));
            options = if *level == 0 {
                options.compression_method(zip::CompressionMethod::Stored)
            } else {
                options
                    .compression_method(zip::CompressionMethod::Deflated)
                    .compression_level(Some(i64::from((*level).min(9))))
            };
            if let Some(mode) = rz_platform::mode_of(&meta) {
                options = options.unix_permissions(mode);
            }
            if let Some(time) = rz_platform::mtime_of(&meta).and_then(crate::zip_fmt::unix_to_dos) {
                options = options.last_modified_time(time);
            }
            writer.start_file(name, options).map_err(map_err)?;
            let mut source = Source {
                file: File::open(path)?,
                cancel,
            };
            io::copy(&mut source, &mut writer)
                .map(|_| ())
                .map_err(zip::result::ZipError::Io)
        } else if entry.is_dir() {
            let mut options = zip::write::SimpleFileOptions::default()
                .unix_permissions(entry.unix_mode().unwrap_or(0o755) & 0o777);
            if let Some(time) = entry.last_modified() {
                options = options.last_modified_time(time);
            }
            writer.add_directory(name, options)
        } else {
            writer.raw_copy_file_rename(entry, name)
        };
        if let Err(error) = copied {
            return Err(if cancel.is_cancelled() {
                Error::Cancelled
            } else {
                map_err(error)
            });
        }
        done.set(done.get() + 1);
    }
    let output = writer.finish().map_err(|error| {
        if cancel.is_cancelled() {
            Error::Cancelled
        } else {
            map_err(error)
        }
    })?;
    output.file.sync_all()?;
    cancel.check()
}
