#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use rz_core::{CancelToken, CreateOptions, EntryKind, Error, Progress};
use rz_engine::{extract, ExtractOptions};
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

#[test]
fn sevenz_independent_blocks_extract_with_selection_and_memory_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("blocks.7z");
    let create = CreateOptions {
        level: 1,
        solid_block_size: Some(0),
        ..CreateOptions::default()
    };
    pack(&src, &archive, &create).unwrap();
    let reader = rz_formats::open(&archive).unwrap();
    let plan = reader.parallel_blocks(256 << 20).unwrap().unwrap();
    assert!(plan.entries.len() >= 2);
    assert!(plan.max_workers >= 2);
    assert!(reader.parallel_blocks(1).unwrap().is_none());

    for (label, budget) in [("parallel", 256 << 20), ("fallback", 1)] {
        let out = tmp.path().join(label);
        let opts = ExtractOptions {
            threads: 4,
            memory_budget: budget,
            ..ExtractOptions::default()
        };
        unpack(&archive, &out, &opts).unwrap();
        assert_same_tree(&src, &out.join("proj"), label);
    }

    let selected = tmp.path().join("selected");
    let opts = ExtractOptions {
        threads: 4,
        only: Some(vec!["proj/readme.txt".into(), "proj/big.dat".into()]),
        ..ExtractOptions::default()
    };
    let report = unpack(&archive, &selected, &opts).unwrap();
    assert_eq!(report.files, 2);
    assert_eq!(
        fs::read(selected.join("proj/readme.txt")).unwrap(),
        fs::read(src.join("readme.txt")).unwrap()
    );
    assert_eq!(
        fs::read(selected.join("proj/big.dat")).unwrap(),
        fs::read(src.join("big.dat")).unwrap()
    );
    assert!(!selected.join("proj/docs").exists());
}

#[test]
fn sevenz_parallel_blocks_support_password_and_cancellation() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("encrypted.7z");
    let create = CreateOptions {
        level: 1,
        password: Some("secret".into()),
        solid_block_size: Some(0),
        ..CreateOptions::default()
    };
    pack(&src, &archive, &create).unwrap();
    let mut reader = rz_formats::open(&archive).unwrap();
    reader.set_password("secret").unwrap();
    assert!(reader.parallel_blocks(256 << 20).unwrap().is_some());
    let opts = ExtractOptions {
        threads: 4,
        password: Some("secret".into()),
        ..ExtractOptions::default()
    };
    let out = tmp.path().join("encrypted-out");
    unpack(&archive, &out, &opts).unwrap();
    assert_same_tree(&src, &out.join("proj"), "encrypted 7z blocks");

    let cancel = CancelToken::new();
    let mut reader = rz_formats::open(&archive).unwrap();
    let cancel_opts = ExtractOptions {
        only: Some(vec!["proj/readme.txt".into(), "proj/big.dat".into()]),
        ..opts
    };
    let mut progress = |progress: Progress| {
        if progress.bytes_done > 0 {
            cancel.cancel();
        }
    };
    let result = extract(
        &mut *reader,
        &tmp.path().join("cancelled"),
        &cancel_opts,
        &cancel,
        &mut progress,
    );
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
}

#[test]
fn sevenz_parallel_blocks_report_corruption_and_wrong_password() {
    let tmp = tempfile::tempdir().unwrap();
    let src = make_tree(tmp.path());
    let archive = tmp.path().join("blocks.7z");
    pack(
        &src,
        &archive,
        &CreateOptions {
            solid_block_size: Some(0),
            level: 1,
            ..CreateOptions::default()
        },
    )
    .unwrap();
    let mut bytes = fs::read(&archive).unwrap();
    bytes[32] ^= 0x80;
    fs::write(&archive, bytes).unwrap();
    let opts = ExtractOptions {
        threads: 4,
        ..ExtractOptions::default()
    };
    assert!(unpack(&archive, &tmp.path().join("corrupt"), &opts).is_err());

    let encrypted = tmp.path().join("encrypted.7z");
    pack(
        &src,
        &encrypted,
        &CreateOptions {
            solid_block_size: Some(0),
            password: Some("secret".into()),
            level: 1,
            ..CreateOptions::default()
        },
    )
    .unwrap();
    let opts = ExtractOptions {
        threads: 4,
        password: Some("wrong".into()),
        ..ExtractOptions::default()
    };
    assert!(unpack(&encrypted, &tmp.path().join("wrong"), &opts).is_err());
}

