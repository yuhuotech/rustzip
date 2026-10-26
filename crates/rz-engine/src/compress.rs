use crate::progress::Tracker;
use rz_core::{
    ArchiveWriter, CancelToken, CreateOptions, Error, FileSource, NewEntry, Progress, ProgressSink,
    Result,
};
use rz_formats::OutputFormat;
use rz_platform::{mode_of, mtime_of, same_file};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct CompressReport {
    pub files: u64,
    pub dirs: u64,
    /// Symlinks and other non-regular files that were left out.
    pub skipped: u64,
    pub bytes: u64,
}

struct Item {
    abs: PathBuf,
    meta: NewEntry,
    is_dir: bool,
}

/// Compresses `sources` into `output`.
///
/// `output` is never overwritten: an existing file is refused up front and the
/// finished archive is published with a no-replace rename, so another writer
/// cannot be silently clobbered either. Each run writes to its own uniquely
/// named temporary sibling (`.name.rz-*.part`), so two runs targeting the same
/// file never share or truncate each other's temporary data.
pub fn compress(
    sources: &[PathBuf],
    output: &Path,
    format: OutputFormat,
    opts: &CreateOptions,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<CompressReport> {
    let io = CompressionIo {
        source_opener: &FileSourceOpener,
        writer_factory: &FormatWriterFactory,
    };
    compress_with_io(sources, output, format, opts, cancel, progress, &io)
}

trait SourceOpener: Sync {
    fn open(&self, path: &Path) -> std::io::Result<Box<dyn Read + Send>>;
}

struct FileSourceOpener;

impl SourceOpener for FileSourceOpener {
    fn open(&self, path: &Path) -> std::io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(File::open(path)?))
    }
}

trait ArchiveWriterFactory: Sync {
    fn create(
        &self,
        path: &Path,
        format: OutputFormat,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>>;
}

struct FormatWriterFactory;

impl ArchiveWriterFactory for FormatWriterFactory {
    fn create(
        &self,
        path: &Path,
        format: OutputFormat,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>> {
        rz_formats::create(path, format, opts)
    }
}

struct CompressionIo<'a> {
    source_opener: &'a dyn SourceOpener,
    writer_factory: &'a dyn ArchiveWriterFactory,
}

struct WriteContext<'a> {
    format: OutputFormat,
    opts: &'a CreateOptions,
    cancel: &'a CancelToken,
    progress: &'a mut dyn ProgressSink,
    report: &'a mut CompressReport,
    source_opener: &'a dyn SourceOpener,
    writer_factory: &'a dyn ArchiveWriterFactory,
}

struct PhaseProgress<'a> {
    inner: &'a mut dyn ProgressSink,
    phase: u64,
    phases: u64,
    work_unit: u64,
    file_unit: u64,
}

struct SplitRequest<'a> {
    source: &'a Path,
    output: &'a Path,
    format: OutputFormat,
    volume_size: u64,
    password: Option<&'a str>,
    cancel: &'a CancelToken,
    report: &'a CompressReport,
}

impl<'a> PhaseProgress<'a> {
    fn new(
        inner: &'a mut dyn ProgressSink,
        phase: u64,
        phases: u64,
        source_bytes: u64,
        source_files: u64,
    ) -> Self {
        Self {
            inner,
            phase,
            phases,
            work_unit: source_bytes.max(1),
            file_unit: source_files.max(1),
        }
    }

    fn complete(&mut self) {
        self.inner.update(Progress {
            files_done: self.file_unit * self.phases,
            files_total: self.file_unit * self.phases,
            bytes_done: self.work_unit * self.phases,
            bytes_total: self.work_unit * self.phases,
        });
    }
}

impl ProgressSink for PhaseProgress<'_> {
    fn update(&mut self, progress: Progress) {
        let local_bytes = if progress.bytes_total > 0 {
            u128::from(progress.bytes_done) * u128::from(self.work_unit)
                / u128::from(progress.bytes_total)
        } else if progress.files_total > 0 {
            u128::from(progress.files_done) * u128::from(self.work_unit)
                / u128::from(progress.files_total)
        } else {
            0
        }
        .min(u128::from(self.work_unit)) as u64;
        let local_files = if progress.files_total > 0 {
            u128::from(progress.files_done) * u128::from(self.file_unit)
                / u128::from(progress.files_total)
        } else {
            0
        }
        .min(u128::from(self.file_unit)) as u64;
        self.inner.update(Progress {
            files_done: self.phase * self.file_unit + local_files,
            files_total: self.file_unit * self.phases,
            bytes_done: self.phase * self.work_unit + local_bytes,
            bytes_total: self.work_unit * self.phases,
        });
    }
}

