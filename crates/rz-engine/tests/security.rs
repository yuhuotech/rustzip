#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use rz_core::{CancelToken, CreateOptions, Error, NoProgress};
use rz_engine::{extract, ExtractOptions};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

fn zip_with(path: &Path, files: &[(&str, &[u8])], opts: SimpleFileOptions) {
    let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, data) in files {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap();
}

fn assert_nothing_escaped(tmp: &Path, out: &Path) {
    for entry in fs::read_dir(tmp).unwrap() {
        let p = entry.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            p == out || name.ends_with(".zip") || name.ends_with(".tar"),
            "unexpected file outside destination: {p:?}"
        );
    }
}

#[test]
fn zip_slip_variants_are_rejected() {
    for bad in [
        "../evil.txt",
        "a/../../evil.txt",
        "..\\evil.txt",
        "/abs/evil.txt",
        "C:/evil.txt",
        "NUL",
        "dir/aux.txt",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("e.zip");
        zip_with(&archive, &[(bad, b"x")], SimpleFileOptions::default());
        let out = tmp.path().join("out");
        let res = common::unpack(&archive, &out, &ExtractOptions::default());
        assert!(matches!(res, Err(Error::UnsafePath(_))), "{bad}: {res:?}");
        assert_nothing_escaped(tmp.path(), &out);
    }
}

#[test]
fn tar_slip_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("e.tar");
    let mut header = tar::Header::new_gnu();
    header.set_size(1);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    // `set_path` refuses `..`, so write the raw name field.
    let name = b"../evil.txt";
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_cksum();
    let mut data = header.as_bytes().to_vec();
    data.push(b'x');
    data.resize(1024, 0);
    data.extend_from_slice(&[0u8; 1024]);
    fs::write(&archive, data).unwrap();

    let out = tmp.path().join("out");
    let res = common::unpack(&archive, &out, &ExtractOptions::default());
    assert!(matches!(res, Err(Error::UnsafePath(_))), "{res:?}");
    assert_nothing_escaped(tmp.path(), &out);
}

#[test]
fn compression_bomb_is_stopped_by_ratio_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("bomb.zip");
    let zeros = vec![0u8; 8 * 1024 * 1024];
    zip_with(
        &archive,
        &[("zeros.bin", &zeros)],
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    );
    let out = tmp.path().join("out");
    let opts = ExtractOptions {
        max_ratio: 50,
        ..ExtractOptions::default()
    };
    let res = common::unpack(&archive, &out, &opts);
    assert!(matches!(res, Err(Error::LimitExceeded(_))), "{res:?}");
    assert!(!out.join("zeros.bin").exists());
}

#[test]
fn total_size_limit_is_enforced_before_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("big.zip");
    zip_with(
        &archive,
        &[("a.bin", &[1u8; 5000]), ("b.bin", &[2u8; 5000])],
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    );
    let out = tmp.path().join("out");
    let opts = ExtractOptions {
        max_total: 9_000,
        ..ExtractOptions::default()
    };
    let res = common::unpack(&archive, &out, &opts);
    assert!(matches!(res, Err(Error::LimitExceeded(_))), "{res:?}");
    assert!(!out.exists() || count_entries(&out) == 0);
}

#[test]
fn zip_symlink_entries_are_not_created() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("l.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    w.add_symlink("link", "/etc/passwd", SimpleFileOptions::default())
        .unwrap();
    w.start_file("ok.txt", SimpleFileOptions::default())
        .unwrap();
    w.write_all(b"ok").unwrap();
    w.finish().unwrap();

    let out = tmp.path().join("out");
    let rep = common::unpack(&archive, &out, &ExtractOptions::default()).unwrap();
    assert_eq!(rep.skipped, 1);
    assert!(fs::symlink_metadata(out.join("link")).is_err());
    assert_eq!(fs::read_to_string(out.join("ok.txt")).unwrap(), "ok");
}

