use crate::progress::Tracker;
use crate::safe_path::safe_join;
use rz_core::{
    clean_path, is_within, resolve_threads, ArchiveReader, CancelToken, Entry, EntryKind, Error,
    ProgressSink, Result,
};
use rz_platform::{apply_mode, set_mtime};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

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
    /// Largest allowed ratio of unpacked to packed bytes for an entry...
    pub max_ratio: u64,
    /// ...checked only for entries that unpack to more than this many bytes.
    pub ratio_check_above: u64,
    /// Largest allowed total unpacked size.
    pub max_total: u64,
    pub password: Option<String>,
    /// Worker threads for formats that allow parallel extraction; 0 means one per CPU.
    pub threads: usize,
    /// Extract only these paths (and everything inside the folders among them).
    /// Paths are as listed by the archive; `None` extracts everything.
    pub only: Option<Vec<String>>,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            overwrite: Overwrite::Error,
            max_ratio: 2000,
            ratio_check_above: 256 << 20,
            max_total: 1 << 40,
            password: None,
            threads: 0,
            only: None,
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

/// Directory attributes applied after all children are written.
type DirMeta = (PathBuf, Option<u32>, Option<i64>);

struct Ctx<'a> {
    dest: &'a Path,
    opts: &'a ExtractOptions,
    tracker: &'a Tracker<'a>,
    /// Indexes of the entries chosen for selective extraction, if any.
    wanted: Option<HashSet<usize>>,
    files: AtomicU64,
    dirs: AtomicU64,
    skipped: AtomicU64,
    bytes: AtomicU64,
    dir_meta: Mutex<Vec<DirMeta>>,
}

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
    let mut entries = reader.entries()?;
    // Selective extraction: keep the chosen paths and everything inside chosen folders.
    let wanted: Option<HashSet<usize>> = opts.only.as_ref().map(|only| {
        let only: Vec<String> = only.iter().map(|p| clean_path(p)).collect();
        entries.retain(|e| {
            let path = clean_path(&e.path);
            only.iter().any(|s| is_within(&path, s))
        });
        entries.iter().map(|e| e.index).collect()
    });
    let declared: u64 = entries.iter().map(|e| e.size).fold(0, u64::saturating_add);
    if declared > opts.max_total {
        return Err(Error::LimitExceeded(format!("total size {declared} bytes")));
    }
    // Check everything before writing anything, so a bad archive leaves no partial result.
    let duplicates = preflight(&entries, dest, opts)?;
    // A task that is paused or cancelled before it starts must not touch the disk,
    // not even to create the destination folder.
    cancel.check()?;
    fs::create_dir_all(dest)?;

    let tracker = Tracker::new(progress, cancel, entries.len() as u64, declared);
    let ctx = Ctx {
        dest,
        opts,
        tracker: &tracker,
        wanted,
        files: AtomicU64::new(0),
        dirs: AtomicU64::new(0),
        skipped: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        dir_meta: Mutex::new(Vec::new()),
    };

    let file_count = entries.iter().filter(|e| e.kind == EntryKind::File).count();
    let threads = resolve_threads(opts.threads).min(file_count);
    if reader.caps().random_access && threads > 1 && !duplicates {
        match run_parallel(reader, &entries, &ctx, threads) {
            Parallel::Done(res) => res?,
            Parallel::Unavailable => run_sequential(reader, &ctx)?,
        }
    } else {
        run_sequential(reader, &ctx)?;
    }

    // Deepest directories first, so setting a parent's mtime is not undone by its children.
    let dir_meta = ctx
        .dir_meta
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default();
    for (dir, mode, modified) in dir_meta.into_iter().rev() {
        if let Some(mode) = mode {
            apply_mode(&dir, mode | 0o700)?;
        }
        if let Some(t) = modified {
            // Directory times are best effort; some platforms cannot open a directory for writing.
            let _ = set_mtime(&dir, t);
        }
    }
    tracker.finish();
    Ok(ExtractReport {
        files: ctx.files.load(Ordering::Relaxed),
        dirs: ctx.dirs.load(Ordering::Relaxed),
        skipped: ctx.skipped.load(Ordering::Relaxed),
        bytes: ctx.bytes.load(Ordering::Relaxed),
    })
}

/// Validates every path and the overwrite policy up front.
/// Returns whether two entries would land on the same file (name clash).
fn preflight(entries: &[Entry], dest: &Path, opts: &ExtractOptions) -> Result<bool> {
    let mut seen = HashSet::new();
    let mut duplicates = false;
    for e in entries {
        let target = safe_join(dest, &e.path)?;
        if e.kind != EntryKind::File {
            continue;
        }
        if opts.overwrite == Overwrite::Error && target.symlink_metadata().is_ok() {
            return Err(already_exists(&target));
        }
        // Compare case-insensitively: on macOS and Windows these would collide.
        if !seen.insert(target.to_string_lossy().to_lowercase()) {
            duplicates = true;
        }
    }
    Ok(duplicates)
}

fn already_exists(path: &Path) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        path.display().to_string(),
    ))
}

fn run_sequential(reader: &mut dyn ArchiveReader, ctx: &Ctx<'_>) -> Result<()> {
    let mut buf = vec![0u8; BUF];
    reader.for_each(&mut |entry, data| {
        if ctx
            .wanted
            .as_ref()
            .is_some_and(|w| !w.contains(&entry.index))
        {
            return Ok(()); // not selected; the reader skips its data
        }
        handle(ctx, entry, || Ok(&mut *data), &mut buf)
    })
}

enum Parallel {
    Done(Result<()>),
    /// The format cannot open independent readers; fall back to one pass.
    Unavailable,
}

