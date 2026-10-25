//! FS-01 / FS-03 / FS-06 / FS-07: what a compression or extraction failure is
//! allowed to leave behind. Nothing: the sources and any previous archive stay
//! byte for byte, no half-written output exists and no temporary file survives.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use rz_core::{CancelToken, CreateOptions, Error, NoProgress};
use rz_engine::{compress, extract, ExtractOptions, Overwrite};
use rz_formats::OutputFormat;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

/// Data big enough that writing it takes long enough for another thread to
/// observe the temporary file while the archive is still being built.
fn big_tree(root: &Path) -> std::path::PathBuf {
    let p = root.join("proj");
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("a.dat"), pseudo_random(8 << 20, 1)).unwrap();
    fs::write(p.join("b.dat"), pseudo_random(8 << 20, 2)).unwrap();
    fs::write(p.join("c.dat"), pseudo_random(8 << 20, 3)).unwrap();
    p
}

fn zip_with(path: &Path, files: &[(&str, &[u8])], stored: bool) {
    let mut opts = SimpleFileOptions::default();
    if stored {
        opts = opts.compression_method(CompressionMethod::Stored);
    }
    let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, data) in files {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap();
}

// ---------------------------------------------------------------- FS-03 ----

#[test]
fn compressing_onto_an_existing_archive_is_refused_and_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let out = tmp.path().join("a.zip");
    pack(&src, &out, &CreateOptions::default()).unwrap();
    let before = fs::read(&out).unwrap();

    let res = pack(&src, &out, &CreateOptions::default());
    match &res {
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        other => panic!("expected an AlreadyExists refusal, got {other:?}"),
    }
    assert_eq!(fs::read(&out).unwrap(), before, "the old archive changed");
    assert!(temp_leftovers(tmp.path(), &out).is_empty());
}

#[cfg(unix)]
#[test]
fn compression_refuses_outputs_that_alias_a_source_file() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let source_file = src.join("readme.txt");
    let before = fs::read(&source_file).unwrap();
    let symlink_output = tmp.path().join("source-symlink.zip");
    let hardlink_output = tmp.path().join("source-hardlink.zip");
    std::os::unix::fs::symlink(&source_file, &symlink_output).unwrap();
    fs::hard_link(&source_file, &hardlink_output).unwrap();

    for output in [&symlink_output, &hardlink_output] {
        let result = compress(
            std::slice::from_ref(&src),
            output,
            OutputFormat::Zip,
            &CreateOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        );
        assert!(result.is_err(), "source alias was accepted: {output:?}");
        assert_eq!(fs::read(&source_file).unwrap(), before);
        assert!(fs::symlink_metadata(output).is_ok());
        assert!(temp_leftovers(tmp.path(), output).is_empty());
    }
}

// ---------------------------------------------------------------- FS-06 ----

#[test]
fn a_foreign_temporary_at_the_target_is_never_touched() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let out = tmp.path().join("a.zip");
    // Looks like one of ours (a crashed run's leftover) but is not.
    let foreign = tmp.path().join(".a.zip.rz-abandoned.part");
    fs::write(&foreign, b"someone else's data").unwrap();

    pack(&src, &out, &CreateOptions::default()).unwrap();
    assert_eq!(
        fs::read(&foreign).unwrap(),
        b"someone else's data",
        "a foreign temporary file was truncated or reused"
    );
    let mut r = rz_formats::open(&out).unwrap();
    let names = r.entries().unwrap().len();
    assert!(names > 0, "the archive is empty");
}

#[test]
fn two_concurrent_compressions_of_the_same_target_share_nothing() {
    for round in 0..5 {
        let tmp = tempfile::tempdir().unwrap();
        // Identical sources: whichever run wins, the archive must be this tree.
        let src1 = make_tree(&tmp.path().join("s1"));
        let src2 = make_tree(&tmp.path().join("s2"));
        let out = tmp.path().join(format!("a{round}.zip"));
        let out1 = out.clone();
        let out2 = out.clone();

        let barrier = Arc::new(Barrier::new(2));
        let b1 = barrier.clone();
        let b2 = barrier.clone();
        let h1 = std::thread::spawn(move || {
            b1.wait();
            compress(
                &[src1],
                &out1,
                OutputFormat::Zip,
                &CreateOptions::default(),
                &CancelToken::new(),
                &mut NoProgress,
            )
        });
        let h2 = std::thread::spawn(move || {
            b2.wait();
            compress(
                &[src2],
                &out2,
                OutputFormat::Zip,
                &CreateOptions::default(),
                &CancelToken::new(),
                &mut NoProgress,
            )
        });
        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();

        // Exactly one run wins; the loser must fail, never clobber the winner.
        assert!(
            r1.is_ok() ^ r2.is_ok(),
            "round {round}: expected exactly one success, got {r1:?} and {r2:?}"
        );
        assert!(out.is_file());
        let mut r = rz_formats::open(&out).unwrap();
        let entries = r.entries().unwrap();
        let files = entries
            .iter()
            .filter(|e| e.kind == rz_core::EntryKind::File)
            .count();
        assert_eq!(files, 6, "round {round}: wrong content in the output");
        assert!(
            temp_leftovers(tmp.path(), &out).is_empty(),
            "round {round}: temporary files survived"
        );
    }
}

