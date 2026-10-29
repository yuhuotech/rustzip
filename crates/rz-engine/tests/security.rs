#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use rz_core::{CancelToken, CreateOptions, Error, NoProgress, Progress};
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
fn chm_slip_is_rejected_before_any_output_is_written() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("e.chm");
    let mut bytes = include_bytes!("../../rz-formats/tests/fixtures/chm_basic.chm").to_vec();
    let name = b"/a.htm";
    let offset = bytes
        .windows(name.len())
        .position(|window| window == name)
        .expect("CHM directory contains the expected entry name");
    bytes[offset..offset + name.len()].copy_from_slice(b"/../ht");
    fs::write(&archive, bytes).unwrap();

    let out = tmp.path().join("out");
    let result = common::unpack(&archive, &out, &ExtractOptions::default());
    assert!(matches!(result, Err(Error::UnsafePath(_))), "{result:?}");
    assert!(!out.exists());
    assert!(!tmp.path().join("ht").exists());
}

#[test]
fn cancelling_chm_extraction_removes_the_partial_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let archive_path = tmp.path().join("content.chm");
    fs::write(
        &archive_path,
        include_bytes!("../../rz-formats/tests/fixtures/chm_basic.chm"),
    )
    .unwrap();
    let output = tmp.path().join("out");
    let cancel = CancelToken::new();
    let stop = cancel.clone();
    let mut progress = move |update: Progress| {
        if update.bytes_done > 0 {
            stop.cancel();
        }
    };
    let mut reader = rz_formats::open(&archive_path).unwrap();

    let result = extract(
        &mut *reader,
        &output,
        &ExtractOptions::default(),
        &cancel,
        &mut progress,
    );
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert!(!output.join("a.htm").exists());
    assert!(!output.join("large.bin").exists());
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
        ratio_check_above: 0,
        ..ExtractOptions::default()
    };
    let res = common::unpack(&archive, &out, &opts);
    assert!(matches!(res, Err(Error::LimitExceeded(_))), "{res:?}");
    assert!(!out.join("zeros.bin").exists());
}

#[test]
fn small_highly_compressible_entries_are_allowed_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("zeros.zip");
    zip_with(
        &archive,
        &[("zeros.bin", &vec![0u8; 8 * 1024 * 1024])],
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    );
    let out = tmp.path().join("out");
    common::unpack(&archive, &out, &ExtractOptions::default()).unwrap();
    assert_eq!(fs::metadata(out.join("zeros.bin")).unwrap().len(), 8 << 20);
}

