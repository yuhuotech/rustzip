use libchm::{ChmError, ChmFile, Entry as ChmEntry, EntryKind as ChmEntryKind, EntrySel};
use rz_core::{
    ArchiveReader, CancelToken, Caps, Entry, EntryKind, Error, NoProgress, Progress, ProgressSink,
    Result,
};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAX_CHM_ENTRIES: usize = 100_000;
const MAX_CHM_METADATA_BYTES: usize = 16 * 1024 * 1024;
const CHM_READ_CHUNK: usize = 64 * 1024;
const CHM_PROGRESS_INTERVAL: usize = 64;
const DEFAULT_MAX_CHM_OUTPUT: u64 = 1 << 40;

struct ReadControl {
    cancel: CancelToken,
    max_total: u64,
    bytes_read: Arc<AtomicU64>,
}

pub struct ChmReader {
    path: PathBuf,
    archive: ChmFile,
    source_entries: Option<Vec<ChmEntry>>,
    read_control: Option<ReadControl>,
}

impl ChmReader {
    pub fn new(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            archive: ChmFile::open(path).map_err(map_error)?,
            source_entries: None,
            read_control: None,
        })
    }

    fn scan_entries(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<Vec<Entry>> {
        cancel.check()?;
        let mut visible = Vec::new();
        let mut source = Vec::new();
        let mut metadata_bytes = 0usize;
        let mut bytes_done = 0u64;
        let mut stopped = None;
        let selected = EntrySel::NORMAL | EntrySel::FILES | EntrySel::DIRS;
        let completed = self
            .archive
            .for_each_entry(
                selected,
                || cancel.check().is_ok(),
                |entry| {
                    let path = entry
                        .path
                        .strip_prefix('/')
                        .unwrap_or(&entry.path)
                        .replace('\\', "/");
                    if path.is_empty() {
                        return true;
                    }
                    let cost = std::mem::size_of::<Entry>()
                        .saturating_add(std::mem::size_of::<ChmEntry>())
                        .saturating_add(path.len())
                        .saturating_add(entry.path.len());
                    let Some(next_metadata_bytes) = metadata_bytes.checked_add(cost) else {
                        stopped = Some(Error::LimitExceeded("CHM directory metadata".into()));
                        return false;
                    };
                    if visible.len() >= MAX_CHM_ENTRIES
                        || next_metadata_bytes > MAX_CHM_METADATA_BYTES
                    {
                        stopped = Some(Error::LimitExceeded("CHM directory metadata".into()));
                        return false;
                    }
                    if visible.try_reserve(1).is_err() || source.try_reserve(1).is_err() {
                        stopped = Some(Error::LimitExceeded("CHM directory metadata".into()));
                        return false;
                    }

                    metadata_bytes = next_metadata_bytes;
                    bytes_done = bytes_done.saturating_add(entry.length);
                    visible.push(Entry {
                        index: visible.len(),
                        path,
                        kind: match entry.kind {
                            ChmEntryKind::File => EntryKind::File,
                            ChmEntryKind::Dir => EntryKind::Dir,
                        },
                        size: entry.length,
                        packed_size: None,
                        modified: None,
                        encrypted: false,
                        unix_mode: None,
                    });
                    source.push(entry);
                    if visible.len() == 1 || visible.len().is_multiple_of(CHM_PROGRESS_INTERVAL) {
                        progress.update(Progress {
                            files_done: visible.len() as u64,
                            files_total: 0,
                            bytes_done,
                            bytes_total: 0,
                        });
                    }
                    true
                },
            )
            .map_err(map_error)?;

        if let Some(error) = stopped {
            return Err(error);
        }
        if !completed {
            return Err(Error::Cancelled);
        }
        cancel.check()?;
        if !visible.is_empty() && !visible.len().is_multiple_of(CHM_PROGRESS_INTERVAL) {
            progress.update(Progress {
                files_done: visible.len() as u64,
                files_total: 0,
                bytes_done,
                bytes_total: 0,
            });
        }
        self.source_entries = Some(source);
        Ok(visible)
    }

    fn ensure_entries(&mut self) -> Result<()> {
        if self.source_entries.is_none() {
            let cancel = self
                .read_control
                .as_ref()
                .map(|control| control.cancel.clone())
                .unwrap_or_default();
            let _ = self.scan_entries(&cancel, &mut NoProgress)?;
        }
        Ok(())
    }
}

