#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use rz_core::{CancelToken, CreateOptions, EntryKind, Error, NoProgress, Progress};
use rz_engine::{compress, extract, ExtractOptions, Overwrite};
use rz_formats::OutputFormat;
use std::fs;

const FORMATS: [&str; 3] = ["zip", "tar", "tar.gz"];

#[test]
fn round_trip_preserves_content_names_and_times() {
    for ext in FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        let rep = pack(&src, &archive, &CreateOptions::default()).unwrap();
        assert_eq!(rep.files, 6, "{ext}");
        assert!(rep.dirs >= 3, "{ext}");

        let out = tmp.path().join("out");
        unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        assert_same_tree(&src, &out.join("proj"), ext);
    }
}

#[cfg(unix)]
#[test]
fn round_trip_preserves_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    for ext in FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &CreateOptions::default()).unwrap();
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
    for level in [0, 1, 6, 9] {
        for ext in ["zip", "tar.gz"] {
            let tmp = tempfile::tempdir().unwrap();
            let src = make_tree(tmp.path());
            let archive = tmp.path().join(format!("a.{ext}"));
            let opts = CreateOptions {
                level,
                password: None,
            };
            pack(&src, &archive, &opts).unwrap();
            let out = tmp.path().join("out");
            unpack(&archive, &out, &ExtractOptions::default()).unwrap();
            assert_same_tree(&src, &out.join("proj"), ext);
        }
    }
}

#[test]
fn higher_level_is_not_larger_on_compressible_data() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("data");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("t.txt"), "the quick brown fox ".repeat(50_000)).unwrap();
    let size = |level| {
        let archive = tmp.path().join(format!("l{level}.zip"));
        let opts = CreateOptions {
            level,
            password: None,
        };
        pack(&src, &archive, &opts).unwrap();
        fs::metadata(archive).unwrap().len()
    };
    let (stored, fast, best) = (size(0), size(1), size(9));
    assert!(fast < stored / 10, "{fast} vs {stored}");
    assert!(best <= fast, "{best} vs {fast}");
}

#[test]
fn encrypted_zip_round_trip_and_password_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("s.zip");
    let opts = CreateOptions {
        level: 6,
        password: Some("p@ss 密码".into()),
    };
    pack(&src, &archive, &opts).unwrap();

    let mut r = rz_formats::open(&archive).unwrap();
    assert!(r.entries().unwrap().iter().any(|e| e.encrypted));

    let none = unpack(&archive, &tmp.path().join("o1"), &ExtractOptions::default());
    assert!(matches!(none, Err(Error::Password)), "{none:?}");

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
        "{res:?}"
    );

    let good = ExtractOptions {
        password: Some("p@ss 密码".into()),
        ..ExtractOptions::default()
    };
    let out = tmp.path().join("o3");
    unpack(&archive, &out, &good).unwrap();
    assert_same_tree(&src, &out.join("proj"), "encrypted");
}

#[test]
fn tar_cannot_be_encrypted() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let opts = CreateOptions {
        level: 6,
        password: Some("x".into()),
    };
    let res = pack(&src, &tmp.path().join("a.tar.gz"), &opts);
    assert!(matches!(res, Err(Error::Unsupported(_))), "{res:?}");
    assert!(!tmp.path().join("a.tar.gz").exists());
}

#[test]
fn failed_compress_leaves_no_output() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("a.zip");
    let res = compress(
        &[tmp.path().join("missing")],
        &out,
        OutputFormat::Zip,
        &CreateOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    );
    assert!(res.is_err());
    assert!(!out.exists());
    assert!(!tmp.path().join("a.zip.rz-part").exists());
}

#[test]
fn cancelling_compress_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let out = tmp.path().join("a.zip");
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let mut sink = move |p: Progress| {
        if p.files_done >= 1 {
            c2.cancel();
        }
    };
    let res = compress(
        &[src],
        &out,
        OutputFormat::Zip,
        &CreateOptions::default(),
        &cancel,
        &mut sink,
    );
    assert!(matches!(res, Err(Error::Cancelled)), "{res:?}");
    assert!(!out.exists());
    assert!(!tmp.path().join("a.zip.rz-part").exists());
}