// ---------------------------------------------------------------- FS-01 ----

#[cfg(unix)]
#[test]
#[ignore = "requires a non-root Unix account; deterministic midstream fault injection is in compress::tests"]
fn a_source_that_fails_midway_leaves_no_output_and_no_temporary() {
    // Root can read anything, so permission-based fault injection would not fire.
    let probe = tempfile::tempdir().unwrap();
    let secret = probe.path().join("probe");
    fs::write(&secret, b"x").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();
    assert!(
        fs::File::open(&secret).is_err(),
        "run this environment-specific test as a non-root user"
    );

    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    fs::set_permissions(src.join("big.dat"), fs::Permissions::from_mode(0o000)).unwrap();
    let out = tmp.path().join("a.zip");

    let res = pack(&src, &out, &CreateOptions::default());
    match &res {
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
        other => panic!("expected a permission failure, got {other:?}"),
    }
    assert!(!out.exists());
    assert!(temp_leftovers(tmp.path(), &out).is_empty());
    // The readable sources are untouched.
    assert_eq!(fs::read(src.join("readme.txt")).unwrap(), b"hello rustzip");
}

#[test]
fn a_target_created_during_the_run_fails_the_publish_not_the_data() {
    let tmp = tempfile::tempdir().unwrap();
    let src = big_tree(tmp.path());
    let out = tmp.path().join("a.zip");
    let out2 = out.clone();

    // Another process creates the target as soon as our temporary file appears.
    let dir = tmp.path().to_path_buf();
    let racer = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let seen = fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().starts_with(".a.zip.rz-"));
            if seen {
                fs::write(&out2, b"someone else was here first").unwrap();
                return true;
            }
            assert!(
                Instant::now() < deadline,
                "the temporary file never appeared; fault point not reached"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    let res = pack(&src, &out, &CreateOptions::default());
    assert!(racer.join().unwrap(), "fault point never hit");
    match &res {
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        other => panic!("expected the publish to refuse, got {other:?}"),
    }
    assert_eq!(
        fs::read(&out).unwrap(),
        b"someone else was here first",
        "the racing writer's file was damaged"
    );
    assert!(
        temp_leftovers(tmp.path(), &out).is_empty(),
        "a failed publish left its temporary file behind"
    );
}

// ---------------------------------------------------------------- FS-07 ----

/// A stored zip whose first entry is damaged, so the failure happens while the
/// entry is being read, after the extractor has already committed to writing.
fn corrupt_first_entry(path: &Path) {
    let payload = vec![b'A'; 100_000];
    zip_with(path, &[("a.txt", &payload), ("b.txt", b"second")], true);
    let mut bytes = fs::read(path).unwrap();
    let pos = bytes
        .windows(64)
        .position(|w| w.iter().all(|b| *b == b'A'))
        .unwrap();
    bytes[pos + 100] = b'B';
    fs::write(path, bytes).unwrap();
}

fn replace_opts() -> ExtractOptions {
    ExtractOptions {
        overwrite: Overwrite::Replace,
        threads: 1,
        max_total: 64 << 20,
        ..ExtractOptions::default()
    }
}

#[test]
fn a_damaged_entry_under_replace_keeps_the_old_file_byte_for_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("c.zip");
    corrupt_first_entry(&archive);

    let out = tmp.path().join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("a.txt"), b"OLD-A").unwrap();
    fs::write(out.join("b.txt"), b"OLD-B").unwrap();

    let res = unpack(&archive, &out, &replace_opts());
    assert!(res.is_err(), "a damaged entry reported success");

    // The task may fail, but the user's previous files must survive intact.
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"OLD-A");
    assert_eq!(fs::read(out.join("b.txt")).unwrap(), b"OLD-B");
    let leftovers: Vec<String> = fs::read_dir(&out)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".rz-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_cancelled_replace_keeps_the_old_files() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("c.zip");
    let payload = vec![b'A'; 100_000];
    zip_with(&archive, &[("a.txt", &payload), ("b.txt", b"second")], true);

    let out = tmp.path().join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("a.txt"), b"OLD-A").unwrap();
    fs::write(out.join("b.txt"), b"OLD-B").unwrap();

    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let mut sink = move |_: rz_core::Progress| c2.cancel();
    let mut r = rz_formats::open(&archive).unwrap();
    let res = extract(&mut *r, &out, &replace_opts(), &cancel, &mut sink);
    assert!(
        matches!(res, Err(Error::Cancelled)),
        "expected cancellation, got {res:?}"
    );
    assert_eq!(fs::read(out.join("b.txt")).unwrap(), b"OLD-B");
    assert_eq!(
        fs::read(out.join("a.txt")).unwrap(),
        b"OLD-A",
        "the cancelled replacement destroyed the previous file"
    );
}