fn compress_with_io(
    sources: &[PathBuf],
    output: &Path,
    format: OutputFormat,
    opts: &CreateOptions,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
    io: &CompressionIo<'_>,
) -> Result<CompressReport> {
    if let Some(size) = opts.volume_size {
        if !matches!(format, OutputFormat::Zip | OutputFormat::SevenZ) {
            return Err(Error::Unsupported(
                "volumes are supported for ZIP and 7z only".into(),
            ));
        }
        if size < 64 * 1024 {
            return Err(Error::Unsupported(
                "volume size must be at least 64 KiB".into(),
            ));
        }
        if matches!(format, OutputFormat::Zip) && size > u64::from(u32::MAX) {
            return Err(Error::Unsupported(
                "ZIP volume size must be below 4 GiB".into(),
            ));
        }
    }
    if format != OutputFormat::SevenZ
        && (opts.dictionary_size.is_some() || opts.solid_block_size.is_some())
    {
        return Err(Error::Unsupported(
            "dictionary size and solid block settings are supported for 7z only".into(),
        ));
    }
    if output.symlink_metadata().is_ok() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "'{}' already exists; RustZIP never overwrites an existing archive",
                output.display()
            ),
        )));
    }
    let mut report = CompressReport::default();
    let mut items = Vec::new();
    for src in sources {
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::UnsafePath(src.display().to_string()))?
            .to_string();
        walk(src, &name, output, &mut items, &mut report, cancel)?;
    }

    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp = tempfile::Builder::new()
        .prefix(&temp_prefix(output))
        .suffix(".part")
        .tempfile_in(parent)?;
    let source_bytes: u64 = items.iter().map(|item| item.meta.size).sum();
    let source_files = items.len() as u64;
    let mut compressed_progress = PhaseProgress::new(
        progress,
        0,
        if opts.volume_size.is_some() { 3 } else { 1 },
        source_bytes,
        source_files,
    );
    let result = write_all(
        &items,
        tmp.path(),
        WriteContext {
            format,
            opts,
            cancel,
            progress: &mut compressed_progress,
            report: &mut report,
            source_opener: io.source_opener,
            writer_factory: io.writer_factory,
        },
    );
    match result {
        Ok(()) => {
            if let Some(volume_size) = opts.volume_size {
                compressed_progress.phase = 1;
                if split_and_publish(
                    SplitRequest {
                        source: tmp.path(),
                        output,
                        format,
                        volume_size,
                        password: opts.password.as_deref(),
                        cancel,
                        report: &report,
                    },
                    &mut compressed_progress,
                )? {
                    return Ok(report);
                }
            }
            match tmp.persist_noclobber(output) {
                Ok(_) => {
                    if opts.volume_size.is_some() {
                        compressed_progress.complete();
                    }
                    Ok(report)
                }
                Err(e) => Err(Error::Io(e.error)),
            }
        }
        Err(e) => Err(e), // dropping `tmp` removes the half-written file
    }
}

/// Prefix shared by every temporary file of this output, so leftovers from a
/// crashed run are recognizable and are never packed into a new archive.
fn temp_prefix(output: &Path) -> String {
    let name = output
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!(".{name}.rz-")
}

fn split_and_publish(request: SplitRequest<'_>, progress: &mut PhaseProgress<'_>) -> Result<bool> {
    let SplitRequest {
        source,
        output,
        format,
        volume_size,
        password,
        cancel,
        report,
    } = request;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let make_part = || tempfile::NamedTempFile::new_in(parent).map_err(Error::Io);
    let parts = match format {
        OutputFormat::Zip => {
            rz_formats::volumes::split_zip(source, volume_size, cancel, progress, make_part)?
        }
        OutputFormat::SevenZ => {
            rz_formats::volumes::split_sevenz(source, volume_size, cancel, progress, make_part)?
        }
        _ => {
            return Err(Error::Unsupported(
                "volumes are supported for ZIP and 7z only".into(),
            ))
        }
    };
    if parts.is_empty() {
        return Ok(false);
    }
    for part in &parts {
        part.as_file().sync_all()?;
    }
    let staged_paths: Vec<PathBuf> = parts.iter().map(|part| part.path().to_path_buf()).collect();
    progress.phase = 2;
    verify_volume_set(format, &staged_paths, password, cancel, progress, report)?;
    let targets = volume_targets(output, format, parts.len())?;
    for target in &targets {
        if target.symlink_metadata().is_ok() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "one or more archive volumes already exist; RustZIP never overwrites them",
            )));
        }
    }

    let mut staged: Vec<Option<tempfile::NamedTempFile>> = parts.into_iter().map(Some).collect();
    let order: Vec<usize> = if matches!(format, OutputFormat::SevenZ) {
        (0..targets.len()).rev().collect()
    } else {
        (0..targets.len()).collect()
    };
    let mut published: Vec<usize> = Vec::with_capacity(order.len());
    for index in order {
        let Some(part) = staged[index].take() else {
            rollback_volumes(&staged, &published, &targets);
            return Err(Error::Corrupt(
                "staged volume disappeared before publishing".into(),
            ));
        };
        match fs::hard_link(part.path(), &targets[index]) {
            Ok(()) => {
                staged[index] = Some(part);
                published.push(index);
            }
            Err(error) => {
                staged[index] = Some(part);
                rollback_volumes(&staged, &published, &targets);
                return Err(Error::Io(error));
            }
        }
    }
    drop(staged);
    progress.complete();
    Ok(true)
}

