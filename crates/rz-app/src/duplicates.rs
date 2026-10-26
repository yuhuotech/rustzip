//! Content-based duplicate scanning for the GUI compression preflight.

use rz_core::{CancelToken, Error, Progress, ProgressSink, Result};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_SCAN_FILES: usize = 100_000;
const BUFFER_SIZE: usize = 256 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// Local paths retained only while the prompt is open; formatting never prints them.
    pub paths: Vec<PathBuf>,
    pub size: u64,
}

impl std::fmt::Debug for DuplicateGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuplicateGroup")
            .field("file_count", &self.paths.len())
            .field("size", &self.size)
            .finish()
    }
}

impl DuplicateGroup {
    pub fn duplicate_count(&self) -> usize {
        self.paths.len().saturating_sub(1)
    }
}

/// Finds equal-size regular files with matching BLAKE3 contents in selected roots.
///
/// Symlinks and special files are ignored. Scanning is bounded to 100,000 regular
/// files and can be cancelled while walking directories or hashing file contents.
pub fn find_duplicates(
    roots: &[PathBuf],
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<Vec<DuplicateGroup>> {
    let mut by_size = BTreeMap::<u64, Vec<PathBuf>>::new();
    let mut scanned = 0usize;
    let mut walk_update = Instant::now();
    for root in roots {
        collect_files(
            root,
            &mut by_size,
            &mut scanned,
            &mut walk_update,
            cancel,
            progress,
        )?;
    }

    let candidates: Vec<(u64, PathBuf)> = by_size
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .flat_map(|(size, paths)| paths.into_iter().map(move |path| (size, path)))
        .collect();
    let bytes_total = candidates
        .iter()
        .fold(0u64, |total, (size, _)| total.saturating_add(*size));
    let files_total = candidates.len() as u64;
    let mut bytes_done = 0u64;
    let mut files_done = 0u64;
    let mut last_update = Instant::now();
    let mut by_hash = HashMap::<(u64, [u8; 32]), Vec<PathBuf>>::new();

    for (size, path) in candidates {
        cancel.check()?;
        let hash = hash_path(
            &path,
            cancel,
            &mut bytes_done,
            bytes_total,
            files_done,
            files_total,
            &mut last_update,
            progress,
        )?;
        by_hash.entry((size, hash)).or_default().push(path);
        files_done = files_done.saturating_add(1);
        progress.update(Progress {
            files_done,
            files_total,
            bytes_done,
            bytes_total,
        });
        last_update = Instant::now();
    }

    if files_total == 0 {
        progress.update(Progress {
            files_done: scanned as u64,
            files_total: scanned as u64,
            bytes_done: 0,
            bytes_total: 0,
        });
    }

    let mut groups: Vec<_> = by_hash
        .into_iter()
        .filter_map(|((size, _), mut paths)| {
            (paths.len() > 1).then(|| {
                paths.sort();
                DuplicateGroup { paths, size }
            })
        })
        .collect();
    groups.sort_by(|left, right| left.paths[0].cmp(&right.paths[0]));
    Ok(groups)
}

fn collect_files(
    path: &Path,
    by_size: &mut BTreeMap<u64, Vec<PathBuf>>,
    scanned: &mut usize,
    last_update: &mut Instant,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    cancel.check()?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        *scanned = scanned.saturating_add(1);
        if *scanned > MAX_SCAN_FILES {
            return Err(Error::LimitExceeded(
                "duplicate scan exceeds the 100,000 file limit".into(),
            ));
        }
        by_size
            .entry(metadata.len())
            .or_default()
            .push(path.to_path_buf());
        report_walk_progress(*scanned, last_update, progress);
        return Ok(());
    }
    if metadata.is_dir() {
        for child in fs::read_dir(path)? {
            cancel.check()?;
            collect_files(
                &child?.path(),
                by_size,
                scanned,
                last_update,
                cancel,
                progress,
            )?;
        }
    }
    Ok(())
}

fn report_walk_progress(
    scanned: usize,
    last_update: &mut Instant,
    progress: &mut dyn ProgressSink,
) {
    if last_update.elapsed() >= PROGRESS_INTERVAL {
        progress.update(Progress {
            files_done: scanned as u64,
            files_total: 0,
            bytes_done: 0,
            bytes_total: 0,
        });
        *last_update = Instant::now();
    }
}

#[allow(clippy::too_many_arguments)]
fn hash_path(
    path: &Path,
    cancel: &CancelToken,
    bytes_done: &mut u64,
    bytes_total: u64,
    files_done: u64,
    files_total: u64,
    last_update: &mut Instant,
    progress: &mut dyn ProgressSink,
) -> Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; BUFFER_SIZE];
    loop {
        cancel.check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        *bytes_done = bytes_done.saturating_add(count as u64);
        if last_update.elapsed() >= PROGRESS_INTERVAL {
            progress.update(Progress {
                files_done,
                files_total,
                bytes_done: *bytes_done,
                bytes_total,
            });
            *last_update = Instant::now();
        }
    }
    Ok(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use rz_core::NoProgress;

    #[test]
    fn finds_duplicates_by_content_and_ignores_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("one")).unwrap();
        fs::create_dir_all(root.join("two")).unwrap();
        fs::write(root.join("one/a.txt"), b"same").unwrap();
        fs::write(root.join("two/b.txt"), b"same").unwrap();
        fs::write(root.join("two/different.txt"), b"else").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("one/a.txt"), root.join("alias.txt")).unwrap();

        let groups = find_duplicates(&[root], &CancelToken::new(), &mut NoProgress).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].size, 4);
        assert_eq!(groups[0].duplicate_count(), 1);
        assert_eq!(groups[0].paths.len(), 2);
        assert!(!format!("{:?}", groups[0]).contains(temp.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn cancellation_and_file_limit_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        fs::write(&path, b"x").unwrap();
        let cancelled = CancelToken::new();
        cancelled.cancel();
        assert!(matches!(
            find_duplicates(std::slice::from_ref(&path), &cancelled, &mut NoProgress),
            Err(Error::Cancelled)
        ));

        let mut count = MAX_SCAN_FILES;
        let mut updated = Instant::now();
        assert!(matches!(
            collect_files(
                &path,
                &mut BTreeMap::new(),
                &mut count,
                &mut updated,
                &CancelToken::new(),
                &mut NoProgress
            ),
            Err(Error::LimitExceeded(_))
        ));
    }
}
