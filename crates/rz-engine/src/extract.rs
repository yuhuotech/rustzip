use crate::progress::Tracker;
use crate::safe_path::safe_join;
use rz_core::{
    clean_path, is_within, resolve_threads, ArchiveReader, CancelToken, Entry, EntryKind, Error,
    ParallelBlocks, ProgressSink, Result,
};
use rz_platform::{apply_mode, set_mtime};
use std::collections::{HashMap, HashSet};
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
    /// Keep both: an item that is already there stays, and the new one is written
    /// as `name (1)`. Applies to the top-level item, so a folder keeps its contents together.
    Rename,
}

#[derive(Clone)]
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
    /// Memory budget for independent decoder workers.
    pub memory_budget: u64,
    /// Extract only these paths (and everything inside the folders among them).
    /// Paths are as listed by the archive; `None` extracts everything.
    pub only: Option<Vec<String>>,
}

impl std::fmt::Debug for ExtractOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Debug output goes to logs; passwords must never appear there.
        f.debug_struct("ExtractOptions")
            .field("overwrite", &self.overwrite)
            .field("max_ratio", &self.max_ratio)
            .field("ratio_check_above", &self.ratio_check_above)
            .field("max_total", &self.max_total)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("threads", &self.threads)
            .field("memory_budget", &self.memory_budget)
            .field("only", &self.only)
            .finish()
    }
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            overwrite: Overwrite::Rename,
            max_ratio: 2000,
            ratio_check_above: 256 << 20,
            max_total: 1 << 40,
            password: None,
            threads: 0,
            memory_budget: 256 << 20,
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
    /// The top-level items written, under the names they ended up with, in archive order.
    pub roots: Vec<PathBuf>,
}

const BUF: usize = 256 * 1024;

/// Directory attributes applied after all children are written.
type DirMeta = (PathBuf, Option<u32>, Option<i64>);

trait EntryMetadata: Sync {
    fn apply_mode(&self, path: &Path, mode: u32) -> std::io::Result<()>;
    fn set_mtime(&self, path: &Path, time: i64) -> std::io::Result<()>;
}

struct PlatformMetadata;

impl EntryMetadata for PlatformMetadata {
    fn apply_mode(&self, path: &Path, mode: u32) -> std::io::Result<()> {
        apply_mode(path, mode)
    }
    fn set_mtime(&self, path: &Path, time: i64) -> std::io::Result<()> {
        set_mtime(path, time)
    }
}

struct Ctx<'a> {
    dest: &'a Path,
    opts: &'a ExtractOptions,
    tracker: &'a Tracker<'a>,
    metadata: &'a dyn EntryMetadata,
    /// Indexes of the entries chosen for selective extraction, if any.
    wanted: Option<HashSet<usize>>,
    /// Where entries go when their top-level name was taken, by entry index.
    remap: HashMap<usize, String>,
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
    extract_with_metadata(reader, dest, opts, cancel, progress, &PlatformMetadata)
}

