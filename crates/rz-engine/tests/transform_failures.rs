//! TRANS-02 / TRANS-06 / TASK-06: publish-time protection for conversion and
//! repair. Whatever fails — an alias output, a dead folder, a cancellation at
//! any phase — the source survives byte for byte and nothing is published.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_core::{CancelToken, CreateOptions, Error, NoProgress, Progress};
use rz_engine::{transform, ExtractOptions, TransformOptions};
use rz_formats::{Compression, OutputFormat};
use std::fs;
use std::io::Write;
use std::path::Path;

/// `TransformOptions` implements `Drop` (password zeroization), so the usual
/// struct-update syntax cannot move out of it; build each variant in full.
fn opts(format: OutputFormat) -> TransformOptions {
    TransformOptions {
        format,
        create: CreateOptions {
            threads: 1,
            ..Default::default()
        },
        limits: ExtractOptions {
            max_total: 2 << 30,
            ..ExtractOptions::default()
        },
        encoding: None,
    }
}

/// A stored zip with a big `data.bin` (byte 7) and a small healthy `keep.bin`
/// (byte 42), so damaging `data.bin` still leaves something to recover.
fn make_zip(path: &Path, size: usize) {
    let payload = vec![7u8; size];
    let healthy = vec![42u8; 4096];
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("data.bin", opts).unwrap();
    zip.write_all(&payload).unwrap();
    zip.start_file("keep.bin", opts).unwrap();
    zip.write_all(&healthy).unwrap();
    zip.finish().unwrap();
}

/// RustZIP temporary files for `output` left in its folder.
fn leftovers(dir: &Path, output: &Path) -> Vec<String> {
    let base = output
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(&format!(".{base}.rz-")))
        .collect()
}

// ---------------------------------------------------------------- TRANS-02 ---

#[cfg(unix)]
#[test]
fn alias_outputs_are_refused_and_never_damage_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.zip");
    make_zip(&source, 4096);
    let before = fs::read(&source).unwrap();

    // A symlink that points at the source is an alias, not a new output.
    let alias = dir.path().join("alias.tar.gz");
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    let res = transform(
        &source,
        &alias,
        &opts(OutputFormat::Tar(Compression::Gzip)),
        false,
        &CancelToken::new(),
        &mut NoProgress,
    );
    assert!(res.is_err(), "an alias output was accepted");
    assert!(fs::symlink_metadata(&alias).is_ok(), "the link vanished");
    assert_eq!(fs::read(&source).unwrap(), before);

    // A hard link to the source is refused as "exists" as well.
    let hard = dir.path().join("hard.tar.gz");
    fs::hard_link(&source, &hard).unwrap();
    assert!(transform(
        &source,
        &hard,
        &opts(OutputFormat::Tar(Compression::Gzip)),
        false,
        &CancelToken::new(),
        &mut NoProgress
    )
    .is_err());
    assert_eq!(fs::read(&source).unwrap(), before);
    assert!(leftovers(dir.path(), &hard).is_empty());
}

#[test]
fn a_target_created_before_publish_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.zip");
    make_zip(&source, 100 << 20);
    let before = fs::read(&source).unwrap();
    let output = dir.path().join("out.zip");
    let raced_output = output.clone();
    let watch_dir = dir.path().to_path_buf();

    let racer = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let staging_exists = fs::read_dir(&watch_dir)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".out.zip.rz-")
                });
            if staging_exists {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&raced_output)
                    .unwrap();
                file.write_all(b"created by the racing process").unwrap();
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the transform temporary file never appeared"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    let result = transform(
        &source,
        &output,
        &opts(OutputFormat::Zip),
        false,
        &CancelToken::new(),
        &mut NoProgress,
    );
    racer.join().unwrap();
    assert!(
        matches!(result, Err(Error::Io(ref e)) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "the transform did not refuse the racing output: {result:?}"
    );
    assert_eq!(fs::read(&output).unwrap(), b"created by the racing process");
    assert_eq!(fs::read(&source).unwrap(), before);
    assert!(leftovers(dir.path(), &output).is_empty());
}

#[cfg(unix)]
#[test]
fn a_read_only_output_folder_fails_the_whole_operation_cleanly() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.zip");
    make_zip(&source, 4096);
    let before = fs::read(&source).unwrap();

    let ro = dir.path().join("ro");
    fs::create_dir_all(&ro).unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
    let out = ro.join("out.tar.gz");
    let res = transform(
        &source,
        &out,
        &opts(OutputFormat::Tar(Compression::Gzip)),
        false,
        &CancelToken::new(),
        &mut NoProgress,
    );
    assert!(res.is_err(), "a read-only folder was accepted");
    assert!(!out.exists());
    assert_eq!(fs::read(&source).unwrap(), before);
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();
}