#[cfg(unix)]
#[test]
fn setuid_bits_from_archives_are_not_restored() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("s.zip");
    zip_with(
        &archive,
        &[("tool", b"#!/bin/sh\n")],
        SimpleFileOptions::default().unix_permissions(0o4755),
    );
    let out = tmp.path().join("out");
    common::unpack(&archive, &out, &ExtractOptions::default()).unwrap();
    let mode = fs::metadata(out.join("tool")).unwrap().permissions().mode();
    assert_eq!(mode & 0o7000, 0);
    assert_eq!(mode & 0o777, 0o755);
}

#[cfg(unix)]
#[test]
fn read_only_directories_do_not_block_extracting_their_children() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("ro.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    w.add_directory(
        "locked",
        SimpleFileOptions::default().unix_permissions(0o555),
    )
    .unwrap();
    w.start_file("locked/f.txt", SimpleFileOptions::default())
        .unwrap();
    w.write_all(b"data").unwrap();
    w.finish().unwrap();

    let out = tmp.path().join("out");
    common::unpack(&archive, &out, &ExtractOptions::default()).unwrap();
    assert_eq!(
        fs::read_to_string(out.join("locked/f.txt")).unwrap(),
        "data"
    );
    // The owner keeps write access so the user can clean up.
    let mode = fs::metadata(out.join("locked"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o700, 0o700);
}

#[test]
fn flipped_byte_in_stored_data_is_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("c.zip");
    let payload = vec![b'A'; 4096];
    zip_with(
        &archive,
        &[("a.txt", &payload)],
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    );
    let mut bytes = fs::read(&archive).unwrap();
    let pos = bytes
        .windows(64)
        .position(|w| w.iter().all(|b| *b == b'A'))
        .unwrap();
    bytes[pos + 100] = b'B';
    fs::write(&archive, bytes).unwrap();

    let res = common::unpack(
        &archive,
        &tmp.path().join("out"),
        &ExtractOptions::default(),
    );
    assert!(res.is_err(), "corruption went unnoticed");
}

#[test]
fn damaged_and_foreign_files_give_clear_errors() {
    let tmp = tempfile::tempdir().unwrap();

    let junk = tmp.path().join("junk");
    fs::write(&junk, "not an archive at all").unwrap();
    assert!(matches!(
        rz_formats::open(&junk).err(),
        Some(Error::UnknownFormat)
    ));

    let empty = tmp.path().join("empty");
    fs::write(&empty, b"").unwrap();
    assert!(rz_formats::open(&empty).is_err());

    let seven = tmp.path().join("a.7z");
    fs::write(&seven, [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c, 0, 4]).unwrap();
    assert!(matches!(
        rz_formats::open(&seven).err(),
        Some(Error::Unsupported(_))
    ));

    // A real zip cut in half has no central directory.
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("t.zip");
    pack(&src, &archive, &CreateOptions::default()).unwrap();
    let bytes = fs::read(&archive).unwrap();
    fs::write(&archive, &bytes[..bytes.len() / 2]).unwrap();
    assert!(rz_formats::open(&archive).is_err());
}

#[test]
fn mutated_archives_never_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let opts = ExtractOptions {
        max_total: 64 << 20,
        ..ExtractOptions::default()
    };
    for ext in ["zip", "tar", "tar.gz"] {
        let archive = tmp.path().join(format!("m.{ext}"));
        pack(&src, &archive, &CreateOptions::default()).unwrap();
        let original = fs::read(&archive).unwrap();
        let mut rng = 0x1234_5678_9abc_def0_u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for round in 0..150 {
            let mut data = original.clone();
            for _ in 0..(1 + next() % 8) {
                let at = (next() as usize) % data.len();
                data[at] = next() as u8;
            }
            if round % 5 == 0 {
                data.truncate((next() as usize) % data.len());
            }
            let victim: PathBuf = tmp.path().join(format!("v.{ext}"));
            fs::write(&victim, &data).unwrap();
            let out = tmp.path().join("fuzz-out");
            let _ = fs::remove_dir_all(&out);
            // Any error is fine; a panic or an escape is not.
            if let Ok(mut r) = rz_formats::open(&victim) {
                let _ = extract(&mut *r, &out, &opts, &CancelToken::new(), &mut NoProgress);
            }
            assert!(
                !tmp.path().join("evil.txt").exists(),
                "round {round} escaped the destination"
            );
        }
    }
}
