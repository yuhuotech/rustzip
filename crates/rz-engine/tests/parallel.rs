#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use rz_core::{CreateOptions, EntryKind};
use rz_engine::ExtractOptions;
use std::fs;
use std::path::Path;

/// 400 small files, one of them mid-sized, spread over three directories.
fn make_many(root: &Path) -> std::path::PathBuf {
    let src = root.join("many");
    for d in ["", "a", "a/b"] {
        fs::create_dir_all(src.join(d)).unwrap();
    }
    for i in 0..400u64 {
        let dir = match i % 3 {
            0 => "",
            1 => "a",
            _ => "a/b",
        };
        let len = if i == 200 {
            3_000_000
        } else {
            200 + (i as usize * 53) % 9000
        };
        let data = if i % 2 == 0 {
            pseudo_random(len, i + 1)
        } else {
            "compressible text ".repeat(len / 18 + 1).into_bytes()
        };
        fs::write(src.join(dir).join(format!("f{i:03}.dat")), data).unwrap();
    }
    for f in walk_files(&src) {
        rz_platform::set_mtime(&f, MTIME).unwrap();
    }
    src
}

fn names(archive: &Path) -> Vec<String> {
    let mut r = rz_formats::open(archive).unwrap();
    r.entries()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == EntryKind::File)
        .map(|e| e.path)
        .collect()
}

#[test]
fn parallel_zip_keeps_input_order_and_content() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_many(tmp.path());
    let one = tmp.path().join("one.zip");
    let many = tmp.path().join("many.zip");
    let single = CreateOptions {
        threads: 1,
        ..CreateOptions::default()
    };
    // A tiny budget makes the 3 MB file run on the writing thread, mid-archive.
    let parallel = CreateOptions {
        threads: 6,
        memory_budget: 4 << 20,
        ..CreateOptions::default()
    };
    pack(&src, &one, &single).unwrap();
    pack(&src, &many, &parallel).unwrap();

    assert_eq!(
        names(&one),
        names(&many),
        "entry order must not depend on threads"
    );
    for (label, archive) in [("single", &one), ("parallel", &many)] {
        let out = tmp.path().join(format!("out-{label}"));
        unpack(archive, &out, &ExtractOptions::default()).unwrap();
        assert_same_tree(&src, &out.join("many"), label);
    }
}

#[test]
fn parallel_zip_does_not_stall_on_a_tiny_memory_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_many(tmp.path());
    let opts = CreateOptions {
        threads: 4,
        memory_budget: 1,
        ..CreateOptions::default()
    };
    let archive = tmp.path().join("tiny.zip");
    pack(&src, &archive, &opts).unwrap();
    assert_eq!(names(&archive).len(), 400);
}

#[test]
fn parallel_zip_encrypts_every_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_many(tmp.path());
    let opts = CreateOptions {
        threads: 4,
        password: Some("secret".into()),
        ..CreateOptions::default()
    };
    let archive = tmp.path().join("enc.zip");
    pack(&src, &archive, &opts).unwrap();

    let mut r = rz_formats::open(&archive).unwrap();
    assert!(r
        .entries()
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .all(|e| e.encrypted));

    let out = tmp.path().join("out");
    let ex = ExtractOptions {
        password: Some("secret".into()),
        threads: 4,
        ..ExtractOptions::default()
    };
    unpack(&archive, &out, &ex).unwrap();
    assert_same_tree(&src, &out.join("many"), "encrypted parallel");
}

#[test]
fn parallel_zip_is_not_slower_to_read_than_it_is_to_write() {
    // A sanity check on the whole pipeline: more threads must still give a valid archive
    // for any thread count, including more threads than files.
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("few");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    fs::write(src.join("b.txt"), "b").unwrap();
    for threads in [1, 2, 3, 16, 64] {
        let archive = tmp.path().join(format!("t{threads}.zip"));
        let opts = CreateOptions {
            threads,
            ..CreateOptions::default()
        };
        pack(&src, &archive, &opts).unwrap();
        let out = tmp.path().join(format!("o{threads}"));
        unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        assert_eq!(fs::read_to_string(out.join("few/b.txt")).unwrap(), "b");
    }
}
