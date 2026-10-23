//! Core types shared by every RustZIP layer.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unrecognized archive format")]
    UnknownFormat,
    #[error("format not supported yet: {0}")]
    Unsupported(String),
    #[error("corrupt archive: {0}")]
    Corrupt(String),
    #[error("wrong password or encrypted archive")]
    Password,
    #[error("unsafe path in archive: {0}")]
    UnsafePath(String),
    #[error("archive exceeds safety limit: {0}")]
    LimitExceeded(String),
    #[error("operation cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

/// One item inside an archive.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Index used to open the entry again with [`ArchiveReader::open`].
    pub index: usize,
    /// Path as stored in the archive, `/`-separated.
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub packed_size: Option<u64>,
    /// Seconds since the Unix epoch, when the archive records one.
    pub modified: Option<i64>,
    pub encrypted: bool,
    pub unix_mode: Option<u32>,
}

/// What a format backend can do; the engine picks a strategy from this.
#[derive(Debug, Clone, Copy, Default)]
pub struct Caps {
    pub random_access: bool,
    pub solid: bool,
    pub writable: bool,
}

pub trait ArchiveReader: Send {
    fn format_name(&self) -> &'static str;
    fn caps(&self) -> Caps;
    fn entries(&mut self) -> Result<Vec<Entry>>;
    /// Sets the password used to open encrypted entries. Formats without
    /// encryption ignore it.
    fn set_password(&mut self, _password: &str) -> Result<()> {
        Ok(())
    }
    /// Opens one entry for reading. The returned reader borrows the archive.
    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>>;

    /// Visits every entry with its data stream (empty for directories and links).
    ///
    /// This is the fast path for bulk work: sequential formats such as tar and
    /// solid 7z decode the archive once instead of once per entry. The visitor
    /// may leave a stream partly unread; the implementation skips the rest.
    /// Entries are not guaranteed to arrive in archive order.
    fn for_each(
        &mut self,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        for entry in self.entries()? {
            let mut data = self.open(&entry)?;
            visit(&entry, &mut data)?;
        }
        Ok(())
    }

    /// Opens an independent reader on the same archive, so several threads can
    /// extract at once. `None` for formats that cannot be read in parallel.
    fn reopen(&self) -> Option<Result<Box<dyn ArchiveReader>>> {
        None
    }
}

/// Metadata for an entry about to be written.
#[derive(Debug, Clone)]
pub struct NewEntry {
    /// Path inside the archive, `/`-separated.
    pub name: String,
    pub size: u64,
    pub unix_mode: Option<u32>,
    /// Seconds since the Unix epoch.
    pub modified: Option<i64>,
}

/// A file to be added: its metadata and a readable, thread-movable data stream.
pub type FileSource<'a> = (NewEntry, Box<dyn Read + Send + 'a>);

pub trait ArchiveWriter: Send {
    fn add_dir(&mut self, entry: &NewEntry) -> Result<()>;
    /// Copies `src` into the archive as one file entry.
    fn add_file(&mut self, entry: &NewEntry, src: &mut dyn Read) -> Result<()>;
    /// Adds many files at once, in iterator order.
    ///
    /// Backends override this to do better than one-by-one: 7z packs the files
    /// into one solid block, zip compresses several files on different threads.
    fn add_files<'a>(
        &mut self,
        files: &mut dyn Iterator<Item = Result<FileSource<'a>>>,
    ) -> Result<()> {
        for item in files {
            let (entry, mut src) = item?;
            self.add_file(&entry, &mut src)?;
        }
        Ok(())
    }
    /// Writes trailing structures and flushes everything to disk.
    fn finish(self: Box<Self>) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    /// 0 stores without compression, 9 is the smallest output.
    pub level: u32,
    pub password: Option<String>,
    /// Worker threads; 0 means one per available CPU.
    pub threads: usize,
    /// Upper bound for data held in memory by parallel workers.
    pub memory_budget: u64,
}

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            level: 6,
            password: None,
            threads: 0,
            memory_budget: 256 << 20,
        }
    }
}

/// Resolves a user-facing thread setting (`0` = automatic) to a real count.
pub fn resolve_threads(requested: usize) -> usize {
    if requested > 0 {
        requested
    } else {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    }
}

/// Cloneable cancellation flag checked by long-running operations.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Progress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

pub trait ProgressSink: Send {
    fn update(&mut self, progress: Progress);
}

impl<F: FnMut(Progress) + Send> ProgressSink for F {
    fn update(&mut self, progress: Progress) {
        self(progress)
    }
}

/// Ignores all progress updates.
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn update(&mut self, _: Progress) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(b.check().is_ok());
        a.cancel();
        assert!(matches!(b.check(), Err(Error::Cancelled)));
    }
}
