#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use rz_core::{CancelToken, CreateOptions, EntryKind, Error, NoProgress, Progress};
use rz_engine::{compress, extract, ExtractOptions, Overwrite};
use rz_formats::OutputFormat;
use std::fs;
use std::path::Path;

/// Every format that can hold a directory tree.
const TREE_FORMATS: [&str; 8] = [
    "zip", "7z", "tar", "tar.gz", "tar.xz", "tar.zst", "tar.bz2", "tar.lz4",
];
const SINGLE_FORMATS: [&str; 5] = ["gz", "xz", "zst", "bz2", "lz4"];

/// Two threads keep the memory of big LZMA dictionaries modest on CI machines.
fn opts(level: u32) -> CreateOptions {
    CreateOptions {
        level,
        threads: 2,
        ..CreateOptions::default()
    }
}

#[test]
fn round_trip_preserves_content_names_and_times() {
    for ext in TREE_FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        let rep = pack(&src, &archive, &opts(6)).unwrap();
        assert_eq!(rep.files, 6, "{ext}");
        assert!(rep.dirs >= 3, "{ext}");

        let out = tmp.path().join("out");
        unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        assert_same_tree(&src, &out.join("proj"), ext);
    }
}

#[test]
fn rpm_package_fixture_is_read_only_and_extracts_through_the_safe_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rz-formats/tests/fixtures/longtail_rpm_bzip2.rpm");
    let mut reader = rz_formats::open(&archive).unwrap();
    assert_eq!(reader.format_name(), "rpm");
    assert!(!reader.caps().writable);

    let dest = tmp.path().join("out");
    extract(
        &mut *reader,
        &dest,
        &ExtractOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    for name in ["file1", "file2", "file3"] {
        assert_eq!(fs::read(dest.join("etc").join(name)).unwrap(), b"hello\n");
    }
}

#[test]
fn zip_and_sevenz_volumes_round_trip_and_reject_missing_parts() {
    for ext in ["zip", "7z"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("vol.{ext}"));
        let create = CreateOptions {
            volume_size: Some(64 * 1024),
            ..opts(1)
        };
        let mut progress = Vec::new();
        let mut progress_sink = |value| progress.push(value);
        compress(
            std::slice::from_ref(&src),
            &archive,
            OutputFormat::from_path(&archive).unwrap(),
            &create,
            &CancelToken::new(),
            &mut progress_sink,
        )
        .unwrap();
        assert!(progress.windows(2).all(|pair| {
            pair[1].bytes_done >= pair[0].bytes_done && pair[1].files_done >= pair[0].files_done
        }));
        assert_eq!(
            progress.last().unwrap().bytes_done,
            progress.last().unwrap().bytes_total
        );
        let open_path = if ext == "zip" {
            assert!(archive.is_file());
            assert!(tmp.path().join("vol.z01").is_file());
            archive.clone()
        } else {
            assert!(!archive.exists());
            assert!(tmp.path().join("vol.7z.001").is_file());
            tmp.path().join("vol.7z.001")
        };
        let out = tmp.path().join("out");
        let mut reader = rz_formats::open(&open_path).unwrap();
        extract(
            &mut *reader,
            &out,
            &ExtractOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert_same_tree(&src, &out.join("proj"), &format!("{ext} volumes"));

        let parts = if ext == "zip" {
            rz_formats::volumes::zip_volume_paths(&archive)
                .unwrap()
                .unwrap()
        } else {
            rz_formats::volumes::sevenz_volume_paths(&open_path)
                .unwrap()
                .unwrap()
        };
        let before: Vec<Vec<u8>> = parts.iter().map(|part| fs::read(part).unwrap()).collect();
        assert!(compress(
            std::slice::from_ref(&src),
            &archive,
            OutputFormat::from_path(&archive).unwrap(),
            &create,
            &CancelToken::new(),
            &mut NoProgress,
        )
        .is_err());
        assert_eq!(
            parts
                .iter()
                .map(|part| fs::read(part).unwrap())
                .collect::<Vec<_>>(),
            before,
            "existing {ext} volumes changed"
        );

        let missing = if ext == "zip" {
            tmp.path().join("vol.z01")
        } else {
            tmp.path().join("vol.7z.002")
        };
        let saved = tmp.path().join("missing-part");
        fs::rename(&missing, &saved).unwrap();
        assert!(
            rz_formats::open(&open_path).is_err(),
            "{ext} missing volume opened"
        );
        fs::rename(saved, missing).unwrap();
    }
}