fn rollback_volumes(
    staged: &[Option<tempfile::NamedTempFile>],
    published: &[usize],
    targets: &[PathBuf],
) {
    for index in published.iter().rev().copied() {
        if staged[index]
            .as_ref()
            .is_some_and(|part| same_file_object(part.path(), &targets[index]))
        {
            let _ = fs::remove_file(&targets[index]);
        }
    }
}

fn same_file_object(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        same_file::is_same_file(a, b).unwrap_or(false)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (a, b);
        false
    }
}

fn volume_targets(output: &Path, format: OutputFormat, count: usize) -> Result<Vec<PathBuf>> {
    if count < 2 {
        return Err(Error::Corrupt(
            "split output must contain at least two volumes".into(),
        ));
    }
    let mut targets = Vec::with_capacity(count);
    match format {
        OutputFormat::Zip => {
            let stem = output
                .file_stem()
                .ok_or_else(|| Error::UnsafePath(output.display().to_string()))?;
            for index in 0..count {
                if index + 1 == count {
                    targets.push(output.to_path_buf());
                } else {
                    let mut name = stem.to_os_string();
                    name.push(format!(".z{:02}", index + 1));
                    targets.push(output.with_file_name(name));
                }
            }
        }
        OutputFormat::SevenZ => {
            let name = output
                .file_name()
                .ok_or_else(|| Error::UnsafePath(output.display().to_string()))?;
            for index in 0..count {
                let mut part = name.to_os_string();
                part.push(format!(".{:03}", index + 1));
                targets.push(output.with_file_name(part));
            }
        }
        _ => {
            return Err(Error::Unsupported(
                "volumes are supported for ZIP and 7z only".into(),
            ))
        }
    }
    Ok(targets)
}

fn verify_volume_set(
    format: OutputFormat,
    paths: &[PathBuf],
    password: Option<&str>,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
    report: &CompressReport,
) -> Result<()> {
    let mut reader = match format {
        OutputFormat::Zip => rz_formats::open_zip_volumes(paths)?,
        OutputFormat::SevenZ => rz_formats::open_sevenz_volumes(paths.to_vec())?,
        _ => {
            return Err(Error::Unsupported(
                "volumes are supported for ZIP and 7z only".into(),
            ))
        }
    };
    if let Some(password) = password {
        reader.set_password(password)?;
    }
    let entries = reader.entries()?;
    let files = entries
        .iter()
        .filter(|entry| entry.kind == rz_core::EntryKind::File)
        .count() as u64;
    let dirs = entries
        .iter()
        .filter(|entry| entry.kind == rz_core::EntryKind::Dir)
        .count() as u64;
    if files != report.files || dirs != report.dirs {
        return Err(Error::Corrupt(
            "split archive entry count did not match its source".into(),
        ));
    }
    let mut buffer = vec![0; 256 * 1024];
    let mut bytes_done = 0u64;
    let mut files_done = 0u64;
    reader.for_each(&mut |entry, data| {
        cancel.check()?;
        if entry.kind == rz_core::EntryKind::File {
            loop {
                cancel.check()?;
                let n = data.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                bytes_done = bytes_done.saturating_add(n as u64);
                progress.update(Progress {
                    files_done,
                    files_total: report.files + report.dirs,
                    bytes_done,
                    bytes_total: report.bytes,
                });
            }
            files_done += 1;
        } else {
            files_done += 1;
        }
        Ok(())
    })?;
    if bytes_done != report.bytes {
        return Err(Error::Corrupt(
            "split archive byte count did not match its source".into(),
        ));
    }
    Ok(())
}

