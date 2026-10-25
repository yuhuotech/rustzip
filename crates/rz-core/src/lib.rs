//! Core types shared by every RustZIP layer.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

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
    #[error("destination does not distinguish the archive names '{first}' and '{second}'; use a case-sensitive volume to preserve both files")]
    NameConflict { first: String, second: String },
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
    /// Bounds work done before an entry stream can be exposed (for example, measuring
    /// a gzip stream whose uncompressed size is not stored in its header).
    fn set_read_control(&mut self, _cancel: CancelToken, _max_total: u64) {}
    fn format_name(&self) -> &'static str;
    fn caps(&self) -> Caps;
    fn entries(&mut self) -> Result<Vec<Entry>>;
    /// Sets the password used to open encrypted entries. Formats without
    /// encryption ignore it.
    fn set_password(&mut self, _password: &str) -> Result<()> {
        Ok(())
    }
    /// Forces the character set used to decode entry names (e.g. `"gbk"`,
    /// `"shift_jis"`). Without it, readers guess when names are not UTF-8.
    /// Formats whose names are always Unicode ignore it.
    fn set_encoding(&mut self, _label: &str) -> Result<()> {
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

#[derive(Clone)]
pub struct CreateOptions {
    /// 0 stores without compression, 9 is the smallest output.
    pub level: u32,
    pub password: Option<String>,
    /// Worker threads; 0 means one per available CPU.
    pub threads: usize,
    /// Upper bound for data held in memory by parallel workers.
    pub memory_budget: u64,
}

impl std::fmt::Debug for CreateOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Debug output goes to logs; passwords must never appear there.
        f.debug_struct("CreateOptions")
            .field("level", &self.level)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("threads", &self.threads)
            .field("memory_budget", &self.memory_budget)
            .finish()
    }
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

/// An entry path the way a user sees it: `/` separators, no empty or `.` parts,
/// no leading or trailing slash. `..` is kept as it is; extraction rejects it.
pub fn clean_path(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// True when `path` is `selected` itself or lies inside it (both cleaned).
pub fn is_within(path: &str, selected: &str) -> bool {
    path == selected
        || (path.len() > selected.len()
            && path.starts_with(selected)
            && path.as_bytes()[selected.len()] == b'/')
        || selected.is_empty()
}

/// Resolves a user-facing thread setting (`0` = automatic) to a real count.
pub fn resolve_threads(requested: usize) -> usize {
    if requested > 0 {
        requested
    } else {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    }
}

#[derive(Debug, Default)]
struct TokenState {
    cancelled: AtomicBool,
    paused: Mutex<bool>,
    resumed: Condvar,
}

/// Cloneable control handle checked by long-running operations: cancel, pause, resume.
///
/// Operations call [`check`](Self::check) in their loops. While paused, `check`
/// blocks, which stops every worker at its next checkpoint without losing state.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<TokenState>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
        // A paused task must wake up to notice it was cancelled. Taking the lock the
        // waiter holds between its check and its wait closes the window in which this
        // notification could otherwise be lost.
        let _guard = self.0.paused.lock();
        self.0.resumed.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    pub fn pause(&self) {
        if let Ok(mut p) = self.0.paused.lock() {
            *p = true;
        }
    }

    pub fn resume(&self) {
        if let Ok(mut p) = self.0.paused.lock() {
            *p = false;
        }
        self.0.resumed.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.0.paused.lock().is_ok_and(|p| *p)
    }

    /// Returns `Err(Cancelled)` once cancelled; waits here while paused.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Ok(mut paused) = self.0.paused.lock() {
            while *paused && !self.is_cancelled() {
                paused = match self.0.resumed.wait(paused) {
                    Ok(p) => p,
                    Err(_) => break,
                };
            }
        }
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
    fn clean_path_normalises_separators_and_dots() {
        assert_eq!(clean_path("a/b/c.txt"), "a/b/c.txt");
        assert_eq!(clean_path("./a//b/"), "a/b");
        assert_eq!(clean_path("/abs/x"), "abs/x");
        assert_eq!(clean_path("a\\b\\c"), "a/b/c");
        assert_eq!(clean_path(""), "");
        assert_eq!(clean_path("./"), "");
        assert_eq!(clean_path("a/../b"), "a/../b");
        assert_eq!(clean_path("项目/说明.md"), "项目/说明.md");
    }

    #[test]
    fn is_within_matches_whole_path_parts_only() {
        assert!(is_within("a/b", "a"));
        assert!(is_within("a", "a"));
        assert!(is_within("a/b/c", "a/b"));
        assert!(!is_within("ab/c", "a"));
        assert!(!is_within("a", "a/b"));
        assert!(is_within("anything", ""));
        assert!(!is_within("a/b", "b"));
    }

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(b.check().is_ok());
        a.cancel();
        assert!(matches!(b.check(), Err(Error::Cancelled)));
    }

    #[test]
    fn check_blocks_while_paused_and_continues_after_resume() {
        let t = CancelToken::new();
        t.pause();
        assert!(t.is_paused());
        let t2 = t.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || {
            let r = t2.check();
            let _ = tx.send(r.is_ok());
        });
        // Still blocked after a moment.
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(150))
            .is_err());
        t.resume();
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true));
        assert!(h.join().is_ok());
    }

    #[test]
    fn cancel_wakes_a_paused_checker() {
        let t = CancelToken::new();
        t.pause();
        let t2 = t.clone();
        let h = std::thread::spawn(move || t2.check());
        std::thread::sleep(std::time::Duration::from_millis(100));
        t.cancel();
        assert!(matches!(h.join(), Ok(Err(Error::Cancelled))));
    }

    #[test]
    fn cancel_never_loses_its_wakeup_when_racing_with_a_checker() {
        // Regression: cancel() used to notify without the lock, so a checker about to
        // wait could miss the signal and sleep forever.
        for _ in 0..2000 {
            let t = CancelToken::new();
            t.pause();
            let t2 = t.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            let h = std::thread::spawn(move || {
                let _ = tx.send(t2.check());
            });
            t.cancel();
            let got = rx.recv_timeout(std::time::Duration::from_secs(10));
            assert!(matches!(got, Ok(Err(Error::Cancelled))), "{got:?}");
            let _ = h.join();
        }
    }
}
