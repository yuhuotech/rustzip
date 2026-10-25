//! Changing an archive that already exists: adding files and deleting entries.
//!
//! No supported format can be edited in place in a way that is safe against a crash
//! halfway, so the archive is unpacked into a scratch folder next to it, changed
//! there, packed into a new file, checked, and only then swapped in for the original.
//! A failure or a cancel at any point leaves the original archive untouched.

use rz_core::{
    clean_path, CancelToken, CreateOptions, EntryKind, Error, Progress, ProgressSink, Result,
};
use rz_engine::{compress, extract, safe_join, ExtractOptions};
use rz_formats::OutputFormat;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum Edit {
    /// Remove these entries (and everything inside the folders among them).
    Delete(Vec<String>),
    /// Copy these files and folders into the folder `into` of the archive (`""` for the top).
    Add { into: String, sources: Vec<PathBuf> },
    /// Give an entry (file or folder) another name, keeping it in the same folder.
    Rename { path: String, new_name: String },
    /// Put the content of a file on disk in place of an existing file entry. Used to save
    /// a file that was opened from the archive and edited.
    Replace { entry: String, with: PathBuf },
}

/// Whether `name` can be the name of one item: no folders, no tricks.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.trim() == name
        && !name.contains(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'])
        && !name.chars().any(char::is_control)
}

#[derive(Debug, Default)]
pub struct EditReport {
    /// Entries in the archive after the change.
    pub files: u64,
    /// Their total size when unpacked.
    pub bytes: u64,
}

/// The format an archive is written back in, from the name the reader reports for it
/// (`"zip"`, `"7z"`, `"tar.gz"`...). `None` for formats that cannot be rewritten.
pub fn edit_format(detected: &str) -> Option<OutputFormat> {
    use rz_formats::Compression;
    Some(match detected {
        "zip" => OutputFormat::Zip,
        "7z" => OutputFormat::SevenZ,
        "tar" => OutputFormat::Tar(Compression::None),
        "tar.gz" => OutputFormat::Tar(Compression::Gzip),
        "tar.xz" => OutputFormat::Tar(Compression::Xz),
        "tar.zst" => OutputFormat::Tar(Compression::Zstd),
        "tar.bz2" => OutputFormat::Tar(Compression::Bzip2),
        _ => return None,
    })
}

/// Maps one phase of the work onto its half of the overall progress bar.
struct Half<'a> {
    inner: &'a mut dyn ProgressSink,
    /// Which half this is: 0 for unpacking, 1 for packing.
    phase: u64,
}

impl ProgressSink for Half<'_> {
    fn update(&mut self, p: Progress) {
        // Both halves cover the same data, so the bar simply doubles the totals.
        self.inner.update(Progress {
            files_done: p.files_done + self.phase * p.files_total,
            files_total: p.files_total * 2,
            bytes_done: p.bytes_done + self.phase * p.bytes_total,
            bytes_total: p.bytes_total * 2,
        });
    }
}

/// The scratch folder is removed however the function ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A bounded set of OS-level advisory locks coordinates edits across processes.
/// Hashed, striped names keep user paths out of the lock files and bound cache growth.
const EDIT_LOCK_STRIPES: usize = 256;

fn archive_lock_path(archive: &Path, root: &Path) -> PathBuf {
    let canonical = archive
        .canonicalize()
        .unwrap_or_else(|_| archive.to_path_buf());
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let stripe = usize::from(digest[0]) % EDIT_LOCK_STRIPES;
    root.join(format!("archive-edit-{stripe:02x}.lock"))
}

fn open_archive_lock(archive: &Path, root: &Path) -> Result<File> {
    fs::create_dir_all(root)?;
    #[cfg(unix)]
    rz_platform::apply_mode(root, 0o700)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(archive_lock_path(archive, root))?)
}