#[test]
fn compress_reports_monotonic_progress_that_ends_complete() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let mut seen: Vec<Progress> = Vec::new();
    let mut sink = |p: Progress| seen.push(p);
    compress(
        &[src],
        &tmp.path().join("a.zip"),
        OutputFormat::Zip,
        &CreateOptions::default(),
        &CancelToken::new(),
        &mut sink,
    )
    .unwrap();
    assert!(!seen.is_empty());
    assert!(seen.windows(2).all(|w| w[1].bytes_done >= w[0].bytes_done));
    let last = seen.last().unwrap();
    assert_eq!(last.bytes_done, last.bytes_total);
    assert_eq!(last.files_done, last.files_total);
}

#[test]
fn archive_inside_its_own_source_is_not_added_to_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = src.join("self.zip");
    fs::write(&archive, b"old").unwrap();
    pack(&src, &archive, &CreateOptions::default()).unwrap();
    let mut r = rz_formats::open(&archive).unwrap();
    assert!(r
        .entries()
        .unwrap()
        .iter()
        .all(|e| !e.path.ends_with("self.zip")));
}

#[cfg(unix)]
#[test]
fn symlinks_are_skipped_when_compressing() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    std::os::unix::fs::symlink("/etc/passwd", src.join("link")).unwrap();
    let archive = tmp.path().join("a.zip");
    let rep = pack(&src, &archive, &CreateOptions::default()).unwrap();
    assert_eq!(rep.skipped, 1);
    let mut r = rz_formats::open(&archive).unwrap();
    assert!(r
        .entries()
        .unwrap()
        .iter()
        .all(|e| !e.path.ends_with("link")));
}

#[test]
fn cancelling_extract_stops_before_later_files() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("a.zip");
    pack(&src, &archive, &CreateOptions::default()).unwrap();
    let out = tmp.path().join("out");
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let mut sink = move |p: Progress| {
        if p.files_done >= 2 {
            c2.cancel();
        }
    };
    let mut r = rz_formats::open(&archive).unwrap();
    let res = extract(
        &mut *r,
        &out,
        &ExtractOptions::default(),
        &cancel,
        &mut sink,
    );
    assert!(matches!(res, Err(Error::Cancelled)), "{res:?}");
    let total = r.entries().unwrap().len();
    assert!(count_entries(&out) < total);
}

#[test]
fn overwrite_policies() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("a.zip");
    pack(&src, &archive, &CreateOptions::default()).unwrap();
    let out = tmp.path().join("out");
    unpack(&archive, &out, &ExtractOptions::default()).unwrap();
    let target = out.join("proj/readme.txt");
    fs::write(&target, "edited").unwrap();

    let err = unpack(&archive, &out, &ExtractOptions::default());
    assert!(matches!(err, Err(Error::Io(_))), "{err:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "edited");

    let skip = ExtractOptions {
        overwrite: Overwrite::Skip,
        ..ExtractOptions::default()
    };
    let rep = unpack(&archive, &out, &skip).unwrap();
    assert!(rep.skipped >= 1);
    assert_eq!(fs::read_to_string(&target).unwrap(), "edited");

    let replace = ExtractOptions {
        overwrite: Overwrite::Replace,
        ..ExtractOptions::default()
    };
    unpack(&archive, &out, &replace).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "hello rustzip");
}

#[test]
fn entries_report_kinds_and_sizes() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    for ext in FORMATS {
        let archive = tmp.path().join(format!("k.{ext}"));
        pack(&src, &archive, &CreateOptions::default()).unwrap();
        let mut r = rz_formats::open(&archive).unwrap();
        let entries = r.entries().unwrap();
        let readme = entries
            .iter()
            .find(|e| e.path.ends_with("readme.txt"))
            .unwrap();
        assert_eq!(readme.kind, EntryKind::File, "{ext}");
        assert_eq!(readme.size, "hello rustzip".len() as u64, "{ext}");
        assert!(entries.iter().any(|e| e.kind == EntryKind::Dir), "{ext}");
    }
}