// ---------------------------------------------------------------- TASK-06 ---

#[test]
fn a_cancellation_at_any_phase_never_publishes() {
    for fraction in [10u64, 50, 90] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("s.zip");
        // Large enough that gzip takes several progress-report windows (>50 ms each),
        // so a cancellation at any fraction lands mid-write, deterministically.
        make_zip(&source, 100 << 20);
        let output = dir.path().join("out.tar.gz");
        let total = 100 << 20;
        let threshold = total * fraction / 100;

        let token = CancelToken::new();
        let stop = token.clone();
        let res = transform(
            &source,
            &output,
            &opts(OutputFormat::Tar(Compression::Gzip)),
            false,
            &token,
            &mut move |p: Progress| {
                if p.bytes_done >= threshold {
                    stop.cancel();
                }
            },
        );
        assert!(
            matches!(res, Err(Error::Cancelled)),
            "fraction {fraction}: {res:?}"
        );
        assert!(!output.exists(), "fraction {fraction}: published anyway");
        assert!(
            leftovers(dir.path(), &output).is_empty(),
            "fraction {fraction}: temporary files survived"
        );
    }
}

// ---------------------------------------------------------------- TRANS-06 ---

/// A repair whose staging volume dies must fail the whole operation; a damaged
/// entry alone only skips that entry. The two outcomes must never be confused.
#[test]
fn repair_treats_a_dead_staging_volume_differently_from_a_bad_entry() {
    // When re-run as the child of this very test, divert before asserting.
    maybe_child();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.zip");
    make_zip(&source, 4096);
    // Damage the stored data: a CRC error on the entry, nothing more.
    let mut bytes = fs::read(&source).unwrap();
    let at = bytes
        .windows(16)
        .position(|w| w.iter().all(|b| *b == 7))
        .unwrap();
    bytes[at + 64] ^= 0xff;
    fs::write(&source, bytes).unwrap();

    // In-process contrast: a damaged entry is skipped, the repair still succeeds.
    let out = dir.path().join("recovered.zip");
    let report = transform(
        &source,
        &out,
        &opts(OutputFormat::Zip),
        true,
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert!(report.skipped >= 1, "the damage was not even reported");
    assert!(out.exists());

    // A dead staging volume (TMPDIR points at a folder without write access) is
    // an operation failure, never "recovered everything except a few entries".
    // Run in a child process: TMPDIR is process-global.
    let child_env = dir.path().join("dead-tmp");
    fs::create_dir_all(&child_env).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&child_env, fs::Permissions::from_mode(0o555)).unwrap();
        let exe = std::env::current_exe().unwrap();
        let output2 = dir.path().join("never-published.zip");
        let status = std::process::Command::new(exe)
            .arg("--exact")
            .arg("repair_treats_a_dead_staging_volume_differently_from_a_bad_entry")
            .arg("--nocapture")
            .env("RZ_TRANSFORM_CHILD", "1")
            .env("TMPDIR", &child_env)
            .env("RZ_CHILD_SOURCE", &source)
            .env("RZ_CHILD_OUTPUT", &output2)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&status.stdout);
        assert!(
            text.contains("CHILD-RESULT: err"),
            "the child reported success on a dead volume: {text}"
        );
        assert!(!output2.exists(), "a failed repair published a file");
        fs::set_permissions(&child_env, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = child_env;
}

/// Child half of the test above; the parent re-runs this same test binary with
/// `RZ_TRANSFORM_CHILD=1`, and this hook diverts it before any assertion runs.
fn maybe_child() {
    if std::env::var("RZ_TRANSFORM_CHILD").ok().as_deref() != Some("1") {
        return;
    }
    let source: std::path::PathBuf = std::env::var("RZ_CHILD_SOURCE").unwrap().into();
    let output: std::path::PathBuf = std::env::var("RZ_CHILD_OUTPUT").unwrap().into();
    let res = transform(
        &source,
        &output,
        &opts(OutputFormat::Zip),
        true,
        &CancelToken::new(),
        &mut NoProgress,
    );
    match res {
        Err(_) => println!("CHILD-RESULT: err"),
        Ok(_) => println!("CHILD-RESULT: ok"),
    }
    std::process::exit(0);
}