#[test]
fn entry_larger_than_its_header_claims_is_cut_off() {
    // A stream that yields more bytes than the declared size must not be written in full.
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("single.gz");
    let big = tmp.path().join("big.txt");
    fs::write(&big, "x".repeat(100_000)).unwrap();
    pack(&big, &archive, &CreateOptions::default()).unwrap();
    let mut r = rz_formats::open(&archive).unwrap();
    let entries = r.entries().unwrap();
    assert_eq!(entries[0].size, 100_000);
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

    // A 7z signature with nothing behind it is a truncated archive.
    let seven = tmp.path().join("a.7z");
    fs::write(&seven, [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c, 0, 4]).unwrap();
    assert!(matches!(
        rz_formats::open(&seven).err(),
        Some(Error::Corrupt(_) | Error::Io(_))
    ));

    // Without the `rar` feature the format is recognised but reported as unsupported.
    #[cfg(not(feature = "rar"))]
    {
        let rar = tmp.path().join("a.rar");
        fs::write(&rar, b"Rar!\x1a\x07\x01\x00rest").unwrap();
        assert!(matches!(
            rz_formats::open(&rar).err(),
            Some(Error::Unsupported(_))
        ));
    }

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
    for ext in ["zip", "7z", "tar", "tar.gz", "tar.xz", "tar.zst", "tar.bz2"] {
        let archive = tmp.path().join(format!("m.{ext}"));
        let create = CreateOptions {
            threads: 2,
            ..CreateOptions::default()
        };
        pack(&src, &archive, &create).unwrap();
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

// ------------------------------------------------- FS-04 / FS-05: links ----

#[cfg(unix)]
mod link_defense {
    use super::*;
    use rz_core::{ArchiveReader, Caps, Entry, EntryKind, Result};
    use std::io::Read;
    use std::path::PathBuf;

    fn stored() -> SimpleFileOptions {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    }

    /// (FS-04) A symlink in the destination that points outside must not be
    /// followed, in any overwrite mode.
    #[test]
    fn a_symlinked_folder_in_the_destination_is_never_followed() {
        for overwrite in [rz_engine::Overwrite::Error, rz_engine::Overwrite::Replace] {
            let tmp = tempfile::tempdir().unwrap();
            let outside = tmp.path().join("outside");
            fs::create_dir_all(&outside).unwrap();
            fs::write(outside.join("sentinel"), b"precious").unwrap();

            let archive = tmp.path().join("e.zip");
            zip_with(&archive, &[("sub/sentinel", b"evil")], stored());
            let out = tmp.path().join("out");
            fs::create_dir_all(&out).unwrap();
            std::os::unix::fs::symlink(&outside, out.join("sub")).unwrap();

            let opts = ExtractOptions {
                overwrite,
                ..ExtractOptions::default()
            };
            let res = common::unpack(&archive, &out, &opts);
            match overwrite {
                // The preflight sees the file through the link and refuses safely.
                rz_engine::Overwrite::Error => assert!(
                    matches!(res, Err(Error::Io(ref e))
                        if e.kind() == std::io::ErrorKind::AlreadyExists),
                    "{res:?}"
                ),
                // Without the exists-check the link defense itself must catch it.
                _ => assert!(matches!(res, Err(Error::UnsafePath(_))), "{res:?}"),
            }
            assert_eq!(
                fs::read(outside.join("sentinel")).unwrap(),
                b"precious",
                "extraction wrote through the link"
            );
        }
    }

    /// (FS-04) The target file itself must not be written through a link either.
    #[test]
    fn a_symlinked_target_file_is_never_written_through() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("old.txt"), b"precious").unwrap();

        let archive = tmp.path().join("e.zip");
        zip_with(&archive, &[("a.txt", b"evil")], stored());
        let out = tmp.path().join("out");
        fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(outside.join("old.txt"), out.join("a.txt")).unwrap();

        let opts = ExtractOptions {
            overwrite: rz_engine::Overwrite::Replace,
            ..ExtractOptions::default()
        };
        let res = common::unpack(&archive, &out, &opts);
        assert!(matches!(res, Err(Error::UnsafePath(_))), "{res:?}");
        assert_eq!(fs::read(outside.join("old.txt")).unwrap(), b"precious");
    }

    /// (FS-04) An archive folder entry landing on a pre-existing link refuses.
    #[test]
    fn a_directory_entry_landing_on_a_link_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"precious").unwrap();

        let archive = tmp.path().join("e.zip");
        let mut w = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        w.add_directory("sub", stored()).unwrap();
        w.finish().unwrap();
        let out = tmp.path().join("out");
        fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(&outside, out.join("sub")).unwrap();

        // Merging into an existing folder is where a link could be followed.
        let merge = ExtractOptions {
            overwrite: rz_engine::Overwrite::Error,
            ..ExtractOptions::default()
        };
        let res = common::unpack(&archive, &out, &merge);
        assert!(matches!(res, Err(Error::UnsafePath(_))), "{res:?}");
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"precious");

        // The default keeps both instead: the link is left alone and never followed.
        common::unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        assert!(out.join("sub (1)").is_dir() && !out.join("sub (1)").is_symlink());
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"precious");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    }

    /// (FS-05) The preflight passed; before the file is created the folder is
    /// swapped for a link. The write must refuse, not follow.
    struct SwapReader {
        dest: PathBuf,
        outside: PathBuf,
    }

    impl ArchiveReader for SwapReader {
        fn format_name(&self) -> &'static str {
            "zip"
        }
        fn caps(&self) -> Caps {
            Caps {
                random_access: false,
                solid: true,
                writable: false,
            }
        }
        fn entries(&mut self) -> Result<Vec<Entry>> {
            Ok(vec![
                Entry {
                    index: 0,
                    path: "d".into(),
                    kind: EntryKind::Dir,
                    size: 0,
                    packed_size: Some(0),
                    modified: None,
                    encrypted: false,
                    unix_mode: None,
                },
                Entry {
                    index: 1,
                    path: "d/sentinel".into(),
                    kind: EntryKind::File,
                    size: 4,
                    packed_size: Some(4),
                    modified: None,
                    encrypted: false,
                    unix_mode: None,
                },
            ])
        }
        fn open<'a>(&'a mut self, _entry: &Entry) -> Result<Box<dyn Read + 'a>> {
            Ok(Box::new(std::io::Cursor::new(b"evil".to_vec())))
        }
        fn for_each(
            &mut self,
            visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
        ) -> Result<()> {
            let entries = self.entries()?;
            for (i, entry) in entries.iter().enumerate() {
                if i == 1 {
                    // The attack: replace the freshly created folder with a link.
                    fs::remove_dir(self.dest.join("d")).unwrap();
                    std::os::unix::fs::symlink(&self.outside, self.dest.join("d")).unwrap();
                }
                let mut empty = std::io::empty();
                let mut data = std::io::Cursor::new(b"evil".to_vec());
                match entry.kind {
                    EntryKind::Dir => visit(entry, &mut empty),
                    _ => visit(entry, &mut data),
                }?;
            }
            Ok(())
        }
    }

    #[test]
    fn a_folder_swapped_for_a_link_after_the_preflight_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"precious").unwrap();
        let out = tmp.path().join("out");

        let mut reader = SwapReader {
            dest: out.clone(),
            outside,
        };
        let res = extract(
            &mut reader,
            &out,
            &ExtractOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        );
        assert!(
            matches!(res, Err(Error::UnsafePath(_))),
            "the swap was followed: {res:?}"
        );
        assert_eq!(
            fs::read(out.join("../outside/sentinel")).unwrap(),
            b"precious",
            "the write escaped the destination"
        );
        // `out/d` is now the attacker's link; the outside folder must still hold
        // exactly the one untouched file.
        let outside_entries: Vec<String> = fs::read_dir(tmp.path().join("outside"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(outside_entries, vec!["sentinel".to_string()]);
    }

    /// After a link refusal the destination stays usable: earlier entries are
    /// intact, a retry in a clean folder works.
    #[test]
    fn extract_reports_and_leaves_the_destination_usable_after_a_link_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("e.zip");
        zip_with(
            &archive,
            &[("ok.txt", b"fine"), ("sub/deep/x.txt", b"also fine")],
            stored(),
        );
        let out = tmp.path().join("out");
        fs::create_dir_all(out.join("sub")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("elsewhere"), out.join("sub/deep")).unwrap();

        // One thread: the entry order in the archive then decides what exists.
        let opts = ExtractOptions {
            threads: 1,
            overwrite: rz_engine::Overwrite::Error,
            ..ExtractOptions::default()
        };
        let res = common::unpack(&archive, &out, &opts);
        assert!(matches!(res, Err(Error::UnsafePath(_))), "{res:?}");
        // Entries before the failure may exist and be correct.
        assert_eq!(fs::read_to_string(out.join("ok.txt")).unwrap(), "fine");
        // After the error a retry in a clean folder works.
        let retry = tmp.path().join("retry");
        common::unpack(&archive, &retry, &opts).unwrap();
        assert_eq!(
            fs::read_to_string(retry.join("sub/deep/x.txt")).unwrap(),
            "also fine"
        );
    }
}