fn extract_with_metadata(
    reader: &mut dyn ArchiveReader,
    dest: &Path,
    opts: &ExtractOptions,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
    metadata: &dyn EntryMetadata,
) -> Result<ExtractReport> {
    cancel.check()?;
    if let Some(pw) = &opts.password {
        reader.set_password(pw)?;
    }
    reader.set_read_control(cancel.clone(), opts.max_total);
    reader.set_ratio_limit(opts.max_ratio, opts.ratio_check_above);
    reader.set_decode_config(opts.threads, opts.memory_budget);
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
    let (created, remap) = if opts.overwrite == Overwrite::Rename {
        keep_both(&mut entries, dest)
    } else {
        Default::default()
    };
    let roots = top_level_roots(&entries, dest);
    let declared: u64 = entries.iter().map(|e| e.size).fold(0, u64::saturating_add);
    if declared > opts.max_total {
        return Err(Error::LimitExceeded(format!("total size {declared} bytes")));
    }
    // Check everything before writing anything, so a bad archive leaves no partial result.
    let duplicates = preflight(&entries, dest, opts, cancel)?;
    // A task that is paused or cancelled before it starts must not touch the disk,
    // not even to create the destination folder.
    cancel.check()?;
    fs::create_dir_all(dest)?;

    let tracker = Tracker::new(progress, cancel, entries.len() as u64, declared);
    let ctx = Ctx {
        dest,
        opts,
        tracker: &tracker,
        metadata,
        wanted,
        remap,
        files: AtomicU64::new(0),
        dirs: AtomicU64::new(0),
        skipped: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        dir_meta: Mutex::new(Vec::new()),
    };

    let file_count = entries.iter().filter(|e| e.kind == EntryKind::File).count();
    let threads = resolve_threads(opts.threads).min(file_count);
    let block_plan = if threads > 1 && !duplicates {
        reader.parallel_blocks(opts.memory_budget)?
    } else {
        None
    };
    let ran = if let Some(plan) = block_plan {
        match run_blocks(reader, &entries, &ctx, threads, plan) {
            Parallel::Done(res) => res,
            Parallel::Unavailable => run_sequential(reader, &ctx),
        }
    } else if reader.caps().random_access && threads > 1 && !duplicates {
        match run_parallel(reader, &entries, &ctx, threads) {
            Parallel::Done(res) => res,
            Parallel::Unavailable => run_sequential(reader, &ctx),
        }
    } else {
        run_sequential(reader, &ctx)
    };
    if matches!(ran, Err(Error::Password)) {
        // The user will try again with a password; what this run began would make that
        // attempt land beside it as `name (1)`.
        for path in &created {
            let _ = match fs::symlink_metadata(path) {
                Ok(m) if m.is_dir() => fs::remove_dir_all(path),
                Ok(_) => fs::remove_file(path),
                Err(_) => Ok(()),
            };
        }
    }
    ran?;

    // Deepest directories first, so setting a parent's mtime is not undone by its children.
    let dir_meta = ctx
        .dir_meta
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default();
    for (dir, mode, modified) in dir_meta.into_iter().rev() {
        // A folder swapped for a link after extraction must not receive metadata.
        if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        if let Some(t) = modified {
            // Directory times are best effort; some platforms cannot open a directory for writing.
            let _ = ctx.metadata.set_mtime(&dir, t);
        }
        if let Some(mode) = mode {
            ctx.metadata.apply_mode(&dir, mode | 0o700)?;
        }
    }
    tracker.finish();
    Ok(ExtractReport {
        files: ctx.files.load(Ordering::Relaxed),
        dirs: ctx.dirs.load(Ordering::Relaxed),
        skipped: ctx.skipped.load(Ordering::Relaxed),
        bytes: ctx.bytes.load(Ordering::Relaxed),
        roots,
    })
}

/// Gives every top-level item that already exists in `dest` a free numbered name, and moves
/// everything inside it along. Entries are opened by index, so their paths can be rewritten.
fn keep_both(entries: &mut [Entry], dest: &Path) -> (Vec<PathBuf>, HashMap<usize, String>) {
    let mut is_dir: HashMap<String, bool> = HashMap::new();
    for e in entries.iter() {
        let clean = clean_path(&e.path);
        let Some(top) = clean.split('/').next().filter(|t| !t.is_empty()) else {
            continue;
        };
        let nested = clean.len() > top.len() || e.kind == EntryKind::Dir;
        *is_dir.entry(top.to_string()).or_insert(false) |= nested;
    }
    let mut remap: HashMap<usize, String> = HashMap::new();
    let mut chosen: HashSet<String> = HashSet::new();
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut tops: Vec<_> = is_dir.into_iter().collect();
    tops.sort();
    for (top, dir) in tops {
        if top == ".." || dest.join(&top).symlink_metadata().is_err() {
            chosen.insert(top);
            continue;
        }
        let free = crate::naming::first_free(&top, dir, |n| {
            chosen.contains(n) || dest.join(n).symlink_metadata().is_ok()
        });
        chosen.insert(free.clone());
        renamed.insert(top, free);
    }
    for e in entries.iter_mut() {
        let clean = clean_path(&e.path);
        let (top, rest) = clean.split_once('/').unwrap_or((clean.as_str(), ""));
        if let Some(new) = renamed.get(top) {
            e.path = if rest.is_empty() {
                new.clone()
            } else {
                format!("{new}/{rest}")
            };
            remap.insert(e.index, e.path.clone());
        }
    }
    // Every top-level name is new now: either it was free, or it was moved to a free one.
    let mut created: Vec<PathBuf> = chosen.into_iter().map(|n| dest.join(n)).collect();
    created.sort();
    (created, remap)
}

/// The first path component of every entry, in order of appearance, joined to `dest`.
fn top_level_roots(entries: &[Entry], dest: &Path) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter_map(|e| {
            let clean = clean_path(&e.path);
            let top = clean
                .split('/')
                .next()
                .filter(|t| !t.is_empty())?
                .to_string();
            seen.insert(top.clone()).then(|| dest.join(top))
        })
        .collect()
}

