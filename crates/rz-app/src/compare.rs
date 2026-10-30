//! Content comparison for two archives. File data is hashed as bounded streams;
//! the archives are never extracted to disk.

use rz_core::{
    clean_path, ArchiveReader, CancelToken, Entry, EntryKind, Error, Progress, ProgressSink, Result,
};
use rz_engine::ExtractOptions;
use std::collections::BTreeMap;
use std::path::Path;
use zeroize::Zeroizing;

const MAX_ENTRIES: usize = 1_000_000;
const BUFFER_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifferenceKind {
    Added,
    Removed,
    Changed,
    TypeChanged,
    NotCompared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub path: String,
    pub kind: DifferenceKind,
    pub left_size: Option<u64>,
    pub right_size: Option<u64>,
}

#[derive(Clone, Default)]
pub struct CompareOptions {
    pub left_password: Option<Zeroizing<String>>,
    pub right_password: Option<Zeroizing<String>>,
    pub encoding: Option<String>,
    /// Uses the same output and decoder safety limits as extraction.
    pub limits: ExtractOptions,
}

impl std::fmt::Debug for CompareOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompareOptions")
            .field(
                "left_password",
                &self.left_password.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "right_password",
                &self.right_password.as_ref().map(|_| "<redacted>"),
            )
            .field("encoding", &self.encoding)
            .field("limits", &self.limits)
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompareCounts {
    pub added: u64,
    pub removed: u64,
    pub changed: u64,
    pub type_changed: u64,
    pub not_compared: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareReport {
    /// File names only; absolute paths are kept out of task history and reports.
    pub left_name: String,
    pub right_name: String,
    pub left_format: String,
    pub right_format: String,
    /// Only entries that differ or could not be compared are returned.
    pub differences: Vec<Difference>,
    pub counts: CompareCounts,
    pub bytes_hashed: u64,
}

#[derive(Debug, Clone)]
struct Snapshot {
    kind: EntryKind,
    size: u64,
    compare_content: bool,
    hash: Option<blake3::Hash>,
}

type SnapshotMap = BTreeMap<String, Snapshot>;

/// Compares archive paths, entry kinds, and file contents without extracting files.
///
/// Symbolic link targets are not exposed by the reader API, so matching symlinks
/// are reported as `NotCompared` instead of being presented as identical.
pub fn compare_archives(
    left_path: &Path,
    right_path: &Path,
    options: CompareOptions,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<CompareReport> {
    if same_file(left_path, right_path)? {
        return Err(Error::Unsupported(
            "choose two different archive files to compare".into(),
        ));
    }

    let limits = options.limits;
    let mut left = rz_formats::open(left_path)?;
    let mut right = rz_formats::open(right_path)?;
    for reader in [&mut left, &mut right] {
        reader.set_read_control(cancel.clone(), limits.max_total);
        reader.set_ratio_limit(limits.max_ratio, limits.ratio_check_above);
        reader.set_decode_config(limits.threads, limits.memory_budget);
        if let Some(encoding) = options.encoding.as_deref() {
            reader.set_encoding(encoding)?;
        }
    }
    if let Some(password) = options.left_password.as_deref() {
        left.set_password(password)?;
    }
    if let Some(password) = options.right_password.as_deref() {
        right.set_password(password)?;
    }

    let left_format = left.format_name().to_string();
    let right_format = right.format_name().to_string();
    let mut no_progress = rz_core::NoProgress;
    let left_entries = left.entries_with_progress(cancel, &mut no_progress)?;
    let right_entries = right.entries_with_progress(cancel, &mut no_progress)?;
    let mut left_map = snapshots(&left_entries)?;
    let mut right_map = snapshots(&right_entries)?;

    let mut bytes_total = 0u64;
    let mut candidate_count = 0usize;
    for (path, left_entry) in &mut left_map {
        if let Some(right_entry) = right_map.get_mut(path) {
            if left_entry.kind == EntryKind::File
                && right_entry.kind == EntryKind::File
                && left_entry.size == right_entry.size
            {
                left_entry.compare_content = true;
                right_entry.compare_content = true;
                bytes_total = bytes_total.saturating_add(left_entry.size.saturating_mul(2));
                candidate_count += 1;
            }
        }
    }
    let entry_total = left_entries.len().saturating_add(right_entries.len()) as u64;
    let (mut files_done, mut bytes_done) = (0u64, 0u64);
    let mut buffer = vec![0; BUFFER_SIZE];

    if candidate_count > 0 {
        hash_candidates(
            &mut *left,
            &mut left_map,
            cancel,
            &mut buffer,
            &mut files_done,
            entry_total,
            limits.max_total,
            limits.max_ratio,
            limits.ratio_check_above,
            &mut bytes_done,
            bytes_total,
            progress,
        )?;
        hash_candidates(
            &mut *right,
            &mut right_map,
            cancel,
            &mut buffer,
            &mut files_done,
            entry_total,
            limits.max_total,
            limits.max_ratio,
            limits.ratio_check_above,
            &mut bytes_done,
            bytes_total,
            progress,
        )?;
    }

    let mut differences = Vec::new();
    let mut counts = CompareCounts::default();
    let mut left_iter = left_map.iter().peekable();
    let mut right_iter = right_map.iter().peekable();
    while left_iter.peek().is_some() || right_iter.peek().is_some() {
        cancel.check()?;
        let (path, left, right) = match (left_iter.peek(), right_iter.peek()) {
            (Some((left_path, _)), Some((right_path, _))) => match left_path.cmp(right_path) {
                std::cmp::Ordering::Less => {
                    let Some((path, entry)) = left_iter.next() else {
                        break;
                    };
                    (path.as_str(), Some(entry), None)
                }
                std::cmp::Ordering::Greater => {
                    let Some((path, entry)) = right_iter.next() else {
                        break;
                    };
                    (path.as_str(), None, Some(entry))
                }
                std::cmp::Ordering::Equal => {
                    let Some((path, left)) = left_iter.next() else {
                        break;
                    };
                    let Some((_, right)) = right_iter.next() else {
                        break;
                    };
                    (path.as_str(), Some(left), Some(right))
                }
            },
            (Some(_), None) => {
                let Some((path, entry)) = left_iter.next() else {
                    break;
                };
                (path.as_str(), Some(entry), None)
            }
            (None, Some(_)) => {
                let Some((path, entry)) = right_iter.next() else {
                    break;
                };
                (path.as_str(), None, Some(entry))
            }
            (None, None) => break,
        };
        let kind = match (left, right) {
            (None, Some(_)) => Some(DifferenceKind::Added),
            (Some(_), None) => Some(DifferenceKind::Removed),
            (Some(left), Some(right)) if left.kind != right.kind => {
                Some(DifferenceKind::TypeChanged)
            }
            (Some(left), Some(right)) if left.kind == EntryKind::File => {
                if left.size != right.size || left.hash != right.hash {
                    Some(DifferenceKind::Changed)
                } else {
                    None
                }
            }
            (Some(left), Some(_)) if left.kind == EntryKind::Symlink => {
                Some(DifferenceKind::NotCompared)
            }
            _ => None,
        };
        if let Some(kind) = kind {
            match kind {
                DifferenceKind::Added => counts.added += 1,
                DifferenceKind::Removed => counts.removed += 1,
                DifferenceKind::Changed => counts.changed += 1,
                DifferenceKind::TypeChanged => counts.type_changed += 1,
                DifferenceKind::NotCompared => counts.not_compared += 1,
            }
            differences.push(Difference {
                path: path.to_string(),
                kind,
                left_size: left.map(|entry| entry.size),
                right_size: right.map(|entry| entry.size),
            });
        }
    }
    progress.update(Progress {
        files_done: entry_total,
        files_total: entry_total,
        bytes_done,
        bytes_total,
    });

    Ok(CompareReport {
        left_name: archive_name(left_path),
        right_name: archive_name(right_path),
        left_format,
        right_format,
        differences,
        counts,
        bytes_hashed: bytes_done,
    })
}

fn archive_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".into())
}

fn same_file(left: &Path, right: &Path) -> Result<bool> {
    let left = std::fs::canonicalize(left)?;
    let right = std::fs::canonicalize(right)?;
    Ok(left == right)
}

fn snapshots(entries: &[Entry]) -> Result<SnapshotMap> {
    if entries.len() > MAX_ENTRIES {
        return Err(Error::LimitExceeded(format!(
            "comparison supports at most {MAX_ENTRIES} entries per archive"
        )));
    }
    let mut snapshots = SnapshotMap::new();
    for entry in entries {
        let path = clean_path(&entry.path);
        if path.is_empty() {
            continue;
        }
        let mut parent = path.as_str();
        while let Some((directory, _)) = parent.rsplit_once('/') {
            if directory.is_empty() {
                break;
            }
            match snapshots.get(directory) {
                Some(existing) if existing.kind != EntryKind::Dir => {
                    return Err(Error::Corrupt(format!(
                        "file conflicts with a parent directory: {directory}"
                    )));
                }
                Some(_) => {}
                None => {
                    ensure_entry_capacity(snapshots.len())?;
                    snapshots.insert(
                        directory.to_string(),
                        Snapshot {
                            kind: EntryKind::Dir,
                            size: 0,
                            compare_content: false,
                            hash: None,
                        },
                    );
                }
            }
            parent = directory;
        }

        match snapshots.get(&path) {
            Some(existing) if existing.kind == EntryKind::Dir && entry.kind == EntryKind::Dir => {}
            Some(_) => return Err(Error::Corrupt(format!("duplicate archive path: {path}"))),
            None => {
                ensure_entry_capacity(snapshots.len())?;
                snapshots.insert(
                    path,
                    Snapshot {
                        kind: entry.kind,
                        size: entry.size,
                        compare_content: false,
                        hash: None,
                    },
                );
            }
        }
    }
    Ok(snapshots)
}

fn ensure_entry_capacity(entries: usize) -> Result<()> {
    if entries >= MAX_ENTRIES {
        return Err(Error::LimitExceeded(format!(
            "comparison supports at most {MAX_ENTRIES} paths per archive"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn hash_candidates(
    reader: &mut dyn ArchiveReader,
    snapshots: &mut SnapshotMap,
    cancel: &CancelToken,
    buffer: &mut [u8],
    files_done: &mut u64,
    files_total: u64,
    max_total: u64,
    max_ratio: u64,
    ratio_check_above: u64,
    bytes_done: &mut u64,
    bytes_total: u64,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    reader.for_each(&mut |entry, data| {
        cancel.check()?;
        *files_done = files_done.saturating_add(1);
        if entry.kind == EntryKind::File {
            let path = clean_path(&entry.path);
            if snapshots
                .get(&path)
                .is_some_and(|snapshot| snapshot.compare_content)
            {
                let expected = snapshots
                    .get(&path)
                    .map(|snapshot| snapshot.size)
                    .ok_or_else(|| Error::Corrupt(format!("missing archive path: {path}")))?;
                if expected > ratio_check_above {
                    if let Some(packed) = entry.packed_size {
                        if packed == 0
                            || u128::from(expected)
                                > u128::from(packed).saturating_mul(u128::from(max_ratio))
                        {
                            return Err(Error::LimitExceeded(format!(
                                "compression ratio limit for {path}"
                            )));
                        }
                    }
                }
                let mut hasher = blake3::Hasher::new();
                let mut read_size = 0u64;
                let mut archive_bytes = 0u64;
                loop {
                    cancel.check()?;
                    let count = data.read(buffer).map_err(|error| {
                        if cancel.is_cancelled() {
                            Error::Cancelled
                        } else {
                            Error::Corrupt(format!("{path}: {error}"))
                        }
                    })?;
                    if count == 0 {
                        break;
                    }
                    read_size = read_size.saturating_add(count as u64);
                    if read_size > expected {
                        return Err(Error::Corrupt(format!(
                            "entry exceeds its declared size: {path}"
                        )));
                    }
                    archive_bytes = archive_bytes.saturating_add(count as u64);
                    if archive_bytes > max_total {
                        return Err(Error::LimitExceeded(
                            "comparison output size exceeds the configured total".into(),
                        ));
                    }
                    *bytes_done = bytes_done.saturating_add(count as u64);
                    hasher.update(&buffer[..count]);
                    progress.update(Progress {
                        files_done: *files_done,
                        files_total,
                        bytes_done: *bytes_done,
                        bytes_total,
                    });
                }
                if read_size != expected {
                    return Err(Error::Corrupt(format!(
                        "entry size does not match its declaration: {path}"
                    )));
                }
                if let Some(snapshot) = snapshots.get_mut(&path) {
                    snapshot.hash = Some(hasher.finalize());
                }
            }
        }
        Ok(())
    })?;
    cancel.check()?;
    if let Some((path, _)) = snapshots
        .iter()
        .find(|(_, snapshot)| snapshot.compare_content && snapshot.hash.is_none())
    {
        return Err(Error::Corrupt(format!(
            "reader did not provide file data: {path}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::fs::File;
    #[cfg(feature = "rar")]
    use std::io::Read;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, data) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn compares_paths_and_streamed_file_contents() {
        let temp = tempfile::tempdir().unwrap();
        let left = temp.path().join("left.zip");
        let right = temp.path().join("right.zip");
        make_zip(
            &left,
            &[
                ("same.txt", b"same"),
                ("changed.txt", b"left"),
                ("removed.txt", b"gone"),
                ("folder/same.txt", b"nested"),
            ],
        );
        make_zip(
            &right,
            &[
                ("same.txt", b"same"),
                ("changed.txt", b"rght"),
                ("added.txt", b"new"),
                ("folder/same.txt", b"nested"),
            ],
        );

        let report = compare_archives(
            &left,
            &right,
            CompareOptions::default(),
            &CancelToken::new(),
            &mut rz_core::NoProgress,
        )
        .unwrap();

        assert_eq!(report.counts.added, 1);
        assert_eq!(report.counts.removed, 1);
        assert_eq!(report.counts.changed, 1);
        assert_eq!(report.differences.len(), 3);
        assert_eq!(report.bytes_hashed, 28);
        assert!(report
            .differences
            .iter()
            .any(|diff| diff.path == "changed.txt" && diff.kind == DifferenceKind::Changed));
        assert!(!report
            .differences
            .iter()
            .any(|diff| diff.path == "same.txt"));
        assert!(!report.differences.iter().any(|diff| diff.path == "folder"));
    }

    #[cfg(feature = "rar")]
    #[test]
    fn compares_rar_entry_contents_against_a_zip_archive() {
        let temp = tempfile::tempdir().unwrap();
        let rar = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/rz-formats/tests/fixtures/container.rar");
        let mut reader = rz_formats::open(&rar).unwrap();
        let entries = reader.entries().unwrap();
        let file = entries
            .iter()
            .find(|entry| entry.kind == EntryKind::File)
            .unwrap();
        let mut contents = Vec::new();
        reader
            .open(file)
            .unwrap()
            .read_to_end(&mut contents)
            .unwrap();

        let zip = temp.path().join("same-content.zip");
        make_zip(&zip, &[(file.path.as_str(), &contents)]);
        let report = compare_archives(
            &rar,
            &zip,
            CompareOptions::default(),
            &CancelToken::new(),
            &mut rz_core::NoProgress,
        )
        .unwrap();

        assert_eq!(report.counts, CompareCounts::default());
        assert!(report.differences.is_empty());
        assert!(report.bytes_hashed > 0);
    }

    #[test]
    fn compares_long_tail_reader_copies() {
        let temp = tempfile::tempdir().unwrap();
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/rz-formats/tests/fixtures");
        for name in [
            "chm_basic.chm",
            "longtail_iso9660.iso.gz",
            "longtail_lha_lh0.lzh",
            "longtail_rpm_bzip2.rpm",
            "longtail_rpm_gzip.rpm",
            "longtail_xar.xar",
        ] {
            let left = fixtures.join(name);
            let right = temp.path().join(name);
            fs::copy(&left, &right).unwrap();
            let report = compare_archives(
                &left,
                &right,
                CompareOptions::default(),
                &CancelToken::new(),
                &mut rz_core::NoProgress,
            )
            .unwrap();
            let expected_not_compared = if name == "longtail_lha_lh0.lzh" { 2 } else { 0 };
            assert_eq!(report.counts.added, 0, "{name}");
            assert_eq!(report.counts.removed, 0, "{name}");
            assert_eq!(report.counts.changed, 0, "{name}");
            assert_eq!(report.counts.type_changed, 0, "{name}");
            assert_eq!(report.counts.not_compared, expected_not_compared, "{name}");
            assert!(
                report
                    .differences
                    .iter()
                    .all(|diff| diff.kind == DifferenceKind::NotCompared),
                "{name}"
            );
            assert!(report.bytes_hashed > 0, "{name} did not stream any content");
        }
    }

    #[test]
    fn reports_file_to_directory_changes_and_implicit_parent_paths() {
        let temp = tempfile::tempdir().unwrap();
        let left = temp.path().join("left.zip");
        let right = temp.path().join("right.zip");
        make_zip(&left, &[("node", b"file")]);
        let mut writer = zip::ZipWriter::new(File::create(&right).unwrap());
        writer
            .add_directory("node/", SimpleFileOptions::default())
            .unwrap();
        writer
            .start_file("node/child.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"child").unwrap();
        writer.finish().unwrap();

        let report = compare_archives(
            &left,
            &right,
            CompareOptions::default(),
            &CancelToken::new(),
            &mut rz_core::NoProgress,
        )
        .unwrap();

        assert_eq!(report.counts.type_changed, 1);
        assert_eq!(report.counts.added, 1);
        assert!(report.differences.iter().any(|difference| {
            difference.path == "node" && difference.kind == DifferenceKind::TypeChanged
        }));
        assert!(report.differences.iter().any(|difference| {
            difference.path == "node/child.txt" && difference.kind == DifferenceKind::Added
        }));
    }

    #[test]
    fn comparison_uses_configured_output_and_ratio_limits() {
        let temp = tempfile::tempdir().unwrap();
        let left = temp.path().join("left.zip");
        let right = temp.path().join("right.zip");
        make_zip(&left, &[("same.txt", b"four")]);
        make_zip(&right, &[("same.txt", b"four")]);
        let limits = ExtractOptions {
            max_total: 3,
            ..ExtractOptions::default()
        };
        let result = compare_archives(
            &left,
            &right,
            CompareOptions {
                limits,
                ..CompareOptions::default()
            },
            &CancelToken::new(),
            &mut rz_core::NoProgress,
        );
        assert!(matches!(result, Err(Error::LimitExceeded(_))));

        let compressed = temp.path().join("compressed.zip");
        let mut writer = zip::ZipWriter::new(File::create(&compressed).unwrap());
        let data = vec![b'a'; 16 * 1024];
        writer
            .start_file(
                "same.txt",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();
        let other = temp.path().join("other.zip");
        fs::copy(&compressed, &other).unwrap();
        let result = compare_archives(
            &compressed,
            &other,
            CompareOptions {
                limits: ExtractOptions {
                    max_ratio: 1,
                    ratio_check_above: 0,
                    ..ExtractOptions::default()
                },
                ..CompareOptions::default()
            },
            &CancelToken::new(),
            &mut rz_core::NoProgress,
        );
        assert!(matches!(result, Err(Error::LimitExceeded(_))));
    }

    #[test]
    fn cancellation_during_entry_scan_stops_comparison() {
        let temp = tempfile::tempdir().unwrap();
        let left = temp.path().join("left.zip");
        let right = temp.path().join("right.zip");
        make_zip(&left, &[("same.txt", b"same")]);
        make_zip(&right, &[("same.txt", b"same")]);
        let cancel = CancelToken::new();
        let cancel_progress = cancel.clone();

        let result = compare_archives(
            &left,
            &right,
            CompareOptions::default(),
            &cancel,
            &mut move |_| cancel_progress.cancel(),
        );

        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn rejects_comparing_the_same_archive_twice() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("same.zip");
        make_zip(&archive, &[("same.txt", b"same")]);

        let result = compare_archives(
            &archive,
            &archive,
            CompareOptions::default(),
            &CancelToken::new(),
            &mut rz_core::NoProgress,
        );

        assert!(matches!(result, Err(Error::Unsupported(_))));
    }

    #[test]
    fn compare_options_redact_passwords_from_debug_output() {
        let options = CompareOptions {
            left_password: Some(Zeroizing::new("left-secret".into())),
            right_password: Some(Zeroizing::new("right-secret".into())),
            encoding: None,
            limits: ExtractOptions::default(),
        };
        let debug = format!("{options:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("left-secret"));
        assert!(!debug.contains("right-secret"));
    }
}
