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

pub trait ArchiveWriter: Send {
    fn add_dir(&mut self, entry: &NewEntry) -> Result<()>;
    /// Copies `src` into the archive as one file entry.
    fn add_file(&mut self, entry: &NewEntry, src: &mut dyn Read) -> Result<()>;
    /// Writes trailing structures and flushes everything to disk.
    fn finish(self: Box<Self>) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    /// 0 stores without compression, 9 is the smallest output.
    pub level: u32,
    pub password: Option<String>,
}

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            level: 6,
            password: None,
        }
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
