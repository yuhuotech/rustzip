#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use rz_core::{CancelToken, CreateOptions, Error, NoProgress, Progress};
use rz_engine::{extract, ExtractOptions};
use std::fs;
use std::path::Path;

fn opts() -> CreateOptions {
    CreateOptions {
        threads: 2,
        ..CreateOptions::default()
    }
}

fn only(paths: &[&str]) -> ExtractOptions {
    ExtractOptions {
        only: Some(paths.iter().map(|p| p.to_string()).collect()),
        ..ExtractOptions::default()
    }
}

fn exists(root: &Path, rel: &str) -> bool {
    root.join(rel).exists()
}

const FORMATS: [&str; 5] = ["zip", "7z", "tar", "tar.zst", "tar.xz"];

#[test]
fn extracting_one_file_leaves_everything_else_behind() {
    for ext in FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &opts()).unwrap();
        let out = tmp.path().join("out");
        let rep = unpack(&archive, &out, &only(&["proj/readme.txt"])).unwrap();
        assert_eq!(rep.files, 1, "{ext}");
        assert_eq!(
            fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
            "hello rustzip",
            "{ext}"
        );
        assert!(!exists(&out, "proj/big.dat"), "{ext}");
        assert!(!exists(&out, "proj/docs"), "{ext}");
    }
}

#[test]
fn choosing_a_folder_takes_everything_inside_it() {
    for ext in FORMATS {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &opts()).unwrap();
        let out = tmp.path().join("out");
        unpack(&archive, &out, &only(&["proj/docs"])).unwrap();
        assert!(exists(&out, "proj/docs/说明.md"), "{ext}");
        assert!(exists(&out, "proj/docs/深层/日本語.txt"), "{ext}");
        assert!(!exists(&out, "proj/readme.txt"), "{ext}");
        assert!(!exists(&out, "proj/big.dat"), "{ext}");
    }
}

#[test]
fn several_choices_and_unnormalised_spellings_work() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("a.zip");
    pack(&src, &archive, &opts()).unwrap();
    let out = tmp.path().join("out");
    unpack(
        &archive,
        &out,
        &only(&["./proj/readme.txt", "proj\\run.sh/"]),
    )
    .unwrap();
    assert!(exists(&out, "proj/readme.txt"));
    assert!(exists(&out, "proj/run.sh"));
    assert!(!exists(&out, "proj/big.dat"));
}

#[test]
fn a_name_that_is_only_a_prefix_does_not_match() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("a.zip");
    pack(&src, &archive, &opts()).unwrap();
    let out = tmp.path().join("out");
    // "proj/doc" is not "proj/docs".
    let rep = unpack(&archive, &out, &only(&["proj/doc"])).unwrap();
    assert_eq!(rep.files, 0);
    assert!(!exists(&out, "proj/docs"));
}

#[test]
fn nothing_selected_extracts_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("a.zip");
    pack(&src, &archive, &opts()).unwrap();
    let out = tmp.path().join("out");
    let rep = unpack(&archive, &out, &only(&[])).unwrap();
    assert_eq!((rep.files, rep.dirs), (0, 0));
    assert_eq!(count_entries(&out), 0);
}

#[test]
fn selection_works_in_parallel_extraction_and_counts_only_chosen_files() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("many");
    fs::create_dir_all(src.join("keep")).unwrap();
    fs::create_dir_all(src.join("skip")).unwrap();
    for i in 0..60 {
        fs::write(
            src.join("keep").join(format!("k{i}.txt")),
            format!("keep {i}"),
        )
        .unwrap();
        fs::write(src.join("skip").join(format!("s{i}.txt")), "skip").unwrap();
    }
    let archive = tmp.path().join("p.zip");
    pack(&src, &archive, &opts()).unwrap();
    let out = tmp.path().join("out");
    let ex = ExtractOptions {
        threads: 4,
        only: Some(vec!["many/keep".into()]),
        ..ExtractOptions::default()
    };
    let mut last = Progress::default();
    let mut sink = |p: Progress| last = p;
    let mut r = rz_formats::open(&archive).unwrap();
    let rep = extract(&mut *r, &out, &ex, &CancelToken::new(), &mut sink).unwrap();
    assert_eq!(rep.files, 60);
    assert_eq!(count_entries(&out.join("many/keep")), 60);
    assert!(!exists(&out, "many/skip"));
    // Progress is relative to what was chosen, not to the whole archive.
    assert_eq!(last.bytes_done, last.bytes_total);
    assert_eq!(last.files_total, 61); // 60 files + the folder entry
}

#[test]
fn a_selected_unsafe_path_is_still_rejected_and_unselected_ones_do_not_matter() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("e.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    let o = zip::write::SimpleFileOptions::default();
    for name in ["good.txt", "../evil.txt"] {
        w.start_file(name, o).unwrap();
        w.write_all(b"x").unwrap();
    }
    w.finish().unwrap();

    // The dangerous entry is not part of the selection, so it is never looked at.
    let out = tmp.path().join("ok");
    unpack(&archive, &out, &only(&["good.txt"])).unwrap();
    assert!(exists(&out, "good.txt"));

    // Selecting it does not make it safe.
    let out2 = tmp.path().join("bad");
    let res = unpack(&archive, &out2, &only(&["../evil.txt"]));
    assert!(matches!(res, Err(Error::UnsafePath(_))), "{res:?}");
    assert!(!tmp.path().join("evil.txt").exists());
}

#[test]
fn selection_does_not_disturb_the_solid_stream_for_later_files() {
    // The wanted file comes after skipped ones inside one solid block.
    for ext in ["7z", "tar.xz"] {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_tree(tmp.path());
        let archive = tmp.path().join(format!("a.{ext}"));
        pack(&src, &archive, &opts()).unwrap();
        let out = tmp.path().join("out");
        unpack(
            &archive,
            &out,
            &only(&["proj/run.sh", "proj/docs/深层/日本語.txt"]),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(out.join("proj/docs/深层/日本語.txt")).unwrap(),
            "テスト",
            "{ext}"
        );
        assert_eq!(
            fs::read_to_string(out.join("proj/run.sh")).unwrap(),
            "#!/bin/sh\necho ok\n",
            "{ext}"
        );
        let _ = NoProgress;
    }
}