/// Splits the files over `threads` readers, biggest first onto the least loaded.
fn run_parallel(
    reader: &dyn ArchiveReader,
    entries: &[Entry],
    ctx: &Ctx<'_>,
    threads: usize,
) -> Parallel {
    let mut readers = Vec::with_capacity(threads);
    for _ in 0..threads {
        match reader.reopen() {
            Some(Ok(r)) => readers.push(r),
            Some(Err(e)) => return Parallel::Done(Err(e)),
            None => return Parallel::Unavailable,
        }
    }

    // Directories and links first, in archive order, on this thread.
    let mut buf = vec![0u8; BUF];
    let mut files: Vec<&Entry> = Vec::new();
    for e in entries {
        if e.kind == EntryKind::File {
            files.push(e);
        } else if let Err(err) = handle(ctx, e, || Ok(std::io::empty()), &mut buf) {
            return Parallel::Done(Err(err));
        }
    }
    files.sort_by_key(|e| std::cmp::Reverse(e.size));
    let mut load = vec![0u64; threads];
    let mut plan: Vec<Vec<&Entry>> = vec![Vec::new(); threads];
    for e in files {
        let lightest = (0..threads).min_by_key(|i| load[*i]).unwrap_or(0);
        load[lightest] += e.size.max(1);
        plan[lightest].push(e);
    }

    let failure: Mutex<Option<Error>> = Mutex::new(None);
    let abort = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for (mut r, chunk) in readers.into_iter().zip(plan) {
            let (failure, abort) = (&failure, &abort);
            scope.spawn(move || {
                let mut buf = vec![0u8; BUF];
                for e in chunk {
                    if abort.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Err(err) = handle(ctx, e, || r.open(e), &mut buf) {
                        abort.store(true, Ordering::Relaxed);
                        if let Ok(mut slot) = failure.lock() {
                            // Keep the first error; later ones are usually fallout.
                            slot.get_or_insert(err);
                        }
                        return;
                    }
                }
            });
        }
    });
    let err = failure.lock().ok().and_then(|mut s| s.take());
    Parallel::Done(err.map_or(Ok(()), Err))
}

/// Writes one entry. `src` is only called when the data is actually needed,
/// so skipped entries cost nothing.
fn handle<R: Read>(
    ctx: &Ctx<'_>,
    entry: &Entry,
    src: impl FnOnce() -> Result<R>,
    buf: &mut [u8],
) -> Result<()> {
    ctx.tracker.check()?;
    let target = safe_join(ctx.dest, &entry.path)?;
    match entry.kind {
        EntryKind::Dir => {
            fs::create_dir_all(&target)?;
            if let Ok(mut v) = ctx.dir_meta.lock() {
                v.push((target, entry.unix_mode, entry.modified));
            }
            ctx.dirs.fetch_add(1, Ordering::Relaxed);
        }
        EntryKind::Symlink => {
            // Symlinks can point outside the destination; skipped until a link policy exists.
            ctx.skipped.fetch_add(1, Ordering::Relaxed);
        }
        EntryKind::File => {
            if target.symlink_metadata().is_ok() && ctx.opts.overwrite == Overwrite::Skip {
                ctx.skipped.fetch_add(1, Ordering::Relaxed);
                ctx.tracker.add_bytes(entry.size);
                ctx.tracker.file_done();
                return Ok(());
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            // Small entries cannot hurt however well they compress. Large ones with an
            // extreme ratio are the classic compression bomb.
            if let Some(packed) = entry.packed_size {
                let allowed = packed.max(1).saturating_mul(ctx.opts.max_ratio);
                if entry.size > ctx.opts.ratio_check_above && entry.size > allowed {
                    return Err(Error::LimitExceeded(format!(
                        "{}: compression ratio too high",
                        entry.path
                    )));
                }
            }
            // A header that understates the size must not become an unbounded write.
            let limit = entry.size;
            let mut src = src()?;
            let mut open = OpenOptions::new();
            open.write(true);
            if ctx.opts.overwrite == Overwrite::Error {
                open.create_new(true); // never clobber, even if the file appeared since the preflight
            } else {
                open.create(true).truncate(true);
            }
            let mut out = open.open(&target).map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => already_exists(&target),
                _ => Error::Io(e),
            })?;
            let copied = copy_limited(&mut src, &mut out, buf, limit, &entry.path, ctx);
            drop(out);
            if let Err(e) = copied {
                let _ = fs::remove_file(&target); // do not leave a truncated file behind
                return Err(e);
            }
            if let Some(mode) = entry.unix_mode {
                apply_mode(&target, mode)?;
            }
            if let Some(t) = entry.modified {
                set_mtime(&target, t)?;
            }
            ctx.files.fetch_add(1, Ordering::Relaxed);
        }
    }
    ctx.tracker.file_done();
    Ok(())
}

fn copy_limited(
    src: &mut impl Read,
    out: &mut impl Write,
    buf: &mut [u8],
    limit: u64,
    name: &str,
    ctx: &Ctx<'_>,
) -> Result<()> {
    let mut written = 0u64;
    loop {
        ctx.tracker.check()?;
        let n = src.read(buf)?;
        if n == 0 {
            return Ok(());
        }
        written += n as u64;
        if written > limit {
            return Err(Error::LimitExceeded(format!(
                "{name}: exceeds declared size"
            )));
        }
        out.write_all(&buf[..n])?;
        ctx.bytes.fetch_add(n as u64, Ordering::Relaxed);
        ctx.tracker.add_bytes(n as u64);
    }
}