/// Whether `path` is a temporary file belonging to a run compressing to `output`.
fn is_our_temp(path: &Path, output: &Path) -> bool {
    path.parent() == output.parent()
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(&temp_prefix(output)))
}

fn walk(
    path: &Path,
    name: &str,
    output: &Path,
    items: &mut Vec<Item>,
    report: &mut CompressReport,
    cancel: &CancelToken,
) -> Result<()> {
    cancel.check()?;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        report.skipped += 1;
        return Ok(());
    }
    // Never put the archive being written into itself.
    if meta.is_file() && (same_file(path, output) || is_our_temp(path, output)) {
        report.skipped += 1;
        return Ok(());
    }
    let new = NewEntry {
        name: name.to_string(),
        size: if meta.is_file() { meta.len() } else { 0 },
        unix_mode: mode_of(&meta),
        modified: mtime_of(&meta),
    };
    if meta.is_dir() {
        items.push(Item {
            abs: path.to_path_buf(),
            meta: new,
            is_dir: true,
        });
        let mut children: Vec<_> = fs::read_dir(path)?.collect::<std::io::Result<_>>()?;
        children.sort_by_key(|c| c.file_name());
        for child in children {
            let child_name = child.file_name().to_string_lossy().into_owned();
            walk(
                &child.path(),
                &format!("{name}/{child_name}"),
                output,
                items,
                report,
                cancel,
            )?;
        }
    } else if meta.is_file() {
        items.push(Item {
            abs: path.to_path_buf(),
            meta: new,
            is_dir: false,
        });
    } else {
        report.skipped += 1;
    }
    Ok(())
}

/// A source file that is opened on first read and closed at its end, so a
/// backend holding thousands of pending sources does not run out of descriptors.
struct SourceFile<'a> {
    path: &'a Path,
    file: Option<Box<dyn Read + Send>>,
    opener: &'a dyn SourceOpener,
    done: bool,
    tracker: &'a Tracker<'a>,
}

impl Read for SourceFile<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        // Blocks while the task is paused; fails once it is cancelled.
        if self.tracker.check().is_err() {
            // `std::io::copy` retries Interrupted errors, so cancellation must
            // use an error kind that the copy loop returns to the caller.
            return Err(std::io::Error::other("cancelled"));
        }
        if self.file.is_none() {
            self.file = Some(self.opener.open(self.path)?);
        }
        let n = match self.file.as_mut() {
            Some(f) => f.read(buf)?,
            None => 0,
        };
        if n == 0 {
            self.file = None;
            self.done = true;
            self.tracker.file_done();
        } else {
            self.tracker.add_bytes(n as u64);
        }
        Ok(n)
    }
}

