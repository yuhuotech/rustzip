//! TASK-04 / FS-10: edits of the same archive must serialize, and an edit or
//! conversion killed at any point must never replace a good archive with a bad
//! one. The killed runs leave recognizable leftovers that never block a retry.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_app::modify::{edit_archive, Edit, EditReport};
use rz_core::{CancelToken, CreateOptions, EntryKind, NoProgress};
use rz_engine::{compress, extract, ExtractOptions, TransformOptions};
use rz_formats::{Compression, OutputFormat};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn opts() -> ExtractOptions {
    ExtractOptions::default()
}

fn run(archive: &Path, edit: &Edit) -> Result<EditReport, rz_core::Error> {
    edit_archive(
        archive,
        edit,
        &opts(),
        6,
        None,
        &CancelToken::new(),
        &mut NoProgress,
    )
}

fn pseudo_random(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// An archive big enough that an edit (unpack + repack) takes a while.
fn make_archive(dir: &Path, name: &str, big: bool) -> PathBuf {
    let src = dir.join("data");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.txt"), "alpha").unwrap();
    fs::write(src.join("b.txt"), "beta").unwrap();
    fs::write(src.join("c.txt"), "gamma").unwrap();
    if big {
        fs::write(src.join("sub/x.bin"), pseudo_random(8 << 20, 3)).unwrap();
        fs::write(src.join("sub/y.bin"), pseudo_random(8 << 20, 4)).unwrap();
    }
    let out = dir.join(name);
    compress(
        &[src],
        &out,
        OutputFormat::Zip,
        &CreateOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    out
}

fn names(path: &Path) -> Vec<String> {
    let mut r = rz_formats::open(path).unwrap();
    let mut v: Vec<String> = r
        .entries()
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .map(|e| rz_core::clean_path(&e.path))
        .collect();
    v.sort();
    v
}

fn rz_leftovers(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".rz-"))
        .collect()
}

// --------------------------------------------------------------- TASK-04 ----

#[test]
fn concurrent_edits_of_the_same_archive_apply_both_and_stay_valid() {
    let dir = tempfile::tempdir().unwrap();
    let archive = make_archive(dir.path(), "t.zip", true);

    let a = archive.clone();
    let b = archive.clone();
    let (ta, tb) = (
        std::thread::spawn(move || run(&a, &Edit::Delete(vec!["data/a.txt".into()]))),
        std::thread::spawn(move || run(&b, &Edit::Delete(vec!["data/b.txt".into()]))),
    );
    let ra = ta.join().unwrap();
    let rb = tb.join().unwrap();
    ra.expect("edit A failed");
    rb.expect("edit B failed");

    // Both deletions applied: the lost-update failure mode would leave one of
    // the two files behind.
    assert_eq!(
        names(&archive),
        ["data/c.txt", "data/sub/x.bin", "data/sub/y.bin"]
    );
    assert!(rz_leftovers(dir.path()).is_empty(), "scratch left behind");
}

#[test]
fn an_edit_running_alongside_a_conversion_never_corrupts_either_side() {
    let dir = tempfile::tempdir().unwrap();
    let archive = make_archive(dir.path(), "t.zip", true);

    let a = archive.clone();
    let out = dir.path().join("snapshot.7z");
    let (o1, o2) = (out.clone(), out.clone());
    let src2 = archive.clone();
    let edit = std::thread::spawn(move || run(&a, &Edit::Delete(vec!["data/a.txt".into()])));
    let convert = std::thread::spawn(move || {
        let topts = TransformOptions {
            format: OutputFormat::SevenZ,
            create: CreateOptions {
                threads: 1,
                ..Default::default()
            },
            limits: ExtractOptions::default(),
            encoding: None,
        };
        rz_engine::transform(
            &src2,
            &o1,
            &topts,
            false,
            &CancelToken::new(),
            &mut NoProgress,
        )
    });
    edit.join().unwrap().expect("edit failed");
    convert
        .join()
        .unwrap()
        .unwrap_or_else(|e| panic!("conversion failed: {e}"));

    // The edit won the archive; the conversion saw a consistent snapshot.
    assert_eq!(
        names(&archive),
        [
            "data/b.txt",
            "data/c.txt",
            "data/sub/x.bin",
            "data/sub/y.bin"
        ]
    );
    let mut snapshot = rz_formats::open(&o2).unwrap();
    snapshot.entries().unwrap(); // readable end to end
    assert!(rz_leftovers(dir.path()).is_empty());
}

// ---------------------------------------------------------------- FS-10 ----
// The child half of the kill tests: re-run this binary with RZ_EDIT_CHILD set.

fn child_mode() -> bool {
    std::env::var("RZ_EDIT_CHILD").is_ok()
}

