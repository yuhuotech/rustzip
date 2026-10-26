use crate::codec::{Compression, Sink};
use rz_core::{
    ArchiveReader, ArchiveWriter, CancelToken, Caps, CreateOptions, Entry, EntryKind, Error,
    NewEntry, NoProgress, Progress, ProgressSink, Result,
};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const LIST_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const LIST_PROGRESS_BYTES: u64 = 1 << 20;

struct ListingReader<'a> {
    inner: Box<dyn Read + Send>,
    cancel: CancelToken,
    progress: &'a mut dyn ProgressSink,
    entries_seen: Arc<AtomicU64>,
    bytes_done: u64,
    last_reported_bytes: u64,
    last_update: Instant,
}

impl Read for ListingReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        let count = self.inner.read(buffer)?;
        if count > 0 {
            self.bytes_done = self.bytes_done.saturating_add(count as u64);
            if self.bytes_done.saturating_sub(self.last_reported_bytes) >= LIST_PROGRESS_BYTES
                || self.last_update.elapsed() >= LIST_PROGRESS_INTERVAL
            {
                self.progress.update(Progress {
                    files_done: self.entries_seen.load(Ordering::Relaxed),
                    files_total: 0,
                    bytes_done: self.bytes_done,
                    bytes_total: 0,
                });
                self.last_reported_bytes = self.bytes_done;
                self.last_update = Instant::now();
            }
        }
        Ok(count)
    }
}

/// tar is a sequential format. `entries` and `for_each` each make one pass over
/// the file; `open` rescans, so bulk work should go through `for_each`.
pub struct TarReader {
    path: PathBuf,
    comp: Compression,
}

impl TarReader {
    pub fn new(path: &Path, comp: Compression) -> Self {
        Self {
            path: path.to_path_buf(),
            comp,
        }
    }

    fn stream(&self) -> Result<Box<dyn Read + Send>> {
        self.comp.reader(File::open(&self.path)?)
    }
}

fn corrupt(e: std::io::Error) -> Error {
    Error::Corrupt(e.to_string())
}

fn describe<R: Read>(index: usize, item: &tar::Entry<'_, R>) -> Result<Entry> {
    let header = item.header();
    let kind = match header.entry_type() {
        t if t.is_dir() => EntryKind::Dir,
        t if t.is_symlink() || t.is_hard_link() => EntryKind::Symlink,
        t if t.is_file() || t.is_contiguous() => EntryKind::File,
        // Devices, FIFOs and metadata records are never extracted.
        _ => EntryKind::Symlink,
    };
    let path = item
        .path()
        .map_err(corrupt)?
        .to_string_lossy()
        .replace('\\', "/");
    Ok(Entry {
        index,
        path,
        kind,
        size: item.size(),
        packed_size: None,
        modified: header.mtime().ok().and_then(|t| i64::try_from(t).ok()),
        encrypted: false,
        unix_mode: header.mode().ok(),
    })
}