#[test]
fn cancelling_volume_creation_after_packing_leaves_no_public_parts() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let output = tmp.path().join("cancelled.zip");
    let options = CreateOptions {
        volume_size: Some(64 * 1024),
        ..opts(1)
    };
    let token = CancelToken::new();
    let cancel = token.clone();
    let mut sink = move |progress: Progress| {
        if progress.bytes_total > 0 && progress.bytes_done >= progress.bytes_total / 3 {
            cancel.cancel();
        }
    };
    let result = compress(
        std::slice::from_ref(&src),
        &output,
        OutputFormat::Zip,
        &options,
        &token,
        &mut sink,
    );
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert!(!output.exists());
    assert!(!tmp.path().join("cancelled.z01").exists());
    assert!(tmp.path().read_dir().unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".cancelled.zip.rz-")
    }));
}

#[cfg(unix)]
#[test]
fn round_trip_preserves_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    for ext in TREE_FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &opts(6)).unwrap();
        let out = tmp.path().join("out");
        unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        let mode = fs::metadata(out.join("proj/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "{ext}");
    }
}

#[test]
fn every_compression_level_round_trips() {
    for ext in TREE_FORMATS {
        for level in [0, 1, 9] {
            let tmp = tempfile::tempdir().unwrap();
            let src = make_tree(tmp.path());
            let archive = tmp.path().join(format!("a.{ext}"));
            pack(&src, &archive, &opts(level)).unwrap();
            let out = tmp.path().join("out");
            unpack(&archive, &out, &ExtractOptions::default()).unwrap();
            assert_same_tree(&src, &out.join("proj"), &format!("{ext} level {level}"));
        }
    }
}

#[test]
fn higher_level_is_not_larger_on_compressible_data() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("data");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("t.txt"), "the quick brown fox ".repeat(50_000)).unwrap();
    for ext in ["zip", "7z", "tar.xz", "tar.zst", "tar.gz", "tar.bz2"] {
        let size = |level| {
            let archive = tmp.path().join(format!("l{level}.{ext}"));
            pack(&src, &archive, &opts(level)).unwrap();
            fs::metadata(archive).unwrap().len()
        };
        let (stored, fast, best) = (size(0), size(1), size(9));
        // Level 0 means "store" only for these two; the stream codecs have no such mode.
        if ext == "zip" || ext == "7z" {
            assert!(
                stored > 900_000,
                "{ext}: level 0 should store, got {stored}"
            );
            assert!(fast < stored / 10, "{ext}: {fast} vs {stored}");
        }
        assert!(fast < 100_000, "{ext}: level 1 gave {fast} bytes");
        assert!(best <= fast + 64, "{ext}: {best} vs {fast}");
    }
}

#[test]
fn encrypted_archives_round_trip_and_reject_bad_passwords() {
    for ext in ["zip", "7z"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("s.{ext}"));
        let create = CreateOptions {
            password: Some("p@ss 密码".into()),
            ..opts(3)
        };
        pack(&src, &archive, &create).unwrap();

        let none = unpack(&archive, &tmp.path().join("o1"), &ExtractOptions::default());
        assert!(matches!(none, Err(Error::Password)), "{ext}: {none:?}");

        let wrong = ExtractOptions {
            password: Some("nope".into()),
            ..ExtractOptions::default()
        };
        let res = unpack(&archive, &tmp.path().join("o2"), &wrong);
        assert!(
            matches!(
                res,
                Err(Error::Password) | Err(Error::Corrupt(_)) | Err(Error::Io(_))
            ),
            "{ext}: {res:?}"
        );

        let good = ExtractOptions {
            password: Some("p@ss 密码".into()),
            ..ExtractOptions::default()
        };
        let out = tmp.path().join("o3");
        unpack(&archive, &out, &good).unwrap();
        assert_same_tree(&src, &out.join("proj"), ext);
    }
}