/// Validates every path and the overwrite policy up front.
/// Returns whether two entries would land on the same file (name clash).
fn preflight(
    entries: &[Entry],
    dest: &Path,
    opts: &ExtractOptions,
    cancel: &CancelToken,
) -> Result<bool> {
    let mut seen = HashMap::new();
    let mut duplicates = false;
    let mut sensitive = None;
    for e in entries {
        cancel.check()?;
        let target = safe_join(dest, &e.path)?;
        if e.kind != EntryKind::File {
            continue;
        }
        if opts.overwrite == Overwrite::Error && target.symlink_metadata().is_ok() {
            return Err(already_exists(&e.path));
        }
        // Compare case-insensitively: on macOS and Windows these would collide.
        let normalized = clean_path(&e.path);
        if let Some(first) = seen.insert(normalized.to_lowercase(), normalized.clone()) {
            duplicates = true;
            if matches!(opts.overwrite, Overwrite::Error | Overwrite::Rename) && first != normalized
            {
                let case_sensitive = match sensitive {
                    Some(value) => value,
                    None => {
                        let value = rz_platform::is_case_sensitive(dest)?;
                        sensitive = Some(value);
                        value
                    }
                };
                if !case_sensitive {
                    return Err(Error::NameConflict {
                        first,
                        second: normalized,
                    });
                }
            }
        }
    }
    Ok(duplicates)
}

fn already_exists(entry: &str) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("destination already contains '{entry}'; choose an empty folder or change the overwrite setting"),
    ))
}

/// Everything below the destination must be written without following links,
/// so a symlink planted in the destination can never redirect writes outside.
fn link_escape(below_dest: &Path) -> Error {
    Error::UnsafePath(format!(
        "'{}' passes through a symbolic link; remove the link or choose another destination",
        below_dest.display()
    ))
}

fn not_a_folder(path: &Path) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("'{}' already exists and is not a folder", path.display()),
    ))
}