fn archive_lock(archive: &Path, cancel: &CancelToken) -> Result<File> {
    let root = crate::settings::config_dir()
        .ok_or_else(|| {
            Error::Unsupported(
                "a user settings directory is required to coordinate archive edits".into(),
            )
        })?
        .join("archive-locks");
    let target = archive
        .canonicalize()
        .unwrap_or_else(|_| archive.to_path_buf());
    archive_lock_at(archive, &root, cancel, || {
        #[cfg(test)]
        if let (Ok(expected), Ok(marker)) = (
            std::env::var("RZ_TEST_EDIT_LOCK_ARCHIVE"),
            std::env::var("RZ_TEST_EDIT_LOCK_WAIT_MARKER"),
        ) {
            if Path::new(&expected)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(expected))
                == target
            {
                let _ = fs::write(marker, b"waiting");
            }
        }
        #[cfg(not(test))]
        let _ = &target;
    })
}

fn archive_lock_at(
    archive: &Path,
    root: &Path,
    cancel: &CancelToken,
    mut on_wait: impl FnMut(),
) -> Result<File> {
    let lock = open_archive_lock(archive, root)?;
    loop {
        cancel.check()?;
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => {
                on_wait();
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

/// Distinct names per edit within this process, so concurrent edits never share
/// a scratch folder or a new-archive file even though they share a PID.
fn unique_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

pub fn edit_archive(
    archive: &Path,
    edit: &Edit,
    opts: &ExtractOptions,
    level: u32,
    encoding: Option<&str>,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<EditReport> {
    // Waiting on a different RustZIP process remains cancellable.
    let _serialization = archive_lock(archive, cancel)?;
    // What the file really is decides, not its name: a `.jar` or `.docx` is a zip.
    let mut reader = rz_formats::open(archive)?;
    let format = edit_format(reader.format_name())
        .ok_or_else(|| Error::Unsupported("only zip, 7z and tar archives can be changed".into()))?;
    let parent = archive
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Same folder as the archive, so the final swap is a rename on one disk.
    let scratch = Scratch(parent.join(format!(
        ".{name}.rz-edit-{}-{}",
        std::process::id(),
        unique_seq()
    )));
    let _ = fs::remove_dir_all(&scratch.0);
    let stage = scratch.0.join("files");
    fs::create_dir_all(&stage)?;

    // 1. Unpack everything.
    if let Some(enc) = encoding {
        reader.set_encoding(enc)?;
    }
    extract(
        &mut *reader,
        &stage,
        opts,
        cancel,
        &mut Half {
            inner: progress,
            phase: 0,
        },
    )?;
    drop(reader);
    cancel.check()?;

    // 2. Change the unpacked copy.
    match edit {
        Edit::Delete(paths) => {
            for path in paths {
                let target = safe_join(&stage, &clean_path(path))?;
                match fs::symlink_metadata(&target) {
                    Ok(m) if m.is_dir() => fs::remove_dir_all(&target)?,
                    Ok(_) => fs::remove_file(&target)?,
                    // Already gone, for example because a folder above it was deleted.
                    Err(_) => {}
                }
            }
        }
        Edit::Add { into, sources } => {
            let dest = if clean_path(into).is_empty() {
                stage.clone()
            } else {
                safe_join(&stage, &clean_path(into))?
            };
            fs::create_dir_all(&dest)?;
            for src in sources {
                let file_name = src
                    .file_name()
                    .ok_or_else(|| Error::UnsafePath(src.display().to_string()))?;
                let target = dest.join(file_name);
                // A file with the same name is replaced, like dropping it into a folder.
                match fs::symlink_metadata(&target) {
                    Ok(m) if m.is_dir() => fs::remove_dir_all(&target)?,
                    Ok(_) => fs::remove_file(&target)?,
                    Err(_) => {}
                }
                copy_tree(src, &target, cancel)?;
            }
        }
        Edit::Rename { path, new_name } => {
            if !valid_name(new_name) {
                return Err(Error::UnsafePath(new_name.clone()));
            }
            let from = safe_join(&stage, &clean_path(path))?;
            let to = from
                .parent()
                .ok_or_else(|| Error::UnsafePath(path.clone()))?
                .join(new_name);
            if fs::symlink_metadata(&from).is_err() {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    path.clone(),
                )));
            }
            if fs::symlink_metadata(&to).is_ok() {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    new_name.clone(),
                )));
            }
            fs::rename(&from, &to)?;
        }
        Edit::Replace { entry, with } => {
            let target = safe_join(&stage, &clean_path(entry))?;
            // Only an existing file is replaced; a folder or a missing entry means the
            // archive changed since the file was opened, and guessing would lose data.
            match fs::symlink_metadata(&target) {
                Ok(m) if m.is_file() => {}
                _ => {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        entry.clone(),
                    )))
                }
            }
            fs::remove_file(&target)?;
            copy_tree(with, &target, cancel)?;
        }
    }
    cancel.check()?;

    // 3. Pack the result into a new file next to the original.
    let mut top: Vec<PathBuf> = fs::read_dir(&stage)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    top.sort();
    let new_file = parent.join(format!(
        ".{name}.rz-new-{}-{}",
        std::process::id(),
        unique_seq()
    ));
    // Nothing of ours can be at that path; a leftover from a dead run is removed.
    let _ = fs::remove_file(&new_file);
    let create = CreateOptions {
        level,
        password: opts.password.clone(),
        threads: opts.threads,
        ..CreateOptions::default()
    };
    let result = compress(
        &top,
        &new_file,
        format,
        &create,
        cancel,
        &mut Half {
            inner: progress,
            phase: 1,
        },
    )
    .and_then(|_| verify(&new_file, opts.password.as_deref(), cancel));
    let report = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = fs::remove_file(&new_file);
            return Err(e);
        }
    };

    // 4. Swap it in. A rename within one folder replaces the original in one step.
    cancel.check().inspect_err(|_| {
        let _ = fs::remove_file(&new_file);
    })?;
    if let Err(e) = fs::rename(&new_file, archive) {
        let _ = fs::remove_file(&new_file);
        return Err(e.into());
    }
    Ok(report)
}