#[test]
fn encrypted_7z_hides_file_names() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("s.7z");
    let create = CreateOptions {
        password: Some("pw".into()),
        ..opts(3)
    };
    pack(&src, &archive, &create).unwrap();

    let raw = fs::read(&archive).unwrap();
    assert!(
        !raw.windows(10).any(|w| w == b"readme.txt"),
        "file name visible in the raw bytes"
    );
    let mut r = rz_formats::open(&archive).unwrap();
    assert!(matches!(r.entries(), Err(Error::Password)));
    r.set_password("pw").unwrap();
    assert!(r
        .entries()
        .unwrap()
        .iter()
        .any(|e| e.path.ends_with("readme.txt")));
}

#[test]
fn formats_without_encryption_refuse_a_password() {
    for ext in ["tar", "tar.gz", "tar.xz", "tar.zst", "tar.bz2", "gz", "xz"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let out = tmp.path().join(format!("a.{ext}"));
        let create = CreateOptions {
            password: Some("x".into()),
            ..opts(3)
        };
        let res = pack(&src, &out, &create);
        assert!(matches!(res, Err(Error::Unsupported(_))), "{ext}: {res:?}");
        assert!(!out.exists(), "{ext}");
    }
}

#[test]
fn single_file_formats_round_trip() {
    for ext in SINGLE_FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("notes.txt");
        let content = "line of text\n".repeat(20_000);
        fs::write(&file, &content).unwrap();
        rz_platform::set_mtime(&file, MTIME).unwrap();

        let archive = tmp.path().join(format!("notes.txt.{ext}"));
        let rep = pack(&file, &archive, &opts(5)).unwrap();
        assert_eq!(rep.files, 1, "{ext}");

        let mut r = rz_formats::open(&archive).unwrap();
        let entries = r.entries().unwrap();
        assert_eq!(entries.len(), 1, "{ext}");
        assert_eq!(entries[0].path, "notes.txt", "{ext}");
        assert_eq!(entries[0].size, content.len() as u64, "{ext}");

        let out = tmp.path().join("out");
        unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        assert_eq!(
            fs::read_to_string(out.join("notes.txt")).unwrap(),
            content,
            "{ext}"
        );
    }
}

#[test]
fn single_file_formats_refuse_folders_and_several_files() {
    for ext in SINGLE_FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let out = tmp.path().join(format!("a.{ext}"));
        let res = pack(&src, &out, &opts(3));
        assert!(matches!(res, Err(Error::Unsupported(_))), "{ext}: {res:?}");
        assert!(!out.exists(), "{ext}");

        let both = compress(
            &[src.join("readme.txt"), src.join("empty.bin")],
            &out,
            OutputFormat::from_path(&out).unwrap(),
            &opts(3),
            &CancelToken::new(),
            &mut NoProgress,
        );
        assert!(
            matches!(both, Err(Error::Unsupported(_))),
            "{ext}: {both:?}"
        );
        assert!(!out.exists(), "{ext}");
    }
}

#[test]
fn compressed_tar_is_recognised_inside_any_stream_codec() {
    for ext in ["tar.gz", "tar.xz", "tar.zst", "tar.bz2"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &opts(3)).unwrap();
        // The extension is only a hint: rename it and the content is still found.
        let renamed = tmp.path().join("mystery.bin");
        fs::rename(&archive, &renamed).unwrap();
        let mut r = rz_formats::open(&renamed).unwrap();
        assert_eq!(r.format_name(), ext);
        assert!(r.entries().unwrap().len() >= 9, "{ext}");
    }
}

#[test]
fn thousands_of_files_do_not_exhaust_file_descriptors() {
    // macOS allows only 256 open files by default; sources must be opened lazily.
    for ext in ["7z", "zip", "tar.zst"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("many");
        fs::create_dir_all(&src).unwrap();
        for i in 0..3000 {
            fs::write(
                src.join(format!("f{i:05}.txt")),
                format!("file number {i}\n"),
            )
            .unwrap();
        }
        let archive = tmp.path().join(format!("m.{ext}"));
        let rep = pack(&src, &archive, &opts(3)).unwrap();
        assert_eq!(rep.files, 3000, "{ext}");
        let out = tmp.path().join("out");
        unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        assert_eq!(count_entries(&out.join("many")), 3000, "{ext}");
        assert_eq!(
            fs::read_to_string(out.join("many/f02999.txt")).unwrap(),
            "file number 2999\n",
            "{ext}"
        );
    }
}