/// Creates every directory between `dest` and `parent` (both inclusive),
/// refusing to pass through a symbolic link at any level. Every level is
/// checked again on every entry: a folder created a moment ago may have been
/// swapped for a link in the meantime.
fn secure_mkdirs(dest: &Path, parent: &Path) -> Result<()> {
    let rel = parent
        .strip_prefix(dest)
        .map_err(|_| Error::UnsafePath(parent.display().to_string()))?;
    let mut cur = dest.to_path_buf();
    for comp in rel.components() {
        cur.push(comp.as_os_str());
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.is_dir() => {}
            Ok(m) if m.file_type().is_symlink() => return Err(link_escape(&cur)),
            Ok(_) => return Err(not_a_folder(&cur)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Err(race) = fs::create_dir(&cur) {
                    // Lost a race: fine if a real folder appeared, refused if a link did.
                    match fs::symlink_metadata(&cur) {
                        Ok(m) if m.is_dir() => {}
                        Ok(m) if m.file_type().is_symlink() => return Err(link_escape(&cur)),
                        _ => return Err(race.into()),
                    }
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn run_sequential(reader: &mut dyn ArchiveReader, ctx: &Ctx<'_>) -> Result<()> {
    let mut buf = vec![0u8; BUF];
    let selected = ctx
        .wanted
        .as_ref()
        .map(|wanted| wanted.iter().copied().collect::<Vec<_>>());
    reader.for_each_selected(selected.as_deref(), &mut |entry, data| {
        handle(ctx, entry, || Ok(&mut *data), &mut buf)
    })
}

enum Parallel {
    Done(Result<()>),
    /// The format cannot open independent readers; fall back to one pass.
    Unavailable,
}

fn run_blocks(
    reader: &dyn ArchiveReader,
    entries: &[Entry],
    ctx: &Ctx<'_>,
    threads: usize,
    plan: ParallelBlocks,
) -> Parallel {
    let selected: HashSet<usize> = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .map(|entry| entry.index)
        .collect();
    let mapped: HashSet<usize> = plan.entries.iter().flatten().copied().collect();
    let groups: Vec<usize> = plan
        .entries
        .iter()
        .enumerate()
        .filter(|(_, group)| group.iter().any(|index| selected.contains(index)))
        .map(|(index, _)| index)
        .collect();
    let workers = threads.min(plan.max_workers).min(groups.len());
    if workers < 2 {
        return Parallel::Unavailable;
    }
    let mut readers = Vec::with_capacity(workers);
    for _ in 0..workers {
        match reader.reopen() {
            Some(Ok(reader)) => readers.push(reader),
            Some(Err(error)) => return Parallel::Done(Err(error)),
            None => return Parallel::Unavailable,
        }
    }
    let mut buf = vec![0u8; BUF];
    for entry in entries {
        if entry.kind != EntryKind::File || !mapped.contains(&entry.index) {
            if let Err(error) = handle(ctx, entry, || Ok(std::io::empty()), &mut buf) {
                return Parallel::Done(Err(error));
            }
        }
    }
    let failure = Mutex::new(None);
    let abort = AtomicBool::new(false);
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for mut reader in readers {
            let (failure, abort, next, groups, selected, mapped) =
                (&failure, &abort, &next, &groups, &selected, &mapped);
            scope.spawn(move || {
                let mut buf = vec![0u8; BUF];
                loop {
                    if abort.load(Ordering::Acquire) {
                        break;
                    }
                    let Some(&block) = groups.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let result = reader.for_each_block(block, &mut |entry, data| {
                        ctx.tracker.check()?;
                        if abort.load(Ordering::Acquire) {
                            return Err(Error::Cancelled);
                        }
                        if entry.kind == EntryKind::File
                            && selected.contains(&entry.index)
                            && mapped.contains(&entry.index)
                        {
                            handle(ctx, entry, || Ok(&mut *data), &mut buf)?;
                        }
                        loop {
                            ctx.tracker.check()?;
                            if abort.load(Ordering::Acquire) {
                                return Err(Error::Cancelled);
                            }
                            if data.read(&mut buf)? == 0 {
                                break;
                            }
                        }
                        Ok(())
                    });
                    if let Err(error) = result {
                        if let Ok(mut first) = failure.lock() {
                            first.get_or_insert(error);
                        }
                        abort.store(true, Ordering::Release);
                        break;
                    }
                }
            });
        }
    });
    let error = failure.lock().ok().and_then(|mut first| first.take());
    Parallel::Done(error.map_or(Ok(()), Err))
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
    let target = safe_join(ctx.dest, ctx.remap.get(&entry.index).unwrap_or(&entry.path))?;
    match entry.kind {
        EntryKind::Dir => {
            if let Some(parent) = target.parent() {
                secure_mkdirs(ctx.dest, parent)?;
            }
            match fs::symlink_metadata(&target) {
                Ok(m) if m.is_dir() => {}
                Ok(m) if m.file_type().is_symlink() => return Err(link_escape(&target)),
                Ok(_) => return Err(not_a_folder(&target)),
                Err(_) => fs::create_dir(&target).or_else(|e| {
                    // Another worker may have created it first; a link still refuses.
                    if e.kind() == std::io::ErrorKind::AlreadyExists
                        && fs::symlink_metadata(&target).is_ok_and(|m| m.is_dir())
                    {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })?,
            }
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
            if let Some(parent) = target.parent() {
                secure_mkdirs(ctx.dest, parent)?;
            }
            // A header that understates the size must not become an unbounded write.
            let limit = entry.size;
            let mut src = src()?;
            let existing = target.symlink_metadata();
            // Never write through a link, and never turn a folder into a file.
            if let Ok(m) = &existing {
                if m.file_type().is_symlink() {
                    return Err(link_escape(&target));
                }
                if m.is_dir() {
                    return Err(not_a_folder(&target));
                }
            }
            let replacing = existing.is_ok() && ctx.opts.overwrite == Overwrite::Replace;
            if replacing {
                // Stage beside the target and swap atomically, so a failed, short or
                // cancelled copy can never leave a truncated file in place of the old one.
                let name = target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let temp = target.with_file_name(format!(".{name}.rz-{}.part", entry.index));
                let mut out = match OpenOptions::new().write(true).create_new(true).open(&temp) {
                    Ok(f) => f,
                    Err(e) => return Err(Error::Io(e)),
                };
                let copied = copy_limited(&mut src, &mut out, buf, limit, &entry.path, ctx);
                drop(out);
                if let Err(e) = copied {
                    let _ = fs::remove_file(&temp);
                    return Err(e);
                }
                // Metadata is part of the staged file so a metadata failure does
                // not occur after the old destination has crossed the commit point.
                if let Some(t) = entry.modified {
                    if let Err(e) = ctx.metadata.set_mtime(&temp, t) {
                        let _ = fs::remove_file(&temp);
                        return Err(Error::Io(e));
                    }
                }
                if let Some(mode) = entry.unix_mode {
                    if let Err(e) = ctx.metadata.apply_mode(&temp, mode) {
                        let _ = fs::remove_file(&temp);
                        return Err(Error::Io(e));
                    }
                }
                if let Err(e) = fs::rename(&temp, &target) {
                    let _ = fs::remove_file(&temp);
                    return Err(e.into());
                }
            } else {
                // `create_new` refuses anything that exists — including a link swapped
                // in after the check above — so this path can never follow a link.
                let mut out = match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                {
                    Ok(f) => f,
                    // Appeared after the skip check: skipping is the policy, not failing.
                    Err(e)
                        if e.kind() == std::io::ErrorKind::AlreadyExists
                            && ctx.opts.overwrite == Overwrite::Skip =>
                    {
                        ctx.skipped.fetch_add(1, Ordering::Relaxed);
                        ctx.tracker.add_bytes(entry.size);
                        ctx.tracker.file_done();
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(match e.kind() {
                            std::io::ErrorKind::AlreadyExists => already_exists(&entry.path),
                            _ => Error::Io(e),
                        })
                    }
                };
                let copied = copy_limited(&mut src, &mut out, buf, limit, &entry.path, ctx);
                drop(out);
                if let Err(e) = copied {
                    let _ = fs::remove_file(&target); // do not leave a truncated file behind
                    return Err(e);
                }
            }
            // Metadata only when the path is still the file this run wrote; a path
            // swapped for a link meanwhile must not receive mode or time changes.
            if !replacing && fs::symlink_metadata(&target).is_ok_and(|m| m.is_file()) {
                if let Some(t) = entry.modified {
                    if let Err(e) = ctx.metadata.set_mtime(&target, t) {
                        let _ = fs::remove_file(&target);
                        return Err(Error::Io(e));
                    }
                }
                if let Some(mode) = entry.unix_mode {
                    if let Err(e) = ctx.metadata.apply_mode(&target, mode) {
                        let _ = fs::remove_file(&target);
                        return Err(Error::Io(e));
                    }
                }
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
        let n = match src.read(buf) {
            Ok(n) => n,
            Err(error) => {
                ctx.tracker.check()?;
                return Err(error.into());
            }
        };
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use rz_core::NoProgress;

    struct RejectTimestamp;

    impl EntryMetadata for RejectTimestamp {
        fn apply_mode(&self, path: &Path, mode: u32) -> std::io::Result<()> {
            apply_mode(path, mode)
        }

        fn set_mtime(&self, _: &Path, _: i64) -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected metadata failure",
            ))
        }
    }

    #[test]
    fn replace_metadata_failure_keeps_the_previous_file() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("source.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let modified = zip::DateTime::from_date_and_time(2024, 1, 2, 3, 4, 6).unwrap();
        let options = zip::write::SimpleFileOptions::default().last_modified_time(modified);
        writer.start_file("preserve.txt", options).unwrap();
        writer.write_all(b"new complete contents").unwrap();
        writer.finish().unwrap();

        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap();
        let original = b"existing user contents";
        fs::write(dest.join("preserve.txt"), original).unwrap();
        let mut reader = rz_formats::open(&archive).unwrap();
        let options = ExtractOptions {
            overwrite: Overwrite::Replace,
            threads: 1,
            ..ExtractOptions::default()
        };
        let result = extract_with_metadata(
            &mut *reader,
            &dest,
            &options,
            &CancelToken::new(),
            &mut NoProgress,
            &RejectTimestamp,
        );
        assert!(
            matches!(result, Err(Error::Io(ref e)) if e.kind() == std::io::ErrorKind::PermissionDenied)
        );
        assert_eq!(fs::read(dest.join("preserve.txt")).unwrap(), original);
        assert_eq!(
            fs::read_dir(&dest).unwrap().count(),
            1,
            "staged replacement survived metadata failure"
        );
    }

    #[cfg(unix)]
    #[test]
    fn replace_read_only_file_sets_time_before_final_permissions() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("readonly.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let modified = zip::DateTime::from_date_and_time(2024, 1, 2, 3, 4, 6).unwrap();
        let options = zip::write::SimpleFileOptions::default()
            .unix_permissions(0o444)
            .last_modified_time(modified);
        writer.start_file("readonly.txt", options).unwrap();
        writer.write_all(b"new contents").unwrap();
        writer.finish().unwrap();
        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("readonly.txt"), b"old contents").unwrap();
        let mut reader = rz_formats::open(&archive).unwrap();
        let options = ExtractOptions {
            overwrite: Overwrite::Replace,
            threads: 1,
            ..ExtractOptions::default()
        };
        let report = extract(
            &mut *reader,
            &dest,
            &options,
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        let output = dest.join("readonly.txt");
        assert_eq!(report.files, 1);
        assert_eq!(fs::read(&output).unwrap(), b"new contents");
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o444
        );
        assert_eq!(
            rz_platform::mtime_of(&fs::metadata(&output).unwrap()),
            Some(1_704_164_646)
        );
    }
}