#[test]
fn sevenz_parallel_solid_blocks_skip_unselected_files_without_losing_alignment() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("files");
    fs::create_dir_all(&src).unwrap();
    for index in 0..6 {
        fs::write(
            src.join(format!("file{index}.dat")),
            pseudo_random(24_000, index + 1),
        )
        .unwrap();
    }
    let archive = tmp.path().join("solid-blocks.7z");
    pack(
        &src,
        &archive,
        &CreateOptions {
            solid_block_size: Some(50_000),
            level: 1,
            ..CreateOptions::default()
        },
    )
    .unwrap();
    let reader = rz_formats::open(&archive).unwrap();
    let plan = reader.parallel_blocks(256 << 20).unwrap().unwrap();
    assert!(plan.entries.iter().filter(|group| group.len() >= 2).count() >= 2);

    let opts = ExtractOptions {
        threads: 4,
        only: Some(vec!["files/file1.dat".into(), "files/file5.dat".into()]),
        ..ExtractOptions::default()
    };
    let out = tmp.path().join("selected-solid");
    let report = unpack(&archive, &out, &opts).unwrap();
    assert_eq!(report.files, 2);
    for name in ["file1.dat", "file5.dat"] {
        assert_eq!(
            fs::read(src.join(name)).unwrap(),
            fs::read(out.join("files").join(name)).unwrap()
        );
    }
    assert!(!out.join("files/file0.dat").exists());
}

#[test]
fn bzip2_parallel_streams_round_trip_and_work_with_system_decoder() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("payload.bin");
    let data = pseudo_random(3_100_000, 9);
    fs::write(&src, &data).unwrap();
    for (label, budget) in [("parallel", 128 << 20), ("fallback", 1)] {
        let archive = tmp.path().join(format!("{label}.bz2"));
        let create = CreateOptions {
            level: 1,
            threads: 4,
            memory_budget: budget,
            ..CreateOptions::default()
        };
        pack(&src, &archive, &create).unwrap();
        let out = tmp.path().join(format!("{label}-out"));
        unpack(&archive, &out, &ExtractOptions::default()).unwrap();
        assert_eq!(fs::read(out.join(label)).unwrap(), data);
        if let Ok(output) = std::process::Command::new("bzip2")
            .arg("-dc")
            .arg(&archive)
            .output()
        {
            assert!(output.status.success());
            assert_eq!(output.stdout, data);
        }
    }

    let folder = tmp.path().join("source");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("data.bin"), &data).unwrap();
    let archive = tmp.path().join("tree.tar.bz2");
    pack(
        &folder,
        &archive,
        &CreateOptions {
            threads: 4,
            level: 1,
            ..CreateOptions::default()
        },
    )
    .unwrap();
    let out = tmp.path().join("tree-out");
    unpack(&archive, &out, &ExtractOptions::default()).unwrap();
    assert_eq!(fs::read(out.join("source/data.bin")).unwrap(), data);
    if let Ok(output) = std::process::Command::new("bsdtar")
        .arg("-xOf")
        .arg(&archive)
        .arg("source/data.bin")
        .output()
    {
        assert!(output.status.success());
        assert_eq!(output.stdout, data);
    }
}

#[test]
fn cancelling_parallel_bzip2_does_not_publish_an_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("payload.bin");
    fs::write(&src, pseudo_random(4_000_000, 12)).unwrap();
    let archive = tmp.path().join("cancel.bz2");
    let cancel = CancelToken::new();
    let mut progress = |progress: Progress| {
        if progress.bytes_done >= 1_000_000 {
            cancel.cancel();
        } else {
            // Keep progress callbacks observable despite the production throttle.
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
    };
    let result = rz_engine::compress(
        &[src],
        &archive,
        rz_formats::OutputFormat::from_path(&archive).unwrap(),
        &CreateOptions {
            threads: 4,
            memory_budget: 128 << 20,
            ..CreateOptions::default()
        },
        &cancel,
        &mut progress,
    );
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert!(!archive.exists());
    assert!(fs::read_dir(tmp.path()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".part")));
}
