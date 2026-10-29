//! Synchronous archive operations for front ends that provide their own event loop.

use crate::settings::{OnConflict, Settings};
use rz_core::{
    CancelToken, CreateOptions, Entry, EntryKind, Error, Progress, ProgressSink, Result,
};
use rz_engine::TransformOptions;
use rz_engine::{compress, extract};
use rz_formats::OutputFormat;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct ArchiveListing {
    pub format: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveOperationReport {
    pub files: u64,
    pub dirs: u64,
    pub skipped: u64,
    pub bytes: u64,
}

pub struct CliExtractOptions {
    pub password: Option<String>,
    pub encoding: Option<String>,
    pub threads: usize,
    pub conflict: OnConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveTestReport {
    pub entries: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ArchiveTransformReport {
    pub files: u64,
    pub bytes: u64,
    pub skipped: u64,
    pub issues: Vec<String>,
}

pub struct CliTransformOptions {
    pub input_password: Option<Zeroizing<String>>,
    pub output_password: Option<Zeroizing<String>>,
    pub encoding: Option<String>,
    pub threads: usize,
    pub level: u32,
    pub dictionary_size: Option<u32>,
    pub solid_block_size: Option<u64>,
}

/// Creates an archive while keeping format selection and engine access in `rz-app`.
pub fn create_archive(
    sources: &[PathBuf],
    output: &Path,
    opts: &CreateOptions,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<ArchiveOperationReport> {
    let format = OutputFormat::from_path(output).ok_or_else(|| {
        Error::Unsupported(
            "cannot create this format; use .zip, .7z, .tar.gz, .tar.xz, .tar.zst, .tar.bz2, .tar.lz4, .gz, .xz, .zst, .bz2 or .lz4".into(),
        )
    })?;
    let report = compress(sources, output, format, opts, cancel, progress)?;
    Ok(ArchiveOperationReport {
        files: report.files,
        dirs: report.dirs,
        skipped: report.skipped,
        bytes: report.bytes,
    })
}

/// Lists an archive after applying optional password and filename-encoding hints.
pub fn list_archive(
    archive: &Path,
    password: Option<&str>,
    encoding: Option<&str>,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<ArchiveListing> {
    let mut reader = rz_formats::open(archive)?;
    reader.set_read_control(cancel.clone(), u64::MAX);
    if let Some(encoding) = encoding {
        reader.set_encoding(encoding)?;
    }
    if let Some(password) = password {
        reader.set_password(password)?;
    }
    let entries = reader.entries_with_progress(cancel, progress)?;
    cancel.check()?;
    let bytes_done = entries
        .iter()
        .fold(0u64, |total, entry| total.saturating_add(entry.size));
    progress.update(Progress {
        files_done: entries.len() as u64,
        files_total: entries.len() as u64,
        bytes_done,
        bytes_total: 0,
    });
    Ok(ArchiveListing {
        format: reader.format_name().to_string(),
        entries,
    })
}

/// Extracts an archive with user-facing conflict and resource settings.
pub fn extract_archive(
    archive: &Path,
    dest: &Path,
    cli_options: CliExtractOptions,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<ArchiveOperationReport> {
    let settings = Settings {
        threads: cli_options.threads,
        on_conflict: cli_options.conflict,
        ..Settings::default()
    };
    let opts = settings.extract_options(cli_options.password);
    let mut reader = rz_formats::open(archive)?;
    if let Some(encoding) = cli_options.encoding.as_deref() {
        reader.set_encoding(encoding)?;
    }
    let report = extract(&mut *reader, dest, &opts, cancel, progress)?;
    Ok(ArchiveOperationReport {
        files: report.files,
        dirs: report.dirs,
        skipped: report.skipped,
        bytes: report.bytes,
    })
}

/// Reads every archive entry and verifies the checksums provided by its format.
pub fn test_archive(
    archive: &Path,
    password: Option<&str>,
    encoding: Option<&str>,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<ArchiveTestReport> {
    let mut reader = rz_formats::open(archive)?;
    if let Some(encoding) = encoding {
        reader.set_encoding(encoding)?;
    }
    if let Some(password) = password {
        reader.set_password(password)?;
    }
    reader.set_read_control(cancel.clone(), u64::MAX);
    let entries = reader.entries()?;
    let total_entries = entries.len() as u64;
    let bytes_total = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .fold(0u64, |total, entry| total.saturating_add(entry.size));
    reader.set_read_control(cancel.clone(), bytes_total);
    let mut buffer = vec![0u8; 256 * 1024];
    let (mut entries_done, mut bytes_done) = (0u64, 0u64);
    reader.for_each(&mut |entry, data| {
        cancel.check()?;
        if entry.kind == EntryKind::File {
            loop {
                cancel.check()?;
                let count = data.read(&mut buffer).map_err(|error| {
                    if cancel.is_cancelled() {
                        Error::Cancelled
                    } else {
                        Error::Corrupt(format!("{}: {error}", entry.path))
                    }
                })?;
                if count == 0 {
                    break;
                }
                bytes_done = bytes_done.saturating_add(count as u64);
                progress.update(Progress {
                    files_done: entries_done,
                    files_total: total_entries,
                    bytes_done,
                    bytes_total,
                });
            }
        }
        entries_done = entries_done.saturating_add(1);
        Ok(())
    })?;
    progress.update(Progress {
        files_done: entries_done,
        files_total: total_entries,
        bytes_done,
        bytes_total,
    });
    Ok(ArchiveTestReport {
        entries: entries_done,
        bytes: bytes_done,
    })
}

/// Converts or repairs an archive using the same bounded streaming engine as the GUI.
/// Repair mode keeps readable entries and reports entries it had to skip.
pub fn transform_archive(
    source: &Path,
    output: &Path,
    options: CliTransformOptions,
    recover: bool,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<ArchiveTransformReport> {
    let format = OutputFormat::from_path(output)
        .ok_or_else(|| Error::Unsupported("unsupported output archive extension".into()))?;
    if matches!(format, OutputFormat::Single(_)) {
        return Err(Error::Unsupported(
            "conversion and repair require a ZIP, 7z or tar archive output".into(),
        ));
    }
    let settings = Settings {
        threads: options.threads,
        ..Settings::default()
    };
    let transform_options = TransformOptions {
        format,
        create: CreateOptions {
            level: options.level,
            threads: options.threads,
            password: options
                .output_password
                .as_ref()
                .map(|password| password.as_str().to_owned()),
            dictionary_size: options.dictionary_size,
            solid_block_size: options.solid_block_size,
            ..CreateOptions::default()
        },
        limits: settings.extract_options(
            options
                .input_password
                .as_ref()
                .map(|password| password.as_str().to_owned()),
        ),
        encoding: options.encoding,
    };
    let report = rz_engine::transform(
        source,
        output,
        &transform_options,
        recover,
        cancel,
        progress,
    )?;
    Ok(ArchiveTransformReport {
        files: report.files,
        bytes: report.bytes,
        skipped: report.skipped,
        issues: report.issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn make_zip(path: &Path) {
        let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, contents) in [("one.txt", b"one".as_slice()), ("two.txt", b"second")] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn archive_listing_reports_metadata_progress() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("list.zip");
        make_zip(&archive);
        let mut updates = Vec::new();

        let listing = list_archive(
            &archive,
            None,
            None,
            &CancelToken::new(),
            &mut |progress: Progress| updates.push(progress),
        )
        .unwrap();

        assert_eq!(listing.entries.len(), 2);
        let final_update = updates.last().unwrap();
        assert_eq!(final_update.files_done, 2);
        assert_eq!(final_update.files_total, 2);
        assert_eq!(final_update.bytes_done, 9);
    }

    #[test]
    fn archive_listing_can_be_cancelled_during_metadata_scan() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("list.zip");
        make_zip(&archive);
        let cancel = CancelToken::new();
        let during_progress = cancel.clone();

        let result = list_archive(&archive, None, None, &cancel, &mut move |_: Progress| {
            during_progress.cancel()
        });

        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn streaming_archive_listing_can_be_cancelled_while_skipping_entry_data() {
        let temp = tempfile::tempdir().unwrap();
        let large = temp.path().join("large.bin");
        let small = temp.path().join("small.txt");
        let contents: Vec<u8> = (0..8 * 1024 * 1024)
            .map(|index| ((index * 73 + index / 19) % 251) as u8)
            .collect();
        fs::write(&large, contents).unwrap();
        fs::write(&small, b"last").unwrap();
        let archive = temp.path().join("stream.tar.gz");
        let mut create_progress = rz_core::NoProgress;
        create_archive(
            &[large, small],
            &archive,
            &CreateOptions::default(),
            &CancelToken::new(),
            &mut create_progress,
        )
        .unwrap();

        let cancel = CancelToken::new();
        let callback_cancel = cancel.clone();
        let mut cancelled = false;
        let result = list_archive(&archive, None, None, &cancel, &mut move |_: Progress| {
            if !cancelled {
                cancelled = true;
                callback_cancel.cancel();
            }
        });

        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn single_stream_listing_can_be_cancelled_while_counting_unpacked_size() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("large.txt");
        let contents: Vec<u8> = (0..4 * 1024 * 1024)
            .map(|index| ((index * 41 + index / 7) % 251) as u8)
            .collect();
        fs::write(&source, contents).unwrap();
        let archive = temp.path().join("large.txt.gz");
        let mut create_progress = rz_core::NoProgress;
        create_archive(
            &[source],
            &archive,
            &CreateOptions::default(),
            &CancelToken::new(),
            &mut create_progress,
        )
        .unwrap();

        let cancel = CancelToken::new();
        let callback_cancel = cancel.clone();
        let mut cancelled = false;
        let result = list_archive(&archive, None, None, &cancel, &mut move |_: Progress| {
            if !cancelled {
                cancelled = true;
                callback_cancel.cancel();
            }
        });

        assert!(matches!(result, Err(Error::Cancelled)));
    }
}