#[test]
fn failed_compress_leaves_no_output() {
    for ext in TREE_FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join(format!("a.{ext}"));
        let res = compress(
            &[tmp.path().join("missing")],
            &out,
            OutputFormat::from_path(&out).unwrap(),
            &opts(3),
            &CancelToken::new(),
            &mut NoProgress,
        );
        assert!(res.is_err(), "{ext}");
        assert!(!out.exists(), "{ext}");
        assert!(temp_leftovers(tmp.path(), &out).is_empty(), "{ext}");
    }
}

#[test]
fn cancelling_compress_cleans_up() {
    for ext in TREE_FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let out = tmp.path().join(format!("a.{ext}"));
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        // The first progress report arrives early, while files are still being read.
        let mut sink = move |_: Progress| c2.cancel();
        let res = compress(
            &[src],
            &out,
            OutputFormat::from_path(&out).unwrap(),
            &opts(3),
            &cancel,
            &mut sink,
        );
        assert!(matches!(res, Err(Error::Cancelled)), "{ext}: {res:?}");
        assert!(!out.exists(), "{ext}");
        assert!(temp_leftovers(tmp.path(), &out).is_empty(), "{ext}");
    }
}

#[test]
fn compress_reports_monotonic_progress_that_ends_complete() {
    for ext in ["zip", "7z", "tar.gz"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let mut seen: Vec<Progress> = Vec::new();
        let mut sink = |p: Progress| seen.push(p);
        compress(
            &[src],
            &tmp.path().join(format!("a.{ext}")),
            OutputFormat::from_path(Path::new(&format!("a.{ext}"))).unwrap(),
            &opts(3),
            &CancelToken::new(),
            &mut sink,
        )
        .unwrap();
        assert!(!seen.is_empty(), "{ext}");
        assert!(
            seen.windows(2).all(|w| w[1].bytes_done >= w[0].bytes_done),
            "{ext}"
        );
        let last = seen.last().unwrap();
        assert_eq!(last.bytes_done, last.bytes_total, "{ext}");
        assert_eq!(last.files_done, last.files_total, "{ext}");
    }
}

#[test]
fn extract_reports_progress_that_ends_complete() {
    for ext in TREE_FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &opts(3)).unwrap();
        let mut seen: Vec<Progress> = Vec::new();
        let mut sink = |p: Progress| seen.push(p);
        let mut r = rz_formats::open(&archive).unwrap();
        extract(
            &mut *r,
            &tmp.path().join("out"),
            &ExtractOptions::default(),
            &CancelToken::new(),
            &mut sink,
        )
        .unwrap();
        let last = seen.last().unwrap();
        assert_eq!(last.bytes_done, last.bytes_total, "{ext}");
        assert_eq!(last.files_done, last.files_total, "{ext}");
    }
}

#[test]
fn archive_inside_its_own_source_is_not_added_to_itself() {
    for ext in ["zip", "7z", "tar.gz"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = src.join(format!("self.{ext}"));
        // A leftover temporary from a crashed run is recognizable and never packed.
        fs::write(src.join(format!(".self.{ext}.rz-abandoned.part")), b"old").unwrap();
        pack(&src, &archive, &opts(3)).unwrap();
        let mut r = rz_formats::open(&archive).unwrap();
        assert!(
            r.entries()
                .unwrap()
                .iter()
                .all(|e| !e.path.contains("self.")),
            "{ext}"
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinks_are_skipped_when_compressing() {
    for ext in TREE_FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        std::os::unix::fs::symlink("/etc/passwd", src.join("link")).unwrap();
        let archive = tmp.path().join(format!("a.{ext}"));
        let rep = pack(&src, &archive, &opts(3)).unwrap();
        assert_eq!(rep.skipped, 1, "{ext}");
        let mut r = rz_formats::open(&archive).unwrap();
        assert!(
            r.entries()
                .unwrap()
                .iter()
                .all(|e| !e.path.ends_with("link")),
            "{ext}"
        );
    }
}

#[test]
fn cancelling_extract_stops_before_later_files() {
    for ext in TREE_FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &opts(3)).unwrap();
        let out = tmp.path().join("out");
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let mut sink = move |_: Progress| c2.cancel();
        let mut r = rz_formats::open(&archive).unwrap();
        let res = extract(
            &mut *r,
            &out,
            &ExtractOptions::default(),
            &cancel,
            &mut sink,
        );
        assert!(matches!(res, Err(Error::Cancelled)), "{ext}: {res:?}");
        assert!(count_entries(&out) < 6, "{ext}");
    }
}