impl ArchiveReader for TarReader {
    fn format_name(&self) -> &'static str {
        match self.comp {
            Compression::None => "tar",
            Compression::Gzip => "tar.gz",
            Compression::Xz => "tar.xz",
            Compression::Zstd => "tar.zst",
            Compression::Bzip2 => "tar.bz2",
            Compression::Lz4 => "tar.lz4",
        }
    }

    fn caps(&self) -> Caps {
        Caps {
            random_access: false,
            solid: true,
            writable: true,
        }
    }

    fn entries(&mut self) -> Result<Vec<Entry>> {
        self.entries_with_progress(&CancelToken::new(), &mut NoProgress)
    }

    fn entries_with_progress(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<Vec<Entry>> {
        cancel.check()?;
        let entries_seen = Arc::new(AtomicU64::new(0));
        let mut archive = tar::Archive::new(ListingReader {
            inner: self.stream()?,
            cancel: cancel.clone(),
            progress,
            entries_seen: Arc::clone(&entries_seen),
            bytes_done: 0,
            last_reported_bytes: 0,
            last_update: Instant::now(),
        });
        let mut out = Vec::new();
        let iterator = archive.entries().map_err(|error| {
            if cancel.is_cancelled() {
                Error::Cancelled
            } else {
                corrupt(error)
            }
        })?;
        for (index, item) in iterator.enumerate() {
            cancel.check()?;
            let item = item.map_err(|error| {
                if cancel.is_cancelled() {
                    Error::Cancelled
                } else {
                    corrupt(error)
                }
            })?;
            let entry = describe(index, &item)?;
            out.push(entry);
            entries_seen.store(index as u64 + 1, Ordering::Relaxed);
        }
        cancel.check()?;
        Ok(out)
    }

    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        let mut archive = tar::Archive::new(self.stream()?);
        let mut data = Vec::new();
        for (index, item) in archive.entries().map_err(corrupt)?.enumerate() {
            let mut item = item.map_err(corrupt)?;
            if index == entry.index {
                item.read_to_end(&mut data).map_err(corrupt)?;
                return Ok(Box::new(std::io::Cursor::new(data)));
            }
        }
        Err(Error::Corrupt(format!("entry {} not found", entry.index)))
    }

    fn for_each(
        &mut self,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let mut archive = tar::Archive::new(self.stream()?);
        for (index, item) in archive.entries().map_err(corrupt)?.enumerate() {
            let mut item = item.map_err(corrupt)?;
            let entry = describe(index, &item)?;
            visit(&entry, &mut item)?;
        }
        Ok(())
    }
}

pub struct TarOut {
    builder: tar::Builder<Sink>,
    portable: bool,
}

impl TarOut {
    pub fn new(file: File, comp: Compression, opts: &CreateOptions) -> Result<Self> {
        if opts.password.as_deref().is_some_and(|p| !p.is_empty()) {
            return Err(Error::Unsupported(
                "tar archives cannot be encrypted; use zip or 7z".into(),
            ));
        }
        let sink = comp.writer(file, opts.level, opts.threads)?;
        Ok(Self {
            builder: tar::Builder::new(sink),
            portable: opts.portable,
        })
    }

    fn header(e: &NewEntry, dir: bool, portable: bool) -> tar::Header {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(if dir {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        });
        h.set_size(if dir { 0 } else { e.size });
        let mode = if portable {
            if dir {
                0o755
            } else {
                0o644
            }
        } else {
            e.unix_mode.unwrap_or(if dir { 0o755 } else { 0o644 })
        };
        h.set_mode(mode & 0o777);
        h.set_mtime(e.modified.and_then(|t| u64::try_from(t).ok()).unwrap_or(0));
        h
    }
}

impl ArchiveWriter for TarOut {
    fn add_dir(&mut self, e: &NewEntry) -> Result<()> {
        let mut h = Self::header(e, true, self.portable);
        let name = format!("{}/", e.name.trim_end_matches('/'));
        self.builder
            .append_data(&mut h, name, std::io::empty())
            .map_err(Error::Io)
    }

    fn add_file(&mut self, e: &NewEntry, src: &mut dyn Read) -> Result<()> {
        let mut h = Self::header(e, false, self.portable);
        self.builder
            .append_data(&mut h, &e.name, src)
            .map_err(Error::Io)
    }

    fn finish(self: Box<Self>) -> Result<()> {
        self.builder.into_inner()?.finish()
    }
}

#[cfg(test)]
mod tests {
    use rz_core::NewEntry;

    #[test]
    fn portable_headers_use_default_permissions_instead_of_source_permissions() {
        let entry = NewEntry {
            name: "run.sh".into(),
            size: 0,
            unix_mode: Some(0o755),
            modified: None,
        };
        let header = super::TarOut::header(&entry, false, true);
        assert_eq!(header.mode().unwrap_or_default(), 0o644);
    }
}