fn run_child_role() {
    let role = std::env::var("RZ_EDIT_CHILD").unwrap();
    let dir = PathBuf::from(std::env::var("RZ_CHILD_DIR").unwrap());
    match role.as_str() {
        // Start an edit on a big archive and simply keep running; the parent
        // kills us while the edit is in flight.
        "edit-mid" => {
            let archive = make_archive(&dir, "kill.zip", true);
            fs::copy(&archive, dir.join("original-copy.zip")).unwrap();
            let _ = run(&archive, &Edit::Delete(vec!["data/a.txt".into()]));
        }
        // Finish an edit on a small archive, announce it, then idle until killed.
        "edit-after" => {
            let archive = make_archive(&dir, "published.zip", false);
            run(&archive, &Edit::Delete(vec!["data/a.txt".into()])).unwrap();
            fs::write(dir.join("marker-published"), b"1").unwrap();
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        // Convert a large archive; the parent kills us mid-conversion.
        "transform-mid" => {
            let source = make_archive(&dir, "kill.zip", true);
            fs::copy(&source, dir.join("original-copy.zip")).unwrap();
            let topts = TransformOptions {
                format: OutputFormat::Tar(Compression::Gzip),
                create: CreateOptions {
                    threads: 1,
                    ..Default::default()
                },
                limits: ExtractOptions::default(),
                encoding: None,
            };
            let _ = rz_engine::transform(
                &source,
                &dir.join("out.tar.gz"),
                &topts,
                false,
                &CancelToken::new(),
                &mut NoProgress,
            );
        }
        other => panic!("unknown child role {other}"),
    }
    std::process::exit(0);
}

fn spawn_child(test: &str, role: &str, dir: &Path) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(test)
        .arg("--nocapture")
        .env("RZ_EDIT_CHILD", role)
        .env("RZ_CHILD_DIR", dir)
        .spawn()
        .unwrap()
}

/// Waits until `pred` holds for some entry of `dir`, or fails the test.
fn wait_for_entry(dir: &Path, what: &str, pred: impl Fn(&str) -> bool) -> PathBuf {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for e in fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            if pred(&name) {
                return e.path();
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what} in {}",
            dir.display()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn an_edit_killed_midway_never_replaces_the_archive() {
    if child_mode() {
        run_child_role();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("kill.zip");
    let original = dir.path().join("original-copy.zip");

    let mut child = spawn_child(
        "an_edit_killed_midway_never_replaces_the_archive",
        "edit-mid",
        dir.path(),
    );
    // The scratch folder appearing means the edit is in flight.
    wait_for_entry(dir.path(), "the scratch folder", |n| {
        n.starts_with(".kill.zip.rz-edit-")
    });
    child.kill().unwrap();
    let _ = child.wait();

    // The original archive is intact and still usable.
    assert!(archive.exists());
    assert_eq!(
        fs::read(&archive).unwrap(),
        fs::read(&original).unwrap(),
        "the original changed"
    );
    assert_eq!(
        names(&archive),
        [
            "data/a.txt",
            "data/b.txt",
            "data/c.txt",
            "data/sub/x.bin",
            "data/sub/y.bin"
        ]
    );

    // Leftovers from the kill are recognizable (dot + .rz- prefix) and do not
    // block a fresh edit, which cleans up only its own files.
    let leftovers = rz_leftovers(dir.path());
    assert!(
        leftovers.iter().all(|n| n.starts_with(".kill.zip.rz-")),
        "unrecognizable leftovers: {leftovers:?}"
    );
    run(&archive, &Edit::Delete(vec!["data/a.txt".into()])).unwrap();
    assert_eq!(
        names(&archive),
        [
            "data/b.txt",
            "data/c.txt",
            "data/sub/x.bin",
            "data/sub/y.bin"
        ]
    );
}

#[test]
fn a_kill_after_publishing_leaves_the_complete_new_version() {
    if child_mode() {
        run_child_role();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn_child(
        "a_kill_after_publishing_leaves_the_complete_new_version",
        "edit-after",
        dir.path(),
    );
    wait_for_entry(dir.path(), "the publish marker", |n| {
        n == "marker-published"
    });
    child.kill().unwrap();
    let _ = child.wait();

    let archive = dir.path().join("published.zip");
    assert_eq!(
        names(&archive),
        ["data/b.txt", "data/c.txt"],
        "the published version is not the complete new one"
    );
    // It extracts correctly, end to end.
    let out = dir.path().join("out");
    let mut r = rz_formats::open(&archive).unwrap();
    extract(
        &mut *r,
        &out,
        &ExtractOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(fs::read_to_string(out.join("data/b.txt")).unwrap(), "beta");
}

#[test]
fn a_conversion_killed_midway_publishes_nothing() {
    if child_mode() {
        run_child_role();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original-copy.zip");

    let mut child = spawn_child(
        "a_conversion_killed_midway_publishes_nothing",
        "transform-mid",
        dir.path(),
    );
    // The engine's temporary output appears as soon as the conversion starts
    // writing; kill while it is there.
    wait_for_entry(dir.path(), "the conversion temporary", |n| {
        n.starts_with(".out.tar.gz.rz-")
    });
    child.kill().unwrap();
    let _ = child.wait();

    assert!(
        !dir.path().join("out.tar.gz").exists(),
        "a killed conversion published its output"
    );
    assert_eq!(
        fs::read(dir.path().join("kill.zip")).unwrap(),
        fs::read(&original).unwrap(),
        "the source archive changed"
    );
}