impl ArchiveReader for ChmReader {
    fn set_read_control(&mut self, cancel: CancelToken, max_total: u64) {
        self.read_control = Some(ReadControl {
            cancel,
            max_total,
            bytes_read: Arc::new(AtomicU64::new(0)),
        });
    }

    fn format_name(&self) -> &'static str {
        "chm"
    }

    fn caps(&self) -> Caps {
        Caps {
            random_access: false,
            solid: true,
            writable: false,
        }
    }

    fn entries(&mut self) -> Result<Vec<Entry>> {
        let cancel = self
            .read_control
            .as_ref()
            .map(|control| control.cancel.clone())
            .unwrap_or_default();
        self.entries_with_progress(&cancel, &mut NoProgress)
    }

    fn entries_with_progress(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<Vec<Entry>> {
        self.source_entries = None;
        self.scan_entries(cancel, progress)
    }

    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        self.ensure_entries()?;
        let source = self
            .source_entries
            .as_ref()
            .and_then(|entries| entries.get(entry.index))
            .filter(|source| {
                source
                    .path
                    .strip_prefix('/')
                    .unwrap_or(&source.path)
                    .replace('\\', "/")
                    == entry.path
            })
            .cloned()
            .ok_or_else(|| Error::Corrupt("CHM entry index is out of range".into()))?;
        let (cancel, max_total, bytes_read) = self
            .read_control
            .as_ref()
            .map(|control| {
                (
                    control.cancel.clone(),
                    control.max_total,
                    control.bytes_read.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    CancelToken::new(),
                    DEFAULT_MAX_CHM_OUTPUT,
                    Arc::new(AtomicU64::new(0)),
                )
            });

        Ok(Box::new(ChmEntryReader {
            archive: &mut self.archive,
            entry: source,
            position: 0,
            cancel,
            max_total,
            bytes_read,
        }))
    }

    fn reopen(&self) -> Option<Result<Box<dyn ArchiveReader>>> {
        Some(Self::new(&self.path).map(|reader| Box::new(reader) as Box<dyn ArchiveReader>))
    }
}

struct ChmEntryReader<'a> {
    archive: &'a mut ChmFile,
    entry: ChmEntry,
    position: u64,
    cancel: CancelToken,
    max_total: u64,
    bytes_read: Arc<AtomicU64>,
}

impl Read for ChmEntryReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.position >= self.entry.length {
            return Ok(0);
        }
        if self.cancel.check().is_err() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "operation cancelled",
            ));
        }
        let current_total = self.bytes_read.load(Ordering::Relaxed);
        let remaining_limit = self.max_total.saturating_sub(current_total);
        if remaining_limit == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "CHM output exceeds the configured read limit",
            ));
        }
        let remaining_entry = self.entry.length - self.position;
        let request = buffer
            .len()
            .min(CHM_READ_CHUNK)
            .min(usize::try_from(remaining_limit).unwrap_or(usize::MAX))
            .min(usize::try_from(remaining_entry).unwrap_or(usize::MAX));
        let read = self
            .archive
            .read_range_with(&self.entry, self.position, &mut buffer[..request], || {
                self.cancel.check().is_ok()
            })
            .map_err(map_io_error)?;
        self.position = self
            .position
            .checked_add(u64::try_from(read).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("CHM entry offset overflow"))?;
        self.bytes_read.fetch_add(
            u64::try_from(read).map_err(io::Error::other)?,
            Ordering::Relaxed,
        );
        Ok(read)
    }
}

fn map_error(error: ChmError) -> Error {
    match error {
        ChmError::Io(error) => Error::Io(error),
        ChmError::Cancelled => Error::Cancelled,
        ChmError::LimitExceeded(detail) => Error::LimitExceeded(detail.into()),
        other => Error::Corrupt(other.to_string()),
    }
}

fn map_io_error(error: ChmError) -> io::Error {
    match error {
        ChmError::Io(error) => error,
        ChmError::Cancelled => io::Error::new(io::ErrorKind::Interrupted, "operation cancelled"),
        ChmError::LimitExceeded(detail) => io::Error::new(io::ErrorKind::InvalidData, detail),
        other => io::Error::new(io::ErrorKind::InvalidData, other),
    }
}