#[test]
fn overwrite_policies() {
    for ext in ["zip", "7z", "tar.gz"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &opts(3)).unwrap();
        let out = tmp.path().join("out");
        unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        let target = out.join("proj/readme.txt");
        fs::write(&target, "edited").unwrap();

        let refuse = ExtractOptions {
            overwrite: Overwrite::Error,
            ..ExtractOptions::default()
        };
        let err = unpack(&archive, &out, &refuse);
        assert!(matches!(err, Err(Error::Io(_))), "{ext}: {err:?}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "edited", "{ext}");

        // The default keeps both: the edited folder stays, the new one is numbered.
        let kept = unpack(&archive, &out, &ExtractOptions::default())
            .unwrap_or_else(|e| panic!("{ext}: failed to keep both versions: {e}"));
        // The report names what was written, so a UI can show the new folder.
        assert_eq!(kept.roots, vec![out.join("proj (1)")], "{ext}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "edited", "{ext}");
        assert_eq!(
            fs::read_to_string(out.join("proj (1)/readme.txt")).unwrap(),
            "hello rustzip",
            "{ext}"
        );
        unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        assert!(out.join("proj (2)/readme.txt").exists(), "{ext}");

        let skip = ExtractOptions {
            overwrite: Overwrite::Skip,
            ..ExtractOptions::default()
        };
        let rep = unpack(&archive, &out, &skip).unwrap();
        assert!(rep.skipped >= 1, "{ext}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "edited", "{ext}");

        let replace = ExtractOptions {
            overwrite: Overwrite::Replace,
            ..ExtractOptions::default()
        };
        unpack(&archive, &out, &replace).unwrap();
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "hello rustzip",
            "{ext}"
        );
    }
}

#[test]
fn skipping_a_file_in_a_solid_archive_does_not_misalign_the_rest() {
    for ext in ["7z", "tar.xz"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &opts(3)).unwrap();
        let out = tmp.path().join("out");
        // Pre-create the large file so extraction has to skip it mid-stream.
        fs::create_dir_all(out.join("proj")).unwrap();
        fs::write(out.join("proj/big.dat"), "keep me").unwrap();
        let skip = ExtractOptions {
            overwrite: Overwrite::Skip,
            ..ExtractOptions::default()
        };
        unpack(&archive, &out, &skip).unwrap();
        assert_eq!(
            fs::read_to_string(out.join("proj/big.dat")).unwrap(),
            "keep me",
            "{ext}"
        );
        assert_eq!(
            fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
            "hello rustzip",
            "{ext}"
        );
        assert_eq!(
            fs::read_to_string(out.join("proj/docs/说明.md")).unwrap(),
            "# 中文文件名\n",
            "{ext}"
        );
    }
}

#[test]
fn entries_report_kinds_and_sizes() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    for ext in TREE_FORMATS {
        let archive = tmp.path().join(format!("k.{ext}"));
        pack(&src, &archive, &opts(3)).unwrap();
        let mut r = rz_formats::open(&archive).unwrap();
        let entries = r.entries().unwrap();
        let readme = entries
            .iter()
            .find(|e| e.path.ends_with("readme.txt"))
            .unwrap();
        assert_eq!(readme.kind, EntryKind::File, "{ext}");
        assert_eq!(readme.size, "hello rustzip".len() as u64, "{ext}");
        assert!(entries.iter().any(|e| e.kind == EntryKind::Dir), "{ext}");
        assert_eq!(
            entries.iter().filter(|e| e.kind == EntryKind::File).count(),
            6,
            "{ext}"
        );
    }
}

#[test]
fn parallel_and_sequential_extraction_give_identical_trees() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("many");
    fs::create_dir_all(src.join("sub/deeper")).unwrap();
    for i in 0..300 {
        let dir = match i % 3 {
            0 => src.clone(),
            1 => src.join("sub"),
            _ => src.join("sub/deeper"),
        };
        fs::write(
            dir.join(format!("f{i:03}.bin")),
            pseudo_random(1000 + i * 37, i as u64 + 1),
        )
        .unwrap();
    }
    fs::write(src.join("huge.bin"), pseudo_random(3_000_000, 99)).unwrap();
    for f in walk_files(&src) {
        rz_platform::set_mtime(&f, MTIME).unwrap();
    }
    let archive = tmp.path().join("p.zip");
    pack(&src, &archive, &opts(3)).unwrap();

    let one = ExtractOptions {
        threads: 1,
        ..ExtractOptions::default()
    };
    let many = ExtractOptions {
        threads: 6,
        ..ExtractOptions::default()
    };
    let a = unpack(&archive, &tmp.path().join("seq"), &one).unwrap();
    let b = unpack(&archive, &tmp.path().join("par"), &many).unwrap();
    assert_eq!((a.files, a.dirs, a.bytes), (b.files, b.dirs, b.bytes));
    assert_eq!(a.files, 301);
    assert_same_tree(&src, &tmp.path().join("seq/many"), "sequential");
    assert_same_tree(&src, &tmp.path().join("par/many"), "parallel");
}

