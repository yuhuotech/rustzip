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

impl Read for Output<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.cancel
            .check()
            .map_err(|_| io::Error::other("cancelled"))?;
        self.file.read(buffer)
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
    if archive.len() != names.len() || archive.offset() != 0 {
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
        if entry.name_raw() != name.as_bytes() {
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
    let archive_comment = archive.comment().to_vec().into_boxed_slice();
    let zip64_extensible_data = archive
        .raw_zip64_extensible_data_sector()
        .map(|data| data.to_vec().into_boxed_slice());
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
    writer.set_raw_comment(archive_comment).map_err(map_err)?;
    if let Some(data) = zip64_extensible_data {
        writer.set_raw_zip64_extensible_data_sector(data);
    }
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
            let file_comment = entry.comment().to_owned();
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
            writer
                .start_file(
                    name,
                    options.into_full_options().with_file_comment(file_comment),
                )
                .map_err(map_err)?;
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

/// Writes changed entries and a rebuilt central directory into a staged copy.
/// Unchanged entries keep pointing at their original local records; only renamed
/// and replaced entries get new local records.
pub fn append_entries(
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

    let changed_total = names
        .iter()
        .enumerate()
        .filter(|(index, name)| {
            name.is_none()
                || replacement
                    .as_ref()
                    .is_some_and(|(replace, _, _)| replace == index)
                || name.as_deref().is_some_and(|name| {
                    archive.by_index_raw(*index).is_ok_and(|entry| {
                        entry.name().is_ok_and(|old_name| old_name.as_ref() != name)
                    })
                })
        })
        .count() as u64;
    let done = Rc::new(Cell::new(0));
    let output = Output {
        file: output,
        cancel,
        progress,
        files_done: Rc::clone(&done),
        files_total: changed_total.max(1),
        bytes_done: 0,
        bytes_total: total_bytes,
        last: None,
    };
    let mut writer = ZipWriter::new_append(output).map_err(|error| {
        if cancel.is_cancelled() {
            Error::Cancelled
        } else {
            map_err(error)
        }
    })?;

    // Remove superseded central-directory records before adding their new names.
    // The source reader stays separate, so renamed payloads remain available.
    for (index, name) in names.iter().enumerate() {
        cancel.check()?;
        let entry = archive.by_index_raw(index).map_err(|error| {
            if cancel.is_cancelled() {
                Error::Cancelled
            } else {
                map_err(error)
            }
        })?;
        let old_name = entry.name().map_err(map_err)?.into_owned();
        let changed = name.as_deref().is_none_or(|name| name != old_name)
            || replacement
                .as_ref()
                .is_some_and(|(replace, _, _)| *replace == index);
        drop(entry);
        if changed {
            writer.hide_file(&old_name).map_err(|error| {
                if cancel.is_cancelled() {
                    Error::Cancelled
                } else {
                    map_err(error)
                }
            })?;
            if name.is_none() {
                done.set(done.get().saturating_add(1));
            }
        }
    }

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
        let old_name = entry.name().map_err(map_err)?.into_owned();
        if let Some((_, path, level)) = replacement
            .as_ref()
            .filter(|(replace, _, _)| *replace == index)
        {
            let file_comment = entry.comment().to_owned();
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
            writer
                .start_file(
                    name,
                    options.into_full_options().with_file_comment(file_comment),
                )
                .map_err(map_err)?;
            let mut input = Source {
                file: File::open(path)?,
                cancel,
            };
            io::copy(&mut input, &mut writer)
                .map_err(zip::result::ZipError::Io)
                .map_err(map_err)?;
        } else if *name != old_name {
            writer.raw_copy_file_rename(entry, name).map_err(map_err)?;
        } else {
            continue;
        }
        done.set(done.get().saturating_add(1));
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
