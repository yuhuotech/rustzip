#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};

fn rz(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rz"))
        .args(args)
        .output()
        .unwrap()
}

fn s(o: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn sample(root: &Path) {
    fs::create_dir_all(root.join("proj/docs")).unwrap();
    fs::write(root.join("proj/readme.txt"), "hello rustzip").unwrap();
    fs::write(root.join("proj/docs/说明.md"), "# 中文文件名").unwrap();
    fs::write(root.join("proj/empty.bin"), "").unwrap();
}

#[test]
fn no_args_prints_usage_and_succeeds() {
    let o = rz(&[]);
    assert!(o.status.success());
    assert!(s(&o).0.contains("Usage"));
}

#[test]
fn add_list_test_extract_round_trip_for_every_format() {
    for ext in [
        "zip", "7z", "tar", "tar.gz", "tgz", "tar.xz", "tar.zst", "tar.bz2",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        sample(tmp.path());
        let archive = tmp.path().join(format!("out.{ext}"));
        let out = tmp.path().join("extracted");

        let o = rz(&["a", p(&archive), p(&tmp.path().join("proj"))]);
        assert!(o.status.success(), "{ext}: add failed: {:?}", s(&o));

        let o = rz(&["l", p(&archive)]);
        let (stdout, _) = s(&o);
        assert!(o.status.success());
        assert!(stdout.contains("proj/readme.txt"), "{ext}: {stdout}");
        assert!(stdout.contains("proj/docs/说明.md"), "{ext}: {stdout}");

        let o = rz(&["t", p(&archive)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        assert!(s(&o).0.starts_with("OK"));

        let o = rz(&["x", p(&archive), "-o", p(&out)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        assert_eq!(
            fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
            "hello rustzip"
        );
        assert_eq!(
            fs::read_to_string(out.join("proj/docs/说明.md")).unwrap(),
            "# 中文文件名"
        );
        assert_eq!(fs::read(out.join("proj/empty.bin")).unwrap().len(), 0);
    }
}

#[test]
fn encrypted_zip_needs_the_right_password() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    let archive = tmp.path().join("secret.zip");
    let src = tmp.path().join("proj");
    let o = rz(&["a", p(&archive), p(&src), "-p", "s3cret"]);
    assert!(o.status.success(), "{:?}", s(&o));

    let out = tmp.path().join("o1");
    let o = rz(&["x", p(&archive), "-o", p(&out)]);
    assert!(!o.status.success());
    assert!(s(&o).1.to_lowercase().contains("password"));

    let o = rz(&["x", p(&archive), "-o", p(&out), "-p", "wrong"]);
    assert!(!o.status.success());

    let out = tmp.path().join("o2");
    let o = rz(&["x", p(&archive), "-o", p(&out), "-p", "s3cret"]);
    assert!(o.status.success(), "{:?}", s(&o));
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "hello rustzip"
    );
}

#[test]
fn extract_refuses_to_overwrite_unless_asked() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    let archive = tmp.path().join("a.zip");
    assert!(rz(&["a", p(&archive), p(&tmp.path().join("proj"))])
        .status
        .success());
    let out = tmp.path().join("out");
    assert!(rz(&["x", p(&archive), "-o", p(&out)]).status.success());
    fs::write(out.join("proj/readme.txt"), "edited").unwrap();

    assert!(!rz(&["x", p(&archive), "-o", p(&out)]).status.success());
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "edited"
    );

    assert!(rz(&["x", p(&archive), "-o", p(&out), "-n"])
        .status
        .success());
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "edited"
    );

    assert!(rz(&["x", p(&archive), "-o", p(&out), "-y"])
        .status
        .success());
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "hello rustzip"
    );
}

#[test]
fn extract_blocks_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("evil.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    w.start_file("../escaped.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    w.write_all(b"x").unwrap();
    w.finish().unwrap();

    let out = tmp.path().join("out");
    let o = rz(&["x", p(&archive), "-o", p(&out)]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("unsafe path"));
    assert!(!tmp.path().join("escaped.txt").exists());
}

#[test]
fn single_file_formats_round_trip_through_the_cli() {
    for ext in ["gz", "xz", "zst", "bz2"] {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("log.txt");
        fs::write(&file, "one line\n".repeat(5000)).unwrap();
        let archive = tmp.path().join(format!("log.txt.{ext}"));
        let o = rz(&["a", p(&archive), p(&file)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        let o = rz(&["t", p(&archive)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        let out = tmp.path().join("out");
        let o = rz(&["x", p(&archive), "-o", p(&out)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        assert_eq!(
            fs::read_to_string(out.join("log.txt")).unwrap(),
            "one line\n".repeat(5000),
            "{ext}"
        );
    }
}

#[test]
fn encrypted_7z_lists_only_with_the_password() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    let archive = tmp.path().join("secret.7z");
    let o = rz(&[
        "a",
        p(&archive),
        p(&tmp.path().join("proj")),
        "-p",
        "pw",
        "-t",
        "2",
    ]);
    assert!(o.status.success(), "{:?}", s(&o));

    let o = rz(&["l", p(&archive)]);
    assert!(!o.status.success());
    assert!(s(&o).1.to_lowercase().contains("password"));

    let o = rz(&["l", p(&archive), "-p", "pw"]);
    assert!(o.status.success(), "{:?}", s(&o));
    assert!(s(&o).0.contains("proj/readme.txt"));

    let o = rz(&["t", p(&archive), "-p", "pw"]);
    assert!(o.status.success(), "{:?}", s(&o));
}

#[test]
fn thread_count_does_not_change_the_result() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    for threads in ["1", "4"] {
        let archive = tmp.path().join(format!("t{threads}.zip"));
        let out = tmp.path().join(format!("o{threads}"));
        let src = tmp.path().join("proj");
        assert!(rz(&["a", p(&archive), p(&src), "-t", threads])
            .status
            .success());
        assert!(rz(&["x", p(&archive), "-o", p(&out), "-t", threads])
            .status
            .success());
        assert_eq!(
            fs::read_to_string(out.join("proj/docs/说明.md")).unwrap(),
            "# 中文文件名"
        );
    }
    assert!(!rz(&["a", "x.zip", "y", "-t", "0"]).status.success());
}

#[test]
fn single_file_formats_reject_folders_and_passwords_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    let o = rz(&[
        "a",
        p(&tmp.path().join("x.gz")),
        p(&tmp.path().join("proj")),
    ]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("one file"));
    assert!(!tmp.path().join("x.gz").exists());

    let o = rz(&[
        "a",
        p(&tmp.path().join("y.tar.gz")),
        p(&tmp.path().join("proj")),
        "-p",
        "pw",
    ]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("cannot be encrypted"));
}

#[test]
fn reports_clear_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let junk = tmp.path().join("junk.zip");
    fs::write(&junk, "this is not an archive").unwrap();
    let o = rz(&["l", p(&junk)]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("unrecognized archive format"));

    let o = rz(&["l", p(&tmp.path().join("missing.zip"))]);
    assert!(!o.status.success());

    // RAR is read-only by design.
    let o = rz(&["a", p(&tmp.path().join("x.rar")), p(&junk)]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("cannot create"));

    let o = rz(&["bogus", "x"]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("unknown command"));

    let o = rz(&["a", p(&tmp.path().join("y.zip"))]);
    assert!(!o.status.success());
}

#[test]
fn failed_add_leaves_no_partial_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("never.zip");
    let o = rz(&["a", p(&archive), p(&tmp.path().join("does-not-exist"))]);
    assert!(!o.status.success());
    assert!(!archive.exists());
    assert!(!tmp.path().join("never.zip.rz-part").exists());
}