/// Reads the new archive from end to end, so a damaged result never replaces a good one.
fn verify(path: &Path, password: Option<&str>, cancel: &CancelToken) -> Result<EditReport> {
    let mut reader = rz_formats::open(path)?;
    if let Some(pw) = password {
        reader.set_password(pw)?;
    }
    let mut report = EditReport::default();
    let mut buf = vec![0u8; 256 * 1024];
    reader.for_each(&mut |e, data| {
        cancel.check()?;
        if e.kind == EntryKind::File {
            let mut seen = 0u64;
            loop {
                let n = data
                    .read(&mut buf)
                    .map_err(|err| Error::Corrupt(format!("{}: {err}", e.path)))?;
                if n == 0 {
                    break;
                }
                seen += n as u64;
            }
            report.bytes += seen;
        }
        report.files += 1;
        Ok(())
    })?;
    Ok(report)
}

/// Copies a file or folder, keeping modification times. Symlinks are left out,
/// the same as when creating an archive.
fn copy_tree(from: &Path, to: &Path, cancel: &CancelToken) -> Result<()> {
    cancel.check()?;
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()), cancel)?;
        }
    } else if meta.is_file() {
        fs::copy(from, to)?;
        if let Some(t) = rz_platform::mtime_of(&meta) {
            let _ = rz_platform::set_mtime(to, t);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_core::NoProgress;

    struct EditGate {
        dir: PathBuf,
        role: String,
        stopped: bool,
    }

    impl ProgressSink for EditGate {
        fn update(&mut self, progress: Progress) {
            if !self.stopped
                && progress.files_total > 0
                && progress.files_done >= progress.files_total / 2
            {
                self.stopped = true;
                fs::write(self.dir.join(format!("gate-{}", self.role)), b"staged").unwrap();
                let release = self.dir.join(format!("release-{}", self.role));
                let deadline = std::time::Instant::now() + Duration::from_secs(20);
                while !release.exists() {
                    assert!(std::time::Instant::now() < deadline, "edit gate timed out");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    fn wait_for_file(path: &Path, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        path.exists()
    }

    fn kill_children_on_timeout(children: &mut [std::process::Child], phase: &str) -> ! {
        for child in children {
            let _ = child.kill();
            let _ = child.wait();
        }
        panic!("timed out waiting for cross-process edit {phase}");
    }

    #[test]
    fn concurrent_edits_in_separate_processes_merge_instead_of_losing_one_update() {
        if let Ok(role) = std::env::var("RZ_TEST_EDIT_ROLE") {
            let dir = PathBuf::from(std::env::var("RZ_TEST_EDIT_DIR").unwrap());
            let archive = dir.join("shared.zip");
            let edit = match role.as_str() {
                "a" => Edit::Delete(vec!["data/a.txt".into()]),
                "b" => Edit::Delete(vec!["data/sub/b.txt".into()]),
                _ => panic!("unknown cross-process test role"),
            };
            let mut progress = EditGate {
                dir: dir.clone(),
                role: role.clone(),
                stopped: false,
            };
            edit_archive(
                &archive,
                &edit,
                &opts(None),
                6,
                None,
                &CancelToken::new(),
                &mut progress,
            )
            .unwrap();
            fs::write(dir.join(format!("done-{role}")), b"done").unwrap();
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let archive = make(dir.path(), "shared.zip", None);
        let exe = std::env::current_exe().unwrap();
        let spawn = |role: &str| {
            std::process::Command::new(&exe)
            .arg("--exact")
            .arg("modify::tests::concurrent_edits_in_separate_processes_merge_instead_of_losing_one_update")
            .arg("--nocapture")
            .env("RZ_TEST_EDIT_ROLE", role)
            .env("RZ_TEST_EDIT_DIR", dir.path())
            .env("RZ_TEST_EDIT_LOCK_ARCHIVE", &archive)
            .env("RZ_TEST_EDIT_LOCK_WAIT_MARKER", dir.path().join(format!("wait-{role}")))
            .spawn().unwrap()
        };
        let mut children = vec![spawn("a"), spawn("b")];
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let holder = loop {
            if dir.path().join("gate-a").exists() {
                break "a";
            }
            if dir.path().join("gate-b").exists() {
                break "b";
            }
            if std::time::Instant::now() >= deadline {
                kill_children_on_timeout(&mut children, "first edit staging");
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let waiter = if holder == "a" { "b" } else { "a" };
        if !wait_for_file(
            &dir.path().join(format!("wait-{waiter}")),
            Duration::from_secs(20),
        ) {
            kill_children_on_timeout(&mut children, "second process to wait on the archive lock");
        }
        assert!(
            !dir.path().join(format!("gate-{waiter}")).exists(),
            "both processes staged the same original archive"
        );
        fs::write(dir.path().join(format!("release-{holder}")), b"go").unwrap();
        if !wait_for_file(
            &dir.path().join(format!("done-{holder}")),
            Duration::from_secs(20),
        ) {
            kill_children_on_timeout(&mut children, "first edit publish");
        }
        if !wait_for_file(
            &dir.path().join(format!("gate-{waiter}")),
            Duration::from_secs(20),
        ) {
            kill_children_on_timeout(&mut children, "second edit staging the latest archive");
        }
        fs::write(dir.path().join(format!("release-{waiter}")), b"go").unwrap();
        for index in 0..children.len() {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            loop {
                if let Some(status) = children[index].try_wait().unwrap() {
                    assert!(status.success(), "cross-process edit failed: {status}");
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    kill_children_on_timeout(&mut children, "child process exit");
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(names(&archive, None), ["data/sub/c.txt"]);
        assert!(fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .all(|e| !e.file_name().to_string_lossy().contains(".rz-")));
    }

    #[test]
    fn waiting_for_an_archive_lock_is_cancellable() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        fs::write(&archive, b"archive").unwrap();
        let held = open_archive_lock(&archive, dir.path()).unwrap();
        held.try_lock().unwrap();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_archive = archive.clone();
        let root = dir.path().to_path_buf();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(());
            archive_lock_at(&worker_archive, &root, &worker_cancel, || {}).map(|_| ())
        });
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.cancel();
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
        drop(held);
    }

    fn make(dir: &Path, name: &str, password: Option<&str>) -> PathBuf {
        let src = dir.join("data");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "alpha").unwrap();
        fs::write(src.join("sub/b.txt"), "beta").unwrap();
        fs::write(src.join("sub/c.txt"), "gamma").unwrap();
        let out = dir.join(name);
        compress(
            &[src],
            &out,
            OutputFormat::from_path(&out).unwrap(),
            &CreateOptions {
                password: password.map(str::to_string),
                ..CreateOptions::default()
            },
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        out
    }

    fn names(path: &Path, password: Option<&str>) -> Vec<String> {
        let mut r = rz_formats::open(path).unwrap();
        if let Some(pw) = password {
            r.set_password(pw).unwrap();
        }
        let mut v: Vec<String> = r
            .entries()
            .unwrap()
            .iter()
            .filter(|e| e.kind == EntryKind::File)
            .map(|e| clean_path(&e.path))
            .collect();
        v.sort();
        v
    }

    fn opts(password: Option<&str>) -> ExtractOptions {
        ExtractOptions {
            password: password.map(str::to_string),
            ..ExtractOptions::default()
        }
    }

    fn run(archive: &Path, edit: &Edit, password: Option<&str>) -> Result<EditReport> {
        edit_archive(
            archive,
            edit,
            &opts(password),
            6,
            None,
            &CancelToken::new(),
            &mut NoProgress,
        )
    }

    #[test]
    fn deleting_a_file_and_a_folder_works_in_every_editable_format() {
        for name in ["t.zip", "t.7z", "t.tar", "t.tar.gz"] {
            let tmp = tempfile::tempdir().unwrap();
            let archive = make(tmp.path(), name, None);
            run(&archive, &Edit::Delete(vec!["data/a.txt".into()]), None).unwrap();
            assert_eq!(
                names(&archive, None),
                ["data/sub/b.txt", "data/sub/c.txt"],
                "{name}"
            );
            let rep = run(&archive, &Edit::Delete(vec!["data/sub".into()]), None).unwrap();
            assert_eq!(names(&archive, None), Vec::<String>::new(), "{name}");
            assert_eq!(rep.bytes, 0);
            // No scratch files are left behind.
            let left: Vec<_> = fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with('.'))
                .collect();
            assert!(left.is_empty(), "{name}: {left:?}");
        }
    }

    #[test]
    fn adding_files_replaces_same_named_ones_and_keeps_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.zip", None);
        let extra = tmp.path().join("new");
        fs::create_dir_all(extra.join("deep")).unwrap();
        fs::write(extra.join("a.txt"), "replaced").unwrap();
        fs::write(extra.join("deep/z.txt"), "zed").unwrap();
        let pic = tmp.path().join("pic.png");
        fs::write(&pic, [1u8, 2, 3]).unwrap();

        let edit = Edit::Add {
            into: "data".into(),
            sources: vec![extra.join("a.txt"), extra.join("deep"), pic],
        };
        let rep = run(&archive, &edit, None).unwrap();
        assert_eq!(
            names(&archive, None),
            [
                "data/a.txt",
                "data/deep/z.txt",
                "data/pic.png",
                "data/sub/b.txt",
                "data/sub/c.txt"
            ]
        );
        // Folders may be counted as entries too, so only a lower bound is certain.
        assert!(rep.files >= 5);
        // The replaced file has the new content.
        let out = tmp.path().join("out");
        let mut r = rz_formats::open(&archive).unwrap();
        extract(
            &mut *r,
            &out,
            &ExtractOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(out.join("data/a.txt")).unwrap(),
            "replaced"
        );
    }

    #[test]
    fn an_encrypted_archive_stays_encrypted() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.7z", Some("pw"));
        run(
            &archive,
            &Edit::Delete(vec!["data/a.txt".into()]),
            Some("pw"),
        )
        .unwrap();
        assert_eq!(
            names(&archive, Some("pw")),
            ["data/sub/b.txt", "data/sub/c.txt"]
        );
        // Without the password the archive still refuses to open its content.
        assert!(run(&archive, &Edit::Delete(vec!["data/sub".into()]), None).is_err());
        assert_eq!(names(&archive, Some("pw")).len(), 2);
    }

    #[test]
    fn a_failed_or_cancelled_edit_leaves_the_original_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.zip", None);
        let before = fs::read(&archive).unwrap();

        // Cancelled before it starts.
        let token = CancelToken::new();
        token.cancel();
        let r = edit_archive(
            &archive,
            &Edit::Delete(vec!["data/a.txt".into()]),
            &opts(None),
            6,
            None,
            &token,
            &mut NoProgress,
        );
        assert!(matches!(r, Err(Error::Cancelled)));
        assert_eq!(fs::read(&archive).unwrap(), before);

        // A path that tries to climb out of the archive is refused.
        let r = run(&archive, &Edit::Delete(vec!["../escape".into()]), None);
        assert!(matches!(r, Err(Error::UnsafePath(_))));
        assert_eq!(fs::read(&archive).unwrap(), before);
    }

    #[test]
    fn renaming_moves_an_entry_and_refuses_clashes_and_bad_names() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.7z", None);
        let rename = |path: &str, to: &str| {
            run(
                &archive,
                &Edit::Rename {
                    path: path.into(),
                    new_name: to.into(),
                },
                None,
            )
        };
        rename("data/a.txt", "first.txt").unwrap();
        // A folder is renamed together with what is inside it.
        rename("data/sub", "more").unwrap();
        assert_eq!(
            names(&archive, None),
            ["data/first.txt", "data/more/b.txt", "data/more/c.txt"]
        );
        let before = fs::read(&archive).unwrap();
        assert!(matches!(
            rename("data/first.txt", "more"),
            Err(Error::Io(_))
        ));
        assert!(matches!(
            rename("data/first.txt", "../x"),
            Err(Error::UnsafePath(_))
        ));
        assert!(matches!(
            rename("data/first.txt", "a/b"),
            Err(Error::UnsafePath(_))
        ));
        assert!(rename("data/none.txt", "x").is_err());
        assert_eq!(fs::read(&archive).unwrap(), before);
        assert!(valid_name("报告 终稿.pdf") && !valid_name(" x") && !valid_name(".."));
    }

    #[test]
    fn replacing_a_file_swaps_its_content_and_keeps_everything_else() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make(tmp.path(), "t.zip", None);
        let edited = tmp.path().join("whatever-name.txt");
        fs::write(&edited, "edited!").unwrap();
        run(
            &archive,
            &Edit::Replace {
                entry: "data/sub/b.txt".into(),
                with: edited.clone(),
            },
            None,
        )
        .unwrap();
        assert_eq!(names(&archive, None).len(), 3);
        let out = tmp.path().join("out");
        let mut r = rz_formats::open(&archive).unwrap();
        extract(
            &mut *r,
            &out,
            &ExtractOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(out.join("data/sub/b.txt")).unwrap(),
            "edited!"
        );
        assert_eq!(fs::read_to_string(out.join("data/a.txt")).unwrap(), "alpha");
        // An entry that is no longer there is not silently recreated.
        let r = run(
            &archive,
            &Edit::Replace {
                entry: "data/gone.txt".into(),
                with: edited,
            },
            None,
        );
        assert!(r.is_err());
    }

    #[test]
    fn single_file_and_unknown_formats_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let one = tmp.path().join("x.txt");
        fs::write(&one, "hello hello").unwrap();
        let p = tmp.path().join("x.txt.gz");
        compress(
            &[one],
            &p,
            OutputFormat::from_path(&p).unwrap(),
            &CreateOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert!(edit_format("gzip").is_none());
        assert!(edit_format("rar").is_none());
        assert!(edit_format("zip").is_some() && edit_format("tar.zst").is_some());
        assert!(matches!(
            run(&p, &Edit::Delete(vec![]), None),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn what_the_file_is_decides_not_what_it_is_called() {
        // A zip under another name (a .jar, a .docx...) can be changed like any zip.
        let tmp = tempfile::tempdir().unwrap();
        let zip = make(tmp.path(), "t.zip", None);
        let odd = tmp.path().join("library.jar");
        fs::rename(&zip, &odd).unwrap();
        run(&odd, &Edit::Delete(vec!["data/a.txt".into()]), None).unwrap();
        assert_eq!(names(&odd, None), ["data/sub/b.txt", "data/sub/c.txt"]);
        assert!(odd.is_file());
    }
}