#[test]
fn parallel_extraction_reports_a_corrupt_entry_and_stops() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("many");
    fs::create_dir_all(&src).unwrap();
    for i in 0..40 {
        fs::write(src.join(format!("f{i:02}.txt")), "A".repeat(5000)).unwrap();
    }
    let archive = tmp.path().join("c.zip");
    // Stored, so flipping bytes in a file's data is a CRC error and not a decoder crash.
    pack(&src, &archive, &opts(0)).unwrap();
    let mut bytes = fs::read(&archive).unwrap();
    let pos = bytes
        .windows(64)
        .position(|w| w.iter().all(|b| *b == b'A'))
        .unwrap();
    bytes[pos + 1000] = b'Z';
    fs::write(&archive, bytes).unwrap();

    let many = ExtractOptions {
        threads: 4,
        ..ExtractOptions::default()
    };
    let res = unpack(&archive, &tmp.path().join("out"), &many);
    assert!(res.is_err(), "corruption went unnoticed");
}

#[test]
fn parallel_extraction_honours_cancellation() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("many");
    fs::create_dir_all(&src).unwrap();
    for i in 0..200 {
        fs::write(src.join(format!("f{i:03}.bin")), pseudo_random(20_000, i)).unwrap();
    }
    let archive = tmp.path().join("p.zip");
    pack(&src, &archive, &opts(1)).unwrap();
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let mut sink = move |_: Progress| c2.cancel();
    let mut r = rz_formats::open(&archive).unwrap();
    let many = ExtractOptions {
        threads: 4,
        ..ExtractOptions::default()
    };
    let res = extract(&mut *r, &tmp.path().join("out"), &many, &cancel, &mut sink);
    assert!(matches!(res, Err(Error::Cancelled)), "{res:?}");
    assert!(count_entries(&tmp.path().join("out")) < 200);
}

#[test]
fn duplicate_names_fall_back_to_sequential_extraction() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("d.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    let o = zip::write::SimpleFileOptions::default();
    for (name, data) in [("a.txt", "first"), ("b.txt", "x"), ("A.TXT", "second")] {
        w.start_file(name, o).unwrap();
        w.write_all(data.as_bytes()).unwrap();
    }
    w.finish().unwrap();
    let replace = ExtractOptions {
        overwrite: Overwrite::Replace,
        threads: 4,
        ..ExtractOptions::default()
    };
    // Must not race: both names map to one file on case-insensitive filesystems.
    assert!(unpack(&archive, &tmp.path().join("out"), &replace).is_ok());
}

#[test]
fn case_colliding_apk_resources_fail_before_extracting_on_an_insensitive_volume() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let probe = tmp.path().join("case-probe");
    fs::write(&probe, b"probe").unwrap();
    let sensitive = !tmp.path().join("CASE-PROBE").exists();
    fs::remove_file(probe).unwrap();
    let archive = tmp.path().join("resources.apk");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for (name, bytes) in [
        ("before.txt", "before"),
        ("res/9N.9.png", "first"),
        ("res/9n.9.png", "second"),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    let out = tmp.path().join("out");
    let result = unpack(&archive, &out, &ExtractOptions::default());
    if sensitive {
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fs::read(out.join("res/9N.9.png")).unwrap(), b"first");
        assert_eq!(fs::read(out.join("res/9n.9.png")).unwrap(), b"second");
    } else {
        assert!(
            matches!(result, Err(Error::NameConflict { ref first, ref second }) if first == "res/9N.9.png" && second == "res/9n.9.png"),
            "{result:?}"
        );
        assert!(
            !out.exists(),
            "a name conflict must be detected before any files are extracted"
        );
    }
}