fn write_all(items: &[Item], tmp: &Path, context: WriteContext<'_>) -> Result<()> {
    let WriteContext {
        format,
        opts,
        cancel,
        progress,
        report,
        source_opener,
        writer_factory,
    } = context;
    let bytes_total = items.iter().map(|i| i.meta.size).sum();
    let tracker = Tracker::new(progress, cancel, items.len() as u64, bytes_total);
    let mut writer = writer_factory.create(tmp, format, opts)?;

    // Directories first: extractors then create parents before their contents.
    for item in items.iter().filter(|i| i.is_dir) {
        cancel.check()?;
        writer.add_dir(&item.meta)?;
        report.dirs += 1;
        tracker.file_done();
    }

    let tracker_ref = &tracker;
    let mut files = items.iter().filter(|i| !i.is_dir).map(|item| {
        let src: Box<dyn Read + Send + '_> = Box::new(SourceFile {
            path: &item.abs,
            file: None,
            opener: source_opener,
            done: false,
            tracker: tracker_ref,
        });
        let pair: Result<FileSource<'_>> = Ok((item.meta.clone(), src));
        pair
    });
    if let Err(e) = writer.add_files(&mut files) {
        // A cancelled read surfaces as an I/O error from the format backend.
        return Err(if cancel.is_cancelled() {
            Error::Cancelled
        } else {
            e
        });
    }
    cancel.check()?;
    writer.finish()?;
    report.files = items.iter().filter(|i| !i.is_dir).count() as u64;
    report.bytes = bytes_total;
    tracker.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use rz_core::NoProgress;

    struct FailAfterPrefix {
        fail_name: String,
    }

    struct PrefixThenError {
        inner: File,
        remaining: usize,
    }

    impl Read for PrefixThenError {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected source read failure",
                ));
            }
            let available = buf.len().min(self.remaining);
            let n = self.inner.read(&mut buf[..available])?;
            self.remaining -= n;
            Ok(n)
        }
    }

    impl SourceOpener for FailAfterPrefix {
        fn open(&self, path: &Path) -> std::io::Result<Box<dyn Read + Send>> {
            let file = File::open(path)?;
            if path
                .file_name()
                .is_some_and(|n| n == self.fail_name.as_str())
            {
                Ok(Box::new(PrefixThenError {
                    inner: file,
                    remaining: 4096,
                }))
            } else {
                Ok(Box::new(file))
            }
        }
    }

    #[test]
    fn cancelled_source_read_returns_an_error_that_copy_will_not_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input");
        fs::write(&path, b"data").unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut progress = NoProgress;
        let tracker = Tracker::new(&mut progress, &cancel, 1, 4);
        let opener = FileSourceOpener;
        let mut source = SourceFile {
            path: &path,
            file: None,
            opener: &opener,
            done: false,
            tracker: &tracker,
        };

        let error = source.read(&mut [0; 1]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
    }

    #[derive(Clone, Copy)]
    enum WriteFailure {
        Entry,
        Finish,
    }

    struct FailingWriterFactory(WriteFailure);

    struct FailingWriter(WriteFailure);

    impl ArchiveWriterFactory for FailingWriterFactory {
        fn create(
            &self,
            path: &Path,
            _: OutputFormat,
            _: &CreateOptions,
        ) -> Result<Box<dyn ArchiveWriter>> {
            fs::write(path, b"partial archive bytes")?;
            Ok(Box::new(FailingWriter(self.0)))
        }
    }

    impl ArchiveWriter for FailingWriter {
        fn add_dir(&mut self, _: &NewEntry) -> Result<()> {
            Ok(())
        }
        fn add_file(&mut self, _: &NewEntry, src: &mut dyn Read) -> Result<()> {
            if matches!(self.0, WriteFailure::Entry) {
                let mut prefix = [0; 32];
                let _ = src.read(&mut prefix)?;
                return Err(Error::Io(std::io::Error::other("injected writer failure")));
            }
            std::io::copy(src, &mut std::io::sink())?;
            Ok(())
        }
        fn finish(self: Box<Self>) -> Result<()> {
            if matches!(self.0, WriteFailure::Finish) {
                Err(Error::Io(std::io::Error::other("injected finish failure")))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn a_midstream_source_error_removes_the_partial_archive_and_preserves_sources() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("input");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), b"a valid first file").unwrap();
        fs::write(src.join("b.bin"), vec![7; 32 * 1024]).unwrap();
        let before_a = fs::read(src.join("a.txt")).unwrap();
        let before_b = fs::read(src.join("b.bin")).unwrap();
        let output = dir.path().join("partial.zip");
        let source_opener = FailAfterPrefix {
            fail_name: "b.bin".into(),
        };
        let io = CompressionIo {
            source_opener: &source_opener,
            writer_factory: &FormatWriterFactory,
        };
        let result = compress_with_io(
            std::slice::from_ref(&src),
            &output,
            OutputFormat::Zip,
            &CreateOptions {
                threads: 1,
                ..CreateOptions::default()
            },
            &CancelToken::new(),
            &mut NoProgress,
            &io,
        );
        assert!(
            matches!(result, Err(Error::Io(ref e)) if e.kind() == std::io::ErrorKind::PermissionDenied)
        );
        assert!(!output.exists());
        assert!(fs::read_dir(dir.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".partial.zip.rz-")));
        assert_eq!(fs::read(src.join("a.txt")).unwrap(), before_a);
        assert_eq!(fs::read(src.join("b.bin")).unwrap(), before_b);
    }

    #[test]
    fn writer_and_finish_errors_never_publish_or_leave_partial_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("input");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("file.bin"), vec![9; 64 * 1024]).unwrap();
        for (name, phase) in [
            ("entry.zip", WriteFailure::Entry),
            ("finish.zip", WriteFailure::Finish),
        ] {
            let output = dir.path().join(name);
            let writer_factory = FailingWriterFactory(phase);
            let io = CompressionIo {
                source_opener: &FileSourceOpener,
                writer_factory: &writer_factory,
            };
            let result = compress_with_io(
                std::slice::from_ref(&src),
                &output,
                OutputFormat::Zip,
                &CreateOptions::default(),
                &CancelToken::new(),
                &mut NoProgress,
                &io,
            );
            assert!(matches!(result, Err(Error::Io(_))), "{name}: {result:?}");
            assert!(!output.exists(), "{name} was published");
            assert!(
                fs::read_dir(dir.path()).unwrap().all(|e| !e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!(".{name}.rz-"))),
                "{name} left a temp file"
            );
        }
    }
}
